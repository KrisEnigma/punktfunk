//! Opening a backend inside WUDFHost, and the QPC clock its frames are stamped with.
//!
//! [`open_backend`] is one `open` per [`OpenSpec::backend`]; the caller walks its preference
//! list and reports what took. PyroWave's private Vulkan instance goes through the box's
//! implicit layers unless [`disable_implicit_vulkan_layers`] ran first: overlays hang in
//! session 0, where there is no desktop to hook.

use pf_encode_win::{ChromaFormat, Codec, Encoder};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use super::convert::{AdapterId, Fail, InputKind};

/// Everything one backend `open` takes, in the backends' own vocabulary.
#[derive(Clone, Copy, Debug)]
pub struct OpenSpec {
    /// 1 NVENC, 2 AMF, 3 QSV, 4 PyroWave.
    pub backend: u32,
    pub codec: Codec,
    pub kind: InputKind,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
    pub bit_depth: u8,
    pub chroma: ChromaFormat,
}

/// The wire codec numbering (1 H264, 2 HEVC, 3 AV1, 4 PyroWave).
pub fn codec_from_wire(codec: u32) -> Option<Codec> {
    Some(match codec {
        1 => Codec::H264,
        2 => Codec::H265,
        3 => Codec::Av1,
        4 => Codec::PyroWave,
        _ => return None,
    })
}

/// Implicit Vulkan layers (overlays, our pf-vkhdr-layer) hang in session 0, and the encoder's
/// private instance wants none of them. The loader-wide knob needs a 1.3.234+ loader; each
/// manifest's own `disable_environment` works on any. Once per process.
pub fn disable_implicit_vulkan_layers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: WUDFHost is this driver's own process (`ProcessSharingDisabled`); Windows'
        // SetEnvironmentVariable is thread-safe and nothing here parses the environment
        // concurrently.
        unsafe {
            for (k, v) in [
                ("VK_LOADER_LAYERS_DISABLE", "~implicit~"),
                ("DISABLE_RTSS_LAYER", "1"),
                ("DISABLE_PF_VKHDR", "1"),
                ("DISABLE_VK_LAYER_VALVE_steam_overlay_1", "1"),
                ("DISABLE_VK_LAYER_VALVE_steam_fossilize_1", "1"),
                ("EOS_OVERLAY_DISABLE_VULKAN_WIN64", "1"),
                ("DISABLE_GALAXY_OVERLAY", "1"),
            ] {
                std::env::set_var(k, v);
            }
        }
    });
}

/// One backend open on `adapter`. `Err` carries the stage tag; the backend's message is logged.
pub fn open_backend(spec: &OpenSpec, adapter: &AdapterId) -> Result<Box<dyn Encoder>, Fail> {
    let (w, h, fps, bps) = (spec.width, spec.height, spec.fps, spec.bitrate_bps);
    let (depth, chroma) = (spec.bit_depth, spec.chroma);
    let format = spec.kind.pixel_format();
    let luid = Some(adapter.luid62());
    let opened: anyhow::Result<Box<dyn Encoder>> = match spec.backend {
        1 => pf_encode_win::nvenc::NvencD3d11Encoder::open(
            spec.codec, format, w, h, fps, bps, depth, chroma, 1, luid,
        )
        .map(|e| Box::new(e) as Box<dyn Encoder>),
        2 => pf_encode_win::amf::AmfEncoder::open(
            spec.codec, format, w, h, fps, bps, depth, chroma, luid,
        )
        .map(|e| Box::new(e) as Box<dyn Encoder>),
        3 => pf_encode_win::qsv::QsvEncoder::open(
            spec.codec, format, w, h, fps, bps, depth, chroma, luid,
        )
        .map(|e| Box::new(e) as Box<dyn Encoder>),
        4 => {
            disable_implicit_vulkan_layers();
            pf_encode_win::pyrowave::PyroWaveEncoder::open(
                w,
                h,
                fps,
                bps,
                chroma,
                depth,
                adapter.vendor_id,
                adapter.device_id,
            )
            .map(|e| Box::new(e) as Box<dyn Encoder>)
        }
        _ => return Err((-1, "backend")),
    };
    opened.map_err(|e| {
        dbglog!(
            "[pf-vd] encode: backend {} open FAILED: {e:#}",
            spec.backend
        );
        (-1, "open")
    })
}

pub fn qpc_now() -> u64 {
    let mut qpc = 0i64;
    // SAFETY: plain FFI; `qpc` is a valid local out-param.
    let _ = unsafe { QueryPerformanceCounter(&mut qpc) };
    qpc as u64
}

pub fn qpc_frequency() -> u64 {
    let mut hz = 0i64;
    // SAFETY: plain FFI; `hz` is a valid local out-param.
    let _ = unsafe { QueryPerformanceFrequency(&mut hz) };
    (hz as u64).max(1)
}

pub fn qpc_to_ns(qpc: u64, hz: u64) -> u64 {
    (u128::from(qpc) * 1_000_000_000 / u128::from(hz)) as u64
}
