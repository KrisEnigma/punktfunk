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

use super::{Inbox, WebTransportPlane};
use anyhow::{Context, Result};
use punktfunk_core::config::{CompositorPref, FecConfig, FecScheme, GamepadPref, Role};
use punktfunk_core::quic::ColorInfo;
use punktfunk_core::quic::{Hello, Start, Welcome};
use punktfunk_core::session::Session;
use rand::RngCore;
use std::sync::Arc;
use wtransport::Connection;

/// Read the handshake, then stream until the browser goes away.
pub(crate) async fn run(conn: Connection, inbox: Arc<Inbox>) -> Result<()> {
    let (mut tx, rx) = conn.accept_bi().await.context("accept control stream")?;
    let mut rx: CtlReader = punktfunk_core::quic::io::MsgReader::new(rx);

    // The browser opens the control stream and sends `Hello` on it.
    let hello_bytes = rx.read_msg().await.context("read Hello")?;
    let hello = Hello::decode(&hello_bytes).map_err(|e| anyhow::anyhow!("bad Hello: {e:?}"))?;
    tracing::info!(
        width = hello.mode.width,
        height = hello.mode.height,
        fps = hello.mode.refresh_hz,
        name = hello.name.as_deref().unwrap_or("<unnamed>"),
        "browser Hello"
    );

    let welcome = offer(&conn, &hello);
    punktfunk_core::quic::io::write_msg(&mut tx, &welcome.encode())
        .await
        .context("write Welcome")?;

    // `Start` carries a UDP port on the native plane; a browser has no second plane, so the
    // message is only a "begin" marker here.
    let start_bytes = rx.read_msg().await.context("read Start")?;
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

/// The control plane's own framing, on a WebTransport stream.
///
/// `punktfunk_core::quic::io` is generic over `AsyncRead`/`AsyncWrite`, and `wtransport`'s
/// streams implement both — so this is the same reader the native plane uses, not a second
/// implementation of the same `u16`-length frame that could drift from it.
type CtlReader = punktfunk_core::quic::io::MsgReader<wtransport::RecvStream>;
