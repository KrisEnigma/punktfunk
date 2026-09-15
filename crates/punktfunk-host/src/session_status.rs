//! Live native-session status for management `GET /status`.
//!
//! GameStream records its session in `AppState.{launch, stream, streaming}`.
//! The native plane never writes those fields — it is handed only the shared
//! stats recorder ([`crate::native::serve`]). This registry is the surface
//! the native video loop ([`crate::native::virtual_stream`]) publishes to,
//! keyed per session so concurrent sessions (up to `max_sessions`) each get
//! an entry.
//!
//! [`register`] on stream start; [`LiveSessionGuard`] removes the entry on
//! any scope exit. `/status` reads [`snapshot`]/[`count`]. Dashboard stop
//! and IDR reach a native session through [`stop_all`] and [`force_idr_all`].

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::encode::Codec;

/// One live native session. The Arcs are the video loop's own handles, so a
/// mid-stream mode/bitrate change shows on `/status` with no second write.
struct LiveSession {
    id: u64,
    /// Packed `w:16|h:16|hz:16` ([`crate::native::pack_mode`]); live on a mode switch.
    mode: Arc<AtomicU64>,
    /// Encoder target, kbps. Same Arc the ABR path writes.
    bitrate_kbps: Arc<AtomicU32>,
    codec: Codec,
    /// Teardown flag ([`stop_all`]).
    stop: Arc<AtomicBool>,
    /// Deliberate-stop flag ([`stop_all_quit`]). Distinct from `stop`: intended
    /// teardown skips display keep-alive linger and trips end-game-on-session-end.
    quit: Arc<AtomicBool>,
    /// One-shot force-keyframe ([`force_idr_all`]). The encode loop drains it
    /// alongside a client decode-recovery request.
    force_idr: Arc<AtomicBool>,
    /// Client label: 12-hex cert-fingerprint prefix, or peer IP if anonymous.
    client: String,
    /// Display name (trust-store, else sanitized Hello). `None` if nameless.
    client_name: Option<String>,
    hdr: bool,
    /// Bring-up total (hello → first packet), ms. 0 until the first packet left.
    ttff_ms: Arc<AtomicU32>,
    /// Last mid-stream resize (reconfigure → rebuilt), ms. 0 = none yet.
    last_resize_ms: Arc<AtomicU32>,
    /// Launched title's lease, if any — what [`games`] reports for this session.
    game: Option<Arc<crate::gamelease::LeaseShared>>,
    /// The capturer's live health, published by the video loop (WP18). `None` until the
    /// first publish, or on a capturer that does not classify.
    capture_health: Arc<Mutex<Option<pf_capture::CaptureHealth>>>,
    /// Admitted by `mode_conflict: join`: shares the owner's display and audio.
    joined: bool,
    /// The audio thread's mute, read per frame.
    mute: Arc<SessionMute>,
}

/// Why a session's audio is silent. The operator's switch and a title's
/// `audio.sessions` policy are separate bits, so a lease ending lifts only its
/// own and never an operator's mute.
#[derive(Default)]
pub struct SessionMute {
    operator: AtomicBool,
    policy: AtomicBool,
}

impl SessionMute {
    pub fn is_muted(&self) -> bool {
        self.operator.load(Ordering::Relaxed) || self.policy.load(Ordering::Relaxed)
    }
}

/// Which sessions hear a title's audio (`audio.sessions` on a custom entry).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AudioSessions {
    #[default]
    All,
    /// The session that owns the display; joiners are silent.
    Owner,
    /// The sessions that joined the display; the owner is silent.
    Joined,
    /// Only the session that launched the title.
    Launcher,
}

/// Whether `policy` silences a session. `launcher` is the launching session's
/// client label, matched the way [`stop_by_fingerprint`] matches.
fn policy_mutes(policy: AudioSessions, launcher: &str, client: &str, joined: bool) -> bool {
    match policy {
        AudioSessions::All => false,
        AudioSessions::Owner => joined,
        AudioSessions::Joined => !joined,
        AudioSessions::Launcher => client != launcher,
    }
}

/// The live title policy, applied to sessions that register while it stands.
/// One at a time: a second lease replaces it, and the first to end lifts both.
static AUDIO_POLICY: Mutex<Option<(AudioSessions, String)>> = Mutex::new(None);

/// Resolved read of one live session for `/status`.
#[derive(Clone)]
pub struct SessionSnapshot {
    /// Registry id: the `{id}` of the per-session routes.
    pub id: u64,
    /// Client label: 12-hex cert-fingerprint prefix, or peer IP if anonymous.
    pub client: String,
    pub joined: bool,
    pub muted: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: Codec,
    /// Display name (trust-store, else sanitized Hello). `None` if nameless.
    pub client_name: Option<String>,
    /// The capturer's live health, if it classifies.
    pub capture_health: Option<pf_capture::CaptureHealth>,
    /// Bring-up total (hello → first packet), ms. 0 while still bringing up.
    pub time_to_first_frame_ms: u32,
    /// Last mid-stream resize total, ms. 0 = no resize this session.
    pub last_resize_ms: u32,
}

/// Serializes tests that touch the process-global registry; otherwise one
/// test's session leaks into another's `/status`.
#[cfg(test)]
pub(crate) static SESSION_REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn registry() -> &'static Mutex<Vec<LiveSession>> {
    static REG: OnceLock<Mutex<Vec<LiveSession>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Vec::new()))
}

fn next_id() -> u64 {
    static ID: AtomicU64 = AtomicU64::new(1);
    ID.fetch_add(1, Ordering::Relaxed)
}

/// [`crate::events::SessionRef`] for this session; mode is read live.
fn session_ref(s: &LiveSession) -> crate::events::SessionRef {
    let (width, height, fps) = crate::native::unpack_mode(s.mode.load(Ordering::Relaxed));
    crate::events::SessionRef {
        id: s.id,
        client: s.client.clone(),
        mode: crate::events::mode_str(width, height, fps),
        hdr: s.hdr,
    }
}

/// Inputs for [`register`]. Named fields: half are same-typed `Arc<Atomic…>`
/// handles, so a transposed pair would compile and report the wrong figure.
pub struct Registration {
    /// Packed `w:16|h:16|hz:16` ([`crate::native::pack_mode`]); live on a mode switch.
    pub mode: Arc<AtomicU64>,
    /// Encoder target, kbps. Same Arc the ABR path writes.
    pub bitrate_kbps: Arc<AtomicU32>,
    pub codec: Codec,
    /// Teardown flag ([`stop_all`]).
    pub stop: Arc<AtomicBool>,
    /// Deliberate-stop flag ([`stop_all_quit`]). Distinct from `stop`.
    pub quit: Arc<AtomicBool>,
    /// One-shot force-keyframe ([`force_idr_all`]).
    pub force_idr: Arc<AtomicBool>,
    /// Client label: 12-hex cert-fingerprint prefix, or peer IP if anonymous.
    pub client: String,
    /// Display name (trust-store, else sanitized Hello). `None` if nameless.
    pub client_name: Option<String>,
    pub hdr: bool,
    /// Bring-up total slot (hello → first packet), ms. 0 until first packet.
    pub ttff_ms: Arc<AtomicU32>,
    /// Last mid-stream resize total, ms. 0 = none yet.
    pub last_resize_ms: Arc<AtomicU32>,
    /// Launched title's lease, if this session launched one.
    pub game: Option<Arc<crate::gamelease::LeaseShared>>,
    /// The video loop's capture-health slot; it stores the capturer's report on its own cadence.
    pub capture_health: Arc<Mutex<Option<pf_capture::CaptureHealth>>>,
    /// Admitted by `mode_conflict: join`.
    pub joined: bool,
    /// The audio thread's mute; the same Arc it reads per frame.
    pub mute: Arc<SessionMute>,
}

/// Publish a live native session. The guard removes it on drop and pairs
/// `session.started` with `session.ended` on every exit path, including panic.
/// A standing title policy applies to the new row at once.
pub fn register(reg: Registration) -> LiveSessionGuard {
    let Registration {
        mode,
        bitrate_kbps,
        codec,
        stop,
        quit,
        force_idr,
        client,
        client_name,
        hdr,
        ttff_ms,
        last_resize_ms,
        game,
        capture_health,
        joined,
        mute,
    } = reg;
    let id = next_id();
    if let Some((policy, launcher)) = AUDIO_POLICY.lock().unwrap().as_ref() {
        mute.policy.store(
            policy_mutes(*policy, launcher, &client, joined),
            Ordering::Relaxed,
        );
    }
    let session = LiveSession {
        id,
        mode,
        bitrate_kbps,
        codec,
        stop,
        quit,
        force_idr,
        client,
        client_name,
        hdr,
        ttff_ms,
        last_resize_ms,
        game,
        capture_health,
        joined,
        mute,
    };
    crate::events::emit(crate::events::EventKind::SessionStarted {
        session: session_ref(&session),
    });
    registry().lock().unwrap().push(session);
    LiveSessionGuard {
        id,
        _sleep: crate::sleep_inhibit::hold(),
    }
}

/// Drops the registry entry for this session (any video-loop scope exit).
pub struct LiveSessionGuard {
    /// Same id `/status` reports; the video loop's log span names it.
    pub(crate) id: u64,
    /// Sleep inhibit for the session lifetime: a passive viewer must not let
    /// the box auto-suspend ([`crate::sleep_inhibit`]).
    _sleep: crate::sleep_inhibit::StreamHold,
}

impl Drop for LiveSessionGuard {
    fn drop(&mut self) {
        let mut reg = registry().lock().unwrap();
        if let Some(pos) = reg.iter().position(|s| s.id == self.id) {
            let session = reg.remove(pos);
            drop(reg); // emit outside the registry lock; the bus takes its own
            crate::events::emit(crate::events::EventKind::SessionEnded {
                session: session_ref(&session),
            });
        }
    }
}

pub fn count() -> usize {
    registry().lock().unwrap().len()
}

/// Snapshot of every live native session; mode/bitrate read live. Newest last.
pub fn snapshot() -> Vec<SessionSnapshot> {
    registry()
        .lock()
        .unwrap()
        .iter()
        .map(|s| {
            let (width, height, fps) = crate::native::unpack_mode(s.mode.load(Ordering::Relaxed));
            SessionSnapshot {
                id: s.id,
                client: s.client.clone(),
                joined: s.joined,
                muted: s.mute.is_muted(),
                width,
                height,
                fps,
                bitrate_kbps: s.bitrate_kbps.load(Ordering::Relaxed),
                codec: s.codec,
                client_name: s.client_name.clone(),
                capture_health: s
                    .capture_health
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
                time_to_first_frame_ms: s.ttff_ms.load(Ordering::Relaxed),
                last_resize_ms: s.last_resize_ms.load(Ordering::Relaxed),
            }
        })
        .collect()
}

/// One launched game as `/status` reports it.
pub struct GameSnapshot {
    /// Streaming session, or `None` if the session is gone and the game is in
    /// its reconnect window.
    pub session_id: Option<u64>,
    pub client: String,
    pub app_id: Option<String>,
    pub title: String,
    pub store: Option<String>,
    pub plane: crate::events::Plane,
    /// `launching` / `running` / `exited` / `untracked`, or `grace` on the
    /// reconnect window.
    pub state: &'static str,
    /// Seconds left before the game is ended. Set only on a `grace` row.
    pub grace_remaining_s: Option<u64>,
}

/// Compat plane's launched game, while it has one.
///
/// GameStream is not in the native registry: that holds the loop's `Arc`
/// handles, which the compat plane does not have. One `AppState.launch`
/// means one slot; this is what keeps a Moonlight game on `/status`
/// alongside a native session.
fn gs_game() -> &'static Mutex<Option<Arc<crate::gamelease::LeaseShared>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<crate::gamelease::LeaseShared>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Publish the compat plane's game. The guard retracts it on any stream-loop
/// exit ([`LiveSessionGuard`]'s counterpart).
pub fn publish_gamestream_game(shared: Arc<crate::gamelease::LeaseShared>) -> GamestreamGameGuard {
    *gs_game().lock().unwrap_or_else(|e| e.into_inner()) = Some(shared);
    GamestreamGameGuard
}

/// Retracts the compat plane's published game on drop.
pub struct GamestreamGameGuard;

impl Drop for GamestreamGameGuard {
    fn drop(&mut self) {
        *gs_game().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Every launched game the host currently knows: live sessions first, then
/// the compat plane, then games waiting out a reconnect window.
///
/// Sources stay separate — a grace-pending game has no session to hang
/// off, and omitting it would hide "the host is about to close this game".
pub fn games() -> Vec<GameSnapshot> {
    let mut out: Vec<GameSnapshot> = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|s| {
            let g = s.game.as_ref()?;
            Some(GameSnapshot {
                session_id: Some(s.id),
                client: g.client.clone(),
                app_id: g.game.id.clone(),
                title: g.game.title.clone(),
                store: g.game.store.clone(),
                plane: g.plane,
                state: g.state().as_str(),
                grace_remaining_s: None,
            })
        })
        .collect();
    out.extend(
        gs_game()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|g| GameSnapshot {
                // Compat plane has no session id. State is never `grace` while
                // streaming, so the console tells this from a grace row by state.
                session_id: None,
                client: g.client.clone(),
                app_id: g.game.id.clone(),
                title: g.game.title.clone(),
                store: g.game.store.clone(),
                plane: g.plane,
                state: g.state().as_str(),
                grace_remaining_s: None,
            }),
    );
    out.extend(
        crate::gamelease::pending_snapshot()
            .into_iter()
            .map(|(g, remaining)| GameSnapshot {
                session_id: None,
                client: g.client.clone(),
                app_id: g.game.id.clone(),
                title: g.game.title.clone(),
                store: g.game.store.clone(),
                plane: g.plane,
                state: "grace",
                grace_remaining_s: Some(remaining),
            }),
    );
    out
}

/// Tear down every live native session. Best-effort: loops observe the
/// flag and exit; the guard then clears the entry. Not intended teardown —
/// prefer [`stop_all_quit`] for an operator action.
pub fn stop_all() {
    for s in registry().lock().unwrap().iter() {
        s.stop.store(true, Ordering::SeqCst);
    }
}

/// Tear down live native sessions for `fp_hex` (lowercase hex cert SHA-256)
/// deliberately — unpair must not leave a mid-stream client streaming.
///
/// Match is the registry client label: the fingerprint's 12-hex prefix.
/// An anonymous/TOFU session carries an IP label and never matches.
/// Returns how many sessions were signalled.
pub fn stop_by_fingerprint(fp_hex: &str) -> usize {
    let mut n = 0;
    for s in registry().lock().unwrap().iter() {
        if s.client.len() == 12 && fp_hex.starts_with(s.client.as_str()) {
            s.quit.store(true, Ordering::SeqCst);
            s.stop.store(true, Ordering::SeqCst);
            n += 1;
        }
    }
    n
}

/// Whether any live native session belongs to a client other than `fp_hex`.
///
/// Busy check for host-power: a granted guest must not power off the host
/// while someone else is streaming. Label match as in [`stop_by_fingerprint`].
/// An anonymous IP-labelled session always counts as another client.
pub fn other_client_live(fp_hex: &str) -> bool {
    registry()
        .lock()
        .unwrap()
        .iter()
        .any(|s| !(s.client.len() == 12 && fp_hex.starts_with(s.client.as_str())))
}

/// Tear down every live native session deliberately (mgmt `DELETE /session`).
///
/// Sets `quit` before `stop` so teardown matches a client's own Stop: the
/// display skips keep-alive linger and end-game-on-session-end sees intent.
pub fn stop_all_quit() {
    for s in registry().lock().unwrap().iter() {
        s.quit.store(true, Ordering::SeqCst);
        s.stop.store(true, Ordering::SeqCst);
    }
}

/// Force a keyframe on every live native session (`POST /session/idr`).
/// The encode loop drains the flag like a client decode-recovery request.
pub fn force_idr_all() {
    for s in registry().lock().unwrap().iter() {
        s.force_idr.store(true, Ordering::Relaxed);
    }
}

/// The operator's mute for one session (`PUT /session/{id}/audio`).
/// `false` = no live session has that id.
pub fn set_muted(id: u64, muted: bool) -> bool {
    let reg = registry().lock().unwrap();
    let Some(s) = reg.iter().find(|s| s.id == id) else {
        return false;
    };
    s.mute.operator.store(muted, Ordering::Relaxed);
    true
}

/// Apply a title's `audio.sessions` to every live session, and to each one
/// that registers while the guard lives. Drop lifts the policy's mutes only.
pub fn apply_audio_policy(policy: AudioSessions, launcher: &str) -> AudioPolicyGuard {
    *AUDIO_POLICY.lock().unwrap() = Some((policy, launcher.to_owned()));
    for s in registry().lock().unwrap().iter() {
        s.mute.policy.store(
            policy_mutes(policy, launcher, &s.client, s.joined),
            Ordering::Relaxed,
        );
    }
    tracing::info!(policy = ?policy, launcher, "title audio policy applied");
    AudioPolicyGuard(())
}

pub struct AudioPolicyGuard(());

impl Drop for AudioPolicyGuard {
    fn drop(&mut self) {
        *AUDIO_POLICY.lock().unwrap() = None;
        for s in registry().lock().unwrap().iter() {
            s.mute.policy.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_session(client: &str) -> (LiveSessionGuard, Arc<AtomicBool>, Arc<AtomicBool>) {
        fake_session_joined(client, false).0
    }

    fn fake_session_joined(
        client: &str,
        joined: bool,
    ) -> (
        (LiveSessionGuard, Arc<AtomicBool>, Arc<AtomicBool>),
        Arc<SessionMute>,
    ) {
        let stop = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));
        let mute = Arc::new(SessionMute::default());
        let guard = register(Registration {
            mode: Arc::new(AtomicU64::new(0)),
            bitrate_kbps: Arc::new(AtomicU32::new(20_000)),
            codec: Codec::H265,
            stop: stop.clone(),
            quit: quit.clone(),
            force_idr: Arc::new(AtomicBool::new(false)),
            client: client.into(),
            client_name: None,
            hdr: false,
            ttff_ms: Arc::new(AtomicU32::new(0)),
            last_resize_ms: Arc::new(AtomicU32::new(0)),
            game: None,
            capture_health: Arc::new(Mutex::new(None)),
            joined,
            mute: mute.clone(),
        });
        ((guard, stop, quit), mute)
    }

    #[test]
    fn policy_picks_the_sessions_it_names() {
        use AudioSessions::*;
        // (policy, client, joined) → muted
        for (policy, client, joined, want) in [
            (All, "aaaaaaaaaaaa", true, false),
            (Owner, "aaaaaaaaaaaa", false, false),
            (Owner, "bbbbbbbbbbbb", true, true),
            (Joined, "aaaaaaaaaaaa", false, true),
            (Joined, "bbbbbbbbbbbb", true, false),
            (Launcher, "aaaaaaaaaaaa", false, false),
            (Launcher, "bbbbbbbbbbbb", true, true),
        ] {
            assert_eq!(
                policy_mutes(policy, "aaaaaaaaaaaa", client, joined),
                want,
                "{policy:?} {client} joined={joined}"
            );
        }
    }

    /// A policy mutes by role, reaches a session that registers later, and its
    /// end lifts only what it set: the operator's mute stays.
    #[test]
    fn policy_and_operator_mutes_are_separate_bits() {
        let _serial = SESSION_REGISTRY_LOCK.blocking_lock();
        let (_owner, owner_mute) = fake_session_joined("aaaaaaaaaaaa", false);
        let guard = apply_audio_policy(AudioSessions::Owner, "aaaaaaaaaaaa");
        let ((_joiner, ..), joiner_mute) = fake_session_joined("bbbbbbbbbbbb", true);
        assert!(!owner_mute.is_muted());
        assert!(
            joiner_mute.is_muted(),
            "a joiner registered under the policy is muted"
        );
        let owner_id = snapshot()
            .iter()
            .find(|s| s.client == "aaaaaaaaaaaa")
            .unwrap()
            .id;
        assert!(set_muted(owner_id, true));
        assert!(!set_muted(owner_id + 1_000_000, true));
        drop(guard);
        assert!(
            !joiner_mute.is_muted(),
            "the lease ending lifts the policy mute"
        );
        assert!(
            owner_mute.is_muted(),
            "the operator's mute outlives the policy"
        );
        assert!(snapshot().iter().any(|s| s.id == owner_id && s.muted));
    }

    /// Unpair revokes a live session by the 12-hex fingerprint prefix
    /// (`quit` + `stop`). Other clients and IP-labelled sessions stay up.
    #[test]
    fn stop_by_fingerprint_revokes_exactly_the_unpaired_client() {
        let fp = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
        let (_g1, stop1, quit1) = fake_session(&fp[..12]);
        let (_g2, stop2, _q2) = fake_session("112233445566"); // a different paired client
        let (_g3, stop3, _q3) = fake_session("192.168.1.50"); // anonymous: IP label, never matches

        assert_eq!(stop_by_fingerprint(fp), 1);
        assert!(stop1.load(Ordering::SeqCst) && quit1.load(Ordering::SeqCst));
        assert!(!stop2.load(Ordering::SeqCst));
        assert!(!stop3.load(Ordering::SeqCst));
    }

    /// A Moonlight game has no live-session entry, so it is only on `/status`
    /// while [`publish_gamestream_game`]'s guard is alive.
    #[test]
    fn a_gamestream_game_is_visible_only_while_its_stream_runs() {
        let id = "steam:1701";
        let mine = || {
            games()
                .into_iter()
                .find(|g| g.app_id.as_deref() == Some(id))
        };

        let lease = crate::gamelease::open(
            crate::gamelease::LeaseRequest {
                game: crate::gamelease::GameRef {
                    id: Some(id.to_string()),
                    store: Some("steam".into()),
                    title: "Test Title".into(),
                },
                client: "192.0.2.7".into(),
                plane: crate::events::Plane::Gamestream,
                // No signals: inert lease, so no watcher thread races the assertions.
                spec: crate::library::DetectSpec::default(),
                nested: false,
                launcher: false,
                child: None,
                spawned: None,
                launch_stamp: None,
                procs: None,
            },
            Box::new(|| {}),
        );
        assert!(mine().is_none(), "not published yet");

        {
            let _pub = publish_gamestream_game(lease.shared());
            let row = mine().expect("the compat plane's game is reported");
            assert_eq!(
                row.session_id, None,
                "no live-session entry to attribute it to"
            );
            assert_eq!(row.plane, crate::events::Plane::Gamestream);
            assert_eq!(row.client, "192.0.2.7");
            assert_eq!(row.title, "Test Title");
            // Not `grace` while the stream is up: the console keys
            // countdown / End now off that state.
            assert_ne!(row.state, "grace");
        }
        assert!(mine().is_none(), "the row goes with the stream");
    }
}
