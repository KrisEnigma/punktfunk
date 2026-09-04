//! The session pump on wasm (`design/web-client-implementation-plan.md` WP2.2).
//!
//! This is the plan's claim tested: **unchanged code, new target.** FEC, decrypt and reassembly
//! are `punktfunk_core::session::Session`, the same one the desktop client runs, over the
//! [`WebTransportDatagrams`](crate::transport::WebTransportDatagrams) ring instead of a UDP
//! socket. Nothing here reimplements the protocol.
//!
//! What is new is the control plane's carrier. The native client runs the handshake on a quinn
//! stream; a browser has a WebTransport stream, so the bytes are the same and the I/O is not.
//! JavaScript owns the stream, Rust owns the codec: [`pf_ctl_recv`] takes what arrived and
//! `pf_wt_ctl_send` hands back what to write.
//!
//! R3 holds throughout. An access unit leaves as a `(ptr, len)` view for `VideoDecoder`; no
//! decoded pixel ever enters this heap.

use crate::credential;
use crate::transport::WebTransportDatagrams;
use punktfunk_core::config::{CompositorPref, GamepadPref, Mode, Role};
use punktfunk_core::quic::{
    AuthChallenge, Hello, PairChallenge, PairResult, Start, Welcome, MAGIC,
};
use punktfunk_core::session::Session;
use std::cell::RefCell;

/// How far the handshake has got. A browser cannot block, so the client is a state machine the
/// page steps rather than an `async fn`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    /// `Hello` written, waiting for the host's `Welcome` — or, on a host that requires pairing,
    /// for the `AuthChallenge` it sends first.
    Offered,
    /// Streaming: `Session` is up and `poll_frame` yields access units.
    Live,
    Failed,
}

struct Client {
    phase: Phase,
    /// Control-stream bytes JavaScript has delivered but that do not yet form a whole message.
    inbox: Vec<u8>,
    /// Boxed: `Session` carries the reassembly and replay state inline and is far larger
    /// than emscripten's default stack. Constructing a `Client` that embedded one by value
    /// overflowed before the function body ran, which surfaces as an out-of-bounds trap.
    session: Option<Box<Session>>,
    /// The negotiated video format, for the page's `VideoDecoder.configure`.
    codec: u8,
    width: u32,
    height: u32,
    /// Access units delivered, so the page can tell "connected" from "streaming".
    frames: u64,
}

impl Client {
    fn new() -> Client {
        Client {
            phase: Phase::Idle,
            inbox: Vec::new(),
            session: None,
            codec: 0,
            width: 0,
            height: 0,
            frames: 0,
        }
    }
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::new());
}

unsafe extern "C" {
    /// Write one length-prefixed control message to the WebTransport stream.
    fn pf_wt_ctl_send(ptr: *const u8, len: u32);
    /// Hand one access unit to the page for `VideoDecoder`. Borrowed for the call only — the
    /// page copies what it needs before returning, and no pixel comes back.
    fn pf_video_au(ptr: *const u8, len: u32, pts_us: f64, key: i32);
    /// The negotiated format, once `Welcome` has been read.
    fn pf_video_config(codec: u32, width: u32, height: u32);
}

/// Frame the way the control plane does everywhere else: `u16` length, then the payload.
fn write_msg(body: &[u8]) {
    let mut framed = Vec::with_capacity(body.len() + 2);
    framed.extend_from_slice(&(body.len() as u16).to_le_bytes());
    framed.extend_from_slice(body);
    // SAFETY: `pf_wt_ctl_send` copies `len` bytes out of `ptr` before returning; the buffer
    // outlives the call and nothing is written through the pointer.
    unsafe { pf_wt_ctl_send(framed.as_ptr(), framed.len() as u32) };
}

/// Pull one complete message out of the inbox, if there is one.
fn take_msg(inbox: &mut Vec<u8>) -> Option<Vec<u8>> {
    if inbox.len() < 2 {
        return None;
    }
    let len = u16::from_le_bytes([inbox[0], inbox[1]]) as usize;
    if inbox.len() < 2 + len {
        return None;
    }
    let body = inbox[2..2 + len].to_vec();
    inbox.drain(..2 + len);
    Some(body)
}

/// Open the session: offer a stream of `width` × `height` at `fps`.
///
/// Called once the page's control stream is up. The browser always speaks first — a stream it
/// opened does not reach the host until it writes on it — so `Hello` goes out here even against
/// a host that will demand a credential. Everything after arrives through [`pf_ctl_recv`].
#[unsafe(no_mangle)]
pub extern "C" fn pf_session_hello(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> i32 {
    CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        let hello = Hello {
            abi_version: punktfunk_core::WIRE_VERSION,
            mode: Mode {
                width,
                height,
                refresh_hz: fps,
            },
            compositor: CompositorPref::Auto,
            gamepad: GamepadPref::Auto,
            bitrate_kbps,
            name: Some("Browser".to_string()),
            launch: None,
            // `STREAMED_AU` is deliberately absent: slice-progressive delivery hands over pieces
            // of an access unit, and `VideoDecoder` wants whole ones.
            video_caps: punktfunk_core::quic::VIDEO_CAP_PROBE_SEQ,
            audio_channels: 2,
            // H.264 only for now: it is what a GPU-less host can encode, and what every engine
            // decodes. HEVC and AV1 wait until there is a stream to test them against.
            video_codecs: punktfunk_core::quic::CODEC_H264,
            preferred_codec: punktfunk_core::quic::CODEC_H264,
            display_hdr: None,
            client_caps: 0,
            max_shard_payload: punktfunk_core::config::max_shard_payload() as u16,
            audio_rate_hz: 0,
            audio_bits: 0,
        };
        write_msg(&hello.encode());
        c.phase = Phase::Offered;
        1
    })
}

/// The page has signed the host's nonce. Send the credential; the host answers with `Welcome`.
///
/// # Safety
/// `sig` must point to 64 readable bytes: WebCrypto's raw `r || s` for P-256.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pf_cred_signed(sig: *const u8, len: u32) -> i32 {
    if sig.is_null() || len != 64 {
        return 0;
    }
    // SAFETY: the caller guarantees 64 readable bytes, copied out here and not held.
    let raw: [u8; 64] = unsafe { std::slice::from_raw_parts(sig, 64) }
        .try_into()
        .expect("64 bytes");
    let Some(response) = credential::auth_response(&raw) else {
        return 0;
    };
    write_msg(&response);
    1
}

/// Begin pairing with the PIN the host is showing. Ends this connection either way — the host
/// closes after the ceremony, and the page reconnects to stream.
///
/// # Safety
/// Both pointers must reference their stated number of readable UTF-8 bytes for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pf_pair_begin(
    pin: *const u8,
    pin_len: u32,
    name: *const u8,
    name_len: u32,
) -> i32 {
    if pin.is_null() || name.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees both ranges; both are copied into owned `String`s here.
    let (pin, name) = unsafe {
        (
            String::from_utf8_lossy(std::slice::from_raw_parts(pin, pin_len as usize)).into_owned(),
            String::from_utf8_lossy(std::slice::from_raw_parts(name, name_len as usize))
                .into_owned(),
        )
    };
    let Some(req) = credential::pair_begin(&pin, &name) else {
        return 0;
    };
    write_msg(&req);
    1
}

/// Control-stream bytes from the browser. Copied in, then parsed as messages complete.
///
/// # Safety
/// `ptr` must point to `len` readable bytes for the duration of the call. The page passes a view
/// into wasm memory it just filled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pf_ctl_recv(ptr: *const u8, len: u32) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // SAFETY: the caller guarantees `len` readable bytes at `ptr`; the slice is copied out and
    // not held.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        c.inbox.extend_from_slice(bytes);
        // Take the message out first: `on_welcome` needs the whole client, and the inbox borrow
        // would still be live inside the `while let`.
        loop {
            let Some(body) = take_msg(&mut c.inbox) else {
                break;
            };
            if c.phase == Phase::Offered && body.starts_with(MAGIC) {
                match Welcome::decode(&body) {
                    Ok(welcome) => on_welcome(&mut c, welcome),
                    Err(e) => {
                        println!("punktfunk-web: welcome rejected: {e:?}");
                        c.phase = Phase::Failed;
                    }
                }
                continue;
            }
            // The credential plane. Each decode checks its own magic and type byte, so trying
            // them in turn cannot confuse one message for another.
            if let Ok(ch) = AuthChallenge::decode(&body) {
                // The page signs and calls back into `pf_cred_signed`; nothing to send here.
                credential::on_challenge(&ch.nonce);
            } else if let Ok(ch) = PairChallenge::decode(&body) {
                match credential::on_pair_challenge(&ch) {
                    Some(proof) => write_msg(&proof),
                    None => println!("punktfunk-web: pairing rejected (wrong PIN, or a MITM)"),
                }
            } else if let Ok(r) = PairResult::decode(&body) {
                credential::on_pair_result(&r);
            }
        }
    });
}

/// The host accepted: build the session the `Welcome` describes and say we are starting.
fn on_welcome(c: &mut Client, welcome: Welcome) {
    let cfg = welcome.session_config(Role::Client);
    c.codec = welcome.codec;
    c.width = welcome.mode.width;
    c.height = welcome.mode.height;
    match Session::new(cfg, Box::new(WebTransportDatagrams)) {
        Ok(session) => {
            c.session = Some(Box::new(session));
            c.phase = Phase::Live;
            // The browser has one connection, so there is no second plane to punch and no port
            // to name — `Start` still marks "begin streaming".
            write_msg(&Start { client_udp_port: 0 }.encode());
            // SAFETY: plain integers to a JavaScript function that returns before this does.
            unsafe { pf_video_config(u32::from(c.codec), c.width, c.height) };
            println!(
                "punktfunk-web: session live, codec {} at {}x{}",
                c.codec, c.width, c.height
            );
        }
        Err(e) => {
            println!("punktfunk-web: session refused: {e:?}");
            c.phase = Phase::Failed;
        }
    }
}

/// Drain the ring, run FEC/decrypt/reassembly, and hand each finished access unit to the page.
/// Returns how many were delivered. Called once per `requestAnimationFrame`.
#[unsafe(no_mangle)]
pub extern "C" fn pf_session_pump() -> u32 {
    CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        if c.phase != Phase::Live {
            return 0;
        }
        let codec = c.codec;
        let Some(session) = c.session.as_mut() else {
            return 0;
        };
        let mut delivered = 0;
        // `poll_frame` returns an error when the ring is empty, which is the non-blocking
        // contract rather than a failure — stop draining and come back next frame.
        while let Ok(frame) = session.poll_frame() {
            if frame.data.is_empty() {
                break;
            }
            let key = i32::from(is_keyframe(&frame.data, codec));
            // SAFETY: the page reads `len` bytes at `ptr` and returns before this does; `frame`
            // owns the buffer for the whole call. R3: what crosses is the encoded access unit,
            // never a decoded pixel.
            unsafe {
                pf_video_au(
                    frame.data.as_ptr(),
                    frame.data.len() as u32,
                    frame.pts_ns as f64 / 1000.0,
                    key,
                )
            };
            delivered += 1;
            if delivered >= 240 {
                break; // a burst this large means the page is behind; let it draw.
            }
        }
        c.frames += u64::from(delivered);
        delivered
    })
}

/// Access units delivered so far — the page's "is it actually streaming" check.
#[unsafe(no_mangle)]
pub extern "C" fn pf_session_frames() -> u32 {
    CLIENT.with(|c| u32::try_from(c.borrow().frames).unwrap_or(u32::MAX))
}

/// `0` idle, `1` offered, `2` live, `3` failed. Small enough to poll from the page.
#[unsafe(no_mangle)]
pub extern "C" fn pf_session_phase() -> u32 {
    CLIENT.with(|c| match c.borrow().phase {
        Phase::Idle => 0,
        Phase::Offered => 1,
        Phase::Live => 2,
        Phase::Failed => 3,
    })
}

/// Does this access unit start a decodable picture?
///
/// `EncodedVideoChunk` needs `key` or `delta` and the first chunk must be a key, but the wire
/// carries no such bit — the native decoders read it from the bitstream, so this does too. The
/// host sends parameter sets with every IDR, so their presence is the signal: SPS for H.264,
/// VPS/SPS for HEVC. Anything else is a delta, which is the safe direction — a mislabelled key
/// corrupts the decoder's state, a mislabelled delta is only dropped.
fn is_keyframe(au: &[u8], codec: u8) -> bool {
    let mut i = 0;
    while i + 3 < au.len() {
        // Annex B start code, three or four bytes.
        let payload = if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            i + 3
        } else if i + 4 < au.len()
            && au[i] == 0
            && au[i + 1] == 0
            && au[i + 2] == 0
            && au[i + 3] == 1
        {
            i + 4
        } else {
            i += 1;
            continue;
        };
        let b = au[payload];
        let hit = if codec == punktfunk_core::quic::CODEC_HEVC {
            let t = (b >> 1) & 0x3f;
            // IDR_W_RADL, IDR_N_LP, CRA, or the VPS/SPS that precede them.
            matches!(t, 19 | 20 | 21 | 32 | 33)
        } else {
            let t = b & 0x1f;
            matches!(t, 5 | 7) // IDR slice, or the SPS that precedes it
        };
        if hit {
            return true;
        }
        i = payload;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_waits_for_a_whole_message() {
        let mut inbox = Vec::new();
        assert!(take_msg(&mut inbox).is_none(), "empty");
        inbox.extend_from_slice(&[3, 0]);
        assert!(take_msg(&mut inbox).is_none(), "length but no body");
        inbox.extend_from_slice(&[1, 2]);
        assert!(take_msg(&mut inbox).is_none(), "body still short");
        inbox.push(3);
        assert_eq!(take_msg(&mut inbox).as_deref(), Some(&[1u8, 2, 3][..]));
        assert!(inbox.is_empty(), "a taken message is consumed");
    }

    #[test]
    fn keyframes_are_read_from_the_bitstream() {
        let h264 = punktfunk_core::quic::CODEC_H264;
        // SPS (type 7) then an IDR slice (type 5), the shape the host sends.
        assert!(is_keyframe(
            &[0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x65, 0x88],
            h264
        ));
        // A plain P slice (type 1) is not.
        assert!(!is_keyframe(&[0, 0, 0, 1, 0x41, 0x9a, 0x00], h264));
        // Three-byte start codes count too.
        assert!(is_keyframe(&[0, 0, 1, 0x65, 0x88], h264));
        assert!(!is_keyframe(&[], h264));

        let hevc = punktfunk_core::quic::CODEC_HEVC;
        // HEVC types live in bits 6..1: VPS = 32 -> 0x40, IDR_W_RADL = 19 -> 0x26.
        assert!(is_keyframe(&[0, 0, 0, 1, 0x40, 0x01], hevc));
        assert!(is_keyframe(&[0, 0, 0, 1, 0x26, 0x01], hevc));
        // TRAIL_R = 1 -> 0x02 is a delta.
        assert!(!is_keyframe(&[0, 0, 0, 1, 0x02, 0x01], hevc));
    }

    #[test]
    fn two_messages_in_one_chunk_both_come_out() {
        // A stream hands over arbitrary chunk boundaries, so several messages can arrive at once.
        let mut inbox = vec![2, 0, 0xaa, 0xbb, 1, 0, 0xcc];
        assert_eq!(take_msg(&mut inbox).as_deref(), Some(&[0xaa, 0xbb][..]));
        assert_eq!(take_msg(&mut inbox).as_deref(), Some(&[0xcc][..]));
        assert!(take_msg(&mut inbox).is_none());
    }
}
