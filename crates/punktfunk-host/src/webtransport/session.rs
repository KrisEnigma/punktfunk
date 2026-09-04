//! One browser session: the punktfunk/1 handshake on a WebTransport stream, then video.
//!
//! **This is Phase 2's path, not the native one.** `native::serve_session` is built around a
//! `quinn::Connection` — audio, cursor, rumble and HID all call `conn.send_datagram` on it
//! directly — so reusing it would mean abstracting that connection first. Video is the half that
//! is already transport-agnostic ([`Session`] holds a `Box<dyn Transport>`), and video is what
//! "first frame on screen" needs, so this is deliberately the video half and says so.
//!
//! The frame source is synthetic and the encoder is openh264. That is not a placeholder for
//! testing's sake: it is what lets a headless box with no GPU serve a browser, which is what the
//! browser tier is being proven against.
//!
//! **Identity, with no client certificate.** The browser speaks first — a stream it opens does
//! not reach the host until it writes on it, so a host that greeted first would wait for a client
//! that was waiting for it. A first-time browser sends a `PairRequest` carrying its WebCrypto
//! device key; any other opens with `Hello`, and a host that requires pairing answers that with
//! an [`AuthChallenge`] rather than a `Welcome`. The signature that comes back is looked up in
//! the same store native clients live in. What mTLS does per packet, this does once — see
//! `design/web-client-implementation-plan.md` Phase 3 for the honest comparison.

use super::{Inbox, Serving, WebTransportPlane};
use anyhow::{Context, Result};
use punktfunk_core::config::{CompositorPref, FecConfig, FecScheme, GamepadPref, Role};
use punktfunk_core::quic::io::{read_msg, write_msg};
use punktfunk_core::quic::ColorInfo;
use punktfunk_core::quic::{
    auth_signed_message, AuthChallenge, AuthResponse, Hello, PairRequest, Start, Welcome,
};
use punktfunk_core::session::Session;
use rand::RngCore;
use std::sync::Arc;
use wtransport::Connection;

/// Read the handshake, then stream until the browser goes away.
pub(crate) async fn run(conn: Connection, inbox: Arc<Inbox>, serving: Arc<Serving>) -> Result<()> {
    let (mut tx, mut rx) = conn.accept_bi().await.context("accept control stream")?;
    let first = read_msg(&mut rx).await.context("read the first message")?;

    // A `PairRequest` ends the session either way — pairing is its own connection, as on the
    // native plane, so a browser reconnects to stream.
    if let Ok(req) = PairRequest::decode(&first) {
        return pair(&conn, tx, rx, req, &serving).await;
    }
    let hello = Hello::decode(&first).map_err(|e| anyhow::anyhow!("bad Hello: {e:?}"))?;

    // Nothing is offered until the device answers. The nonce is fresh per connection, so a
    // captured response does not open a second one.
    if serving.plane.require_pairing {
        let mut nonce = [0u8; 32];
        rand::rng().fill_bytes(&mut nonce);
        write_msg(&mut tx, &AuthChallenge { nonce }.encode())
            .await
            .context("write AuthChallenge")?;
        let answer = read_msg(&mut rx).await.context("read AuthResponse")?;
        let auth = AuthResponse::decode(&answer)
            .map_err(|_| anyhow::anyhow!("this host requires pairing — no device signature"))?;
        let name = admit(&auth, &nonce, &serving)?;
        tracing::info!(device = %name, "browser authenticated");
    }
    tracing::info!(
        width = hello.mode.width,
        height = hello.mode.height,
        fps = hello.mode.refresh_hz,
        name = hello.name.as_deref().unwrap_or("<unnamed>"),
        "browser Hello"
    );

    let welcome = offer(&conn, &hello);
    write_msg(&mut tx, &welcome.encode())
        .await
        .context("write Welcome")?;

    // `Start` carries a UDP port on the native plane; a browser has no second plane, so the
    // message is only a "begin" marker here.
    let start_bytes = read_msg(&mut rx).await.context("read Start")?;
    Start::decode(&start_bytes).map_err(|e| anyhow::anyhow!("bad Start: {e:?}"))?;

    let cfg = welcome.session_config(Role::Host);
    let session = Session::new(cfg, Box::new(WebTransportPlane::new(conn.clone(), inbox)))
        .map_err(|e| anyhow::anyhow!("host session: {e:?}"))?;
    tracing::info!(
        codec = welcome.codec,
        shard_payload = welcome.shard_payload,
        "browser session streaming"
    );

    // The encoder and the pump are synchronous; give them a thread rather than block the runtime.
    let mode = welcome.mode;
    let bitrate_kbps = welcome.bitrate_kbps;
    let streamed = tokio::task::spawn_blocking(move || stream(session, mode, bitrate_kbps)).await;
    match streamed {
        Ok(r) => r,
        Err(e) => Err(anyhow::anyhow!("stream thread: {e}")),
    }
}

/// Is this device one the host paired with, and does it still hold the key?
///
/// Both halves matter and neither is enough. A signature by an unpaired key proves possession of
/// something the host never trusted; a fingerprint in the store with no signature is a public
/// value anyone can replay. Returns the stored device name, for the log.
fn admit(auth: &AuthResponse, nonce: &[u8; 32], serving: &Serving) -> Result<String> {
    let fp = sha256(&auth.device_key);
    let hex: String = fp.iter().map(|b| format!("{b:02x}")).collect();
    let paired = serving
        .plane
        .pairing
        .list()
        .into_iter()
        .find(|c| c.fingerprint == hex)
        .context("this device is not paired with the host")?;
    let msg = auth_signed_message(&serving.cert_hash, nonce);
    aws_lc_rs::signature::UnparsedPublicKey::new(
        &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1,
        spki_p256_point(&auth.device_key).context("device key is not a P-256 SPKI")?,
    )
    .verify(&msg, &auth.signature)
    .map_err(|_| anyhow::anyhow!("the device signature does not verify"))?;
    Ok(paired.name)
}

/// The 65-byte uncompressed point inside a P-256 SPKI.
///
/// Every P-256 SPKI starts with the same 26-byte header — the SEQUENCE, the two OIDs and the BIT
/// STRING tag are all fixed by the key type — so matching it whole both locates the point and
/// rejects any other key type, which is what we want: the verifier is P-256 only.
fn spki_p256_point(spki: &[u8]) -> Option<&[u8]> {
    const P256_SPKI_HEADER: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    let (head, point) = spki.split_at_checked(P256_SPKI_HEADER.len())?;
    (head == P256_SPKI_HEADER && point.len() == 65 && point[0] == 0x04).then_some(point)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

/// Pair a browser, then end the connection.
///
/// The identities are what makes this the native ceremony rather than a second one: the client's
/// is the SHA-256 of the device key it just sent, the host's is the hash of the certificate this
/// endpoint presented — which the browser had to pin to connect at all, so a man in the middle
/// cannot hold it without the host's private key.
async fn pair(
    conn: &Connection,
    tx: wtransport::SendStream,
    rx: wtransport::RecvStream,
    req: PairRequest,
    serving: &Serving,
) -> Result<()> {
    anyhow::ensure!(
        spki_p256_point(&req.device_key).is_some(),
        "a browser must pair with a P-256 device key"
    );
    // Charged before arming is consulted, on every outcome: otherwise this plane answers "is
    // pairing armed?" for free, to anyone. Knocks can hold it against the real device, which is
    // the trade the native plane already makes.
    {
        let mut last = serving.last_pairing.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < crate::native::PAIRING_COOLDOWN) {
            anyhow::bail!("pairing rate-limited — retry shortly");
        }
        *last = Some(std::time::Instant::now());
    }
    let client_fp = sha256(&req.device_key);
    let pin = match serving.plane.pairing.pin_for_attempt(
        &client_fp
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    ) {
        crate::native_pairing::PinAttempt::Pin(pin) => pin,
        crate::native_pairing::PinAttempt::Disarmed => {
            anyhow::bail!("pairing is not armed — arm it in the console, then retry")
        }
        crate::native_pairing::PinAttempt::BoundToOther => {
            anyhow::bail!("pairing is armed for a different device")
        }
    };
    crate::native::pair_ceremony(
        &crate::native::link::SessionLink::Web(conn.clone()),
        tx,
        rx,
        req,
        &client_fp,
        &serving.cert_hash,
        &serving.plane.pairing,
        &pin,
    )
    .await
}

/// What this host will serve a browser. Deliberately narrow: no host capability bits, because
/// the features they gate (cursor forwarding, clipboard, pen, audio redundancy) all ride the
/// quinn connection this session does not have.
fn offer(conn: &Connection, hello: &Hello) -> Welcome {
    let mut key = [0u8; 16];
    rand::rng().fill_bytes(&mut key);
    let mut salt = [0u8; 4];
    rand::rng().fill_bytes(&mut salt);

    // A WebTransport datagram is smaller than the ~1408 B a 1500-MTU UDP path allows — the QUIC
    // and HTTP/3 framing come out of the same budget. Ask the connection rather than assume, and
    // fall back to the conservative 1200 every QUIC path guarantees.
    let budget = conn.max_datagram_size().unwrap_or(1200);
    let shard_payload =
        punktfunk_core::config::shard_payload_for_udp_budget(budget, conn.remote_address().ip());

    Welcome {
        abi_version: punktfunk_core::WIRE_VERSION,
        // No second plane to reach: everything rides the one WebTransport session.
        udp_port: 0,
        mode: hello.mode,
        fec: FecConfig {
            scheme: FecScheme::Gf16,
            fec_percent: 20,
            max_data_per_block: 4096,
        },
        shard_payload: shard_payload as u16,
        encrypt: true,
        key,
        salt,
        // Unbounded: the browser streams until it closes.
        frames: 0,
        compositor: CompositorPref::Auto,
        gamepad: GamepadPref::Auto,
        bitrate_kbps: if hello.bitrate_kbps == 0 {
            20_000
        } else {
            hello.bitrate_kbps
        },
        bit_depth: 8,
        color: ColorInfo::SDR_BT709,
        chroma_format: punktfunk_core::quic::CHROMA_IDC_420,
        audio_channels: 0,
        codec: punktfunk_core::quic::CODEC_H264,
        host_caps: 0,
        host_caps2: 0,
        cipher: punktfunk_core::quic::CIPHER_AES_128_GCM,
        key_chacha: None,
        audio_codec: 0,
        audio_rate_hz: 0,
        audio_bits: 0,
        audio_frame_us: 0,
        mgmt_port: 0,
        grants: 0,
        expires_in_secs: 0,
    }
}

/// Generate, encode and submit until the session ends.
///
/// Linux only, because the software encoder is: Windows has no GPU-less encode path at all. A
/// browser reaching a host that cannot software-encode gets a refusal it can read, not a hang.
#[cfg(target_os = "linux")]
fn stream(
    mut session: Session,
    mode: punktfunk_core::config::Mode,
    bitrate_kbps: u32,
) -> Result<()> {
    use pf_capture::Capturer;
    use punktfunk_core::packet::{FLAG_PIC, FLAG_SOF};

    let (w, h, fps) = (mode.width, mode.height, mode.refresh_hz.max(1));
    // The house synthetic source: a sweeping bar over an animated gradient, every pixel changing,
    // already `Bgrx` + a CPU payload, which is exactly what openh264 wants with no conversion.
    let mut capturer = pf_capture::SyntheticCapturer::new(w, h, fps);
    let mut frame = capturer.next_frame().context("first synthetic frame")?;
    // Named, not resolved from the ladder: `auto` never picks software, and the environment that
    // would have said so is latched long before a browser connects.
    let mut encoder =
        pf_encode::open_software_h264(frame.format, w, h, fps, u64::from(bitrate_kbps) * 1000)
            .context("open the software encoder")?;

    let interval = std::time::Duration::from_nanos(1_000_000_000 / u64::from(fps));
    loop {
        encoder.submit(&frame).context("encode submit")?;
        while let Some(au) = encoder.poll().context("encode poll")? {
            // The reassembler needs the picture and start-of-frame markers to find AU
            // boundaries; a keyframe is also where a joining decoder can start.
            let mut flags = u32::from(FLAG_PIC);
            if au.keyframe {
                flags |= u32::from(FLAG_SOF);
            }
            if session.submit_frame(&au.data, au.pts_ns, flags).is_err() {
                // The peer is gone, or the transport refused — either way this session is over.
                return Ok(());
            }
        }
        std::thread::sleep(interval);
        frame = capturer.next_frame().context("synthetic frame")?;
    }
}

#[cfg(not(target_os = "linux"))]
fn stream(
    _session: Session,
    _mode: punktfunk_core::config::Mode,
    _bitrate_kbps: u32,
) -> Result<()> {
    anyhow::bail!(
        "the browser plane's synthetic source needs the software encoder, which is Linux-only"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use punktfunk_core::quic::auth_signed_message;
    use rcgen::{
        KeyPair, PublicKeyData as _, SigningKey as _, PKCS_ECDSA_P256_SHA256,
        PKCS_ECDSA_P384_SHA384,
    };

    fn store(tag: &str) -> Arc<crate::native_pairing::NativePairing> {
        let path =
            std::env::temp_dir().join(format!("pf-wt-auth-{tag}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Arc::new(crate::native_pairing::NativePairing::load_with(Some(path), None, false).unwrap())
    }

    fn serving(pairing: Arc<crate::native_pairing::NativePairing>) -> Serving {
        Serving {
            plane: crate::webtransport::Plane {
                bind: "127.0.0.1:9778".parse().unwrap(),
                sans: Vec::new(),
                origins: Vec::new(),
                identity: crate::identity::ephemeral().unwrap(),
                pairing,
                require_pairing: true,
            },
            cert_hash: [0x11; 32],
            last_pairing: std::sync::Mutex::new(None),
        }
    }

    fn respond(key: &KeyPair, binding: &[u8; 32], nonce: &[u8; 32]) -> AuthResponse {
        let device_key = key.subject_public_key_info();
        let signature = key.sign(&auth_signed_message(binding, nonce)).unwrap();
        AuthResponse {
            device_key,
            signature,
        }
    }

    /// A P-256 SPKI is a fixed shape, so locating the point is exact rather than a guess — and
    /// anything that is not one has to be refused, not misread.
    #[test]
    fn only_a_p256_spki_yields_a_key() {
        let p256 = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let spki = p256.subject_public_key_info();
        let point = spki_p256_point(&spki).expect("a P-256 SPKI has a point");
        assert_eq!(point.len(), 65);
        assert_eq!(point[0], 0x04, "uncompressed");

        let p384 = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
        assert!(spki_p256_point(&p384.subject_public_key_info()).is_none());
        assert!(spki_p256_point(&[]).is_none());
        assert!(
            spki_p256_point(&spki[..spki.len() - 1]).is_none(),
            "truncated"
        );
        let mut trailing = spki.clone();
        trailing.push(0);
        assert!(spki_p256_point(&trailing).is_none(), "over-long");
    }

    /// The session credential: paired *and* holding the key, on *this* connection.
    #[test]
    fn admission_needs_a_pairing_a_signature_and_this_channel() {
        let np = store("admit");
        let s = serving(np.clone());
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let nonce = [0x77u8; 32];
        let auth = respond(&key, &s.cert_hash, &nonce);

        // Unpaired: a perfect signature buys nothing.
        assert!(admit(&auth, &nonce, &s).is_err(), "not paired yet");

        let fp: String = sha256(&auth.device_key)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        np.add("Enrico's browser", &fp).unwrap();
        assert_eq!(admit(&auth, &nonce, &s).unwrap(), "Enrico's browser");

        // Paired, but the signature must still be over this nonce on this channel.
        assert!(admit(&auth, &[0x78; 32], &s).is_err(), "replayed nonce");
        let mut elsewhere = serving(np.clone());
        elsewhere.cert_hash = [0x22; 32];
        assert!(
            admit(&auth, &nonce, &elsewhere).is_err(),
            "a response captured on one connection must not open another"
        );

        // A different key that claims the paired fingerprint cannot: the fingerprint IS the key.
        let other = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        assert!(admit(&respond(&other, &s.cert_hash, &nonce), &nonce, &s).is_err());

        // And the paired key with a mangled signature does not slip through.
        let mut bent = auth.clone();
        let last = bent.signature.len() - 1;
        bent.signature[last] ^= 0xff;
        assert!(admit(&bent, &nonce, &s).is_err());
    }
}
