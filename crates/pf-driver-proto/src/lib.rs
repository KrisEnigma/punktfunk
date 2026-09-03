//! Shared binary contract between the punktfunk host and the `pf-vdisplay` IddCx driver.
//!
//! Two planes:
//! * [`control`] — `DeviceIoControl` (add/remove, adapter pin, keepalive, info, clear-all,
//!   frame-channel delivery). Owned and versioned — not the SudoVDA ABI.
//! * [`frame`] — IDD-push transport. The host creates unnamed keyed-mutex textures plus a
//!   header and a frame-ready event, duplicates handles into WUDFHost, and delivers the
//!   values over [`control::IOCTL_SET_FRAME_CHANNEL`]. No object-name scheme: unnamed
//!   objects cannot be enumerated, opened by name, or squatted. This crate owns
//!   [`frame::SharedHeader`], [`frame::FrameToken`], the channel-delivery struct, and the
//!   status codes. Evidence: `design/idd-push-security.md`.
//!
//! GUID and LUID travel as integers; each side converts to its own `windows` / bindgen types.
//! `Pod` + `offset_of!` asserts make a one-sided layout edit a compile error.
#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]

extern crate alloc;

/// Device-interface GUID `{70667664-7044-5350-a1b2-c3d4e5f60001}`.
/// Not SudoVDA's `{e5bcc234-…}`: a private GUID so a real SudoVDA install cannot bind.
/// Construct via `GUID::from_u128(PF_VDISPLAY_INTERFACE_GUID_U128)`.
pub const PF_VDISPLAY_INTERFACE_GUID_U128: u128 = 0x7066_7664_7044_5350_a1b2_c3d4_e5f6_0001;

/// `(Data1, Data2, Data3, Data4)` of [`PF_VDISPLAY_INTERFACE_GUID_U128`].
/// This crate is `no_std` and has no `GUID` type; callers rebuild theirs from these fields.
#[must_use]
pub const fn interface_guid_fields() -> (u32, u16, u16, [u8; 8]) {
    let g = PF_VDISPLAY_INTERFACE_GUID_U128;
    (
        (g >> 96) as u32,
        (g >> 80) as u16,
        (g >> 64) as u16,
        (g as u64).to_be_bytes(),
    )
}

/// Bumped on any incompatible change to either plane. Exchanged via [`control::IOCTL_GET_INFO`];
/// host and driver assert a match at startup.
///
/// v6 is additive over v3–v5: [`control::IOCTL_UPDATE_MODES`], hardware-cursor channel
/// ([`control::IOCTL_SET_CURSOR_CHANNEL`]), mid-stream cursor flip
/// ([`control::IOCTL_SET_CURSOR_FORWARD`]). A v3 driver lacks only UPDATE_MODES; the host
/// gates that IOCTL on the handshake and falls back to re-arrival. Ship driver+host together
/// for v4+ features. Evidence: `design/first-frame-and-resize-latency.md`.
///
/// [`control::AddRequest`] luminance tail and [`control::AddReply::cursor_excluded`] are
/// prefix-compatible (no bump): a short read/write sees zeros = unknown. A hardware-cursor
/// declare is irrevocable on the adapter — DWM excludes the pointer from every later monitor
/// until the adapter resets — so the host self-composites when the cursor channel is absent.
pub const PROTOCOL_VERSION: u32 = 6;

/// Oldest driver this host still drives. v4+ IOCTLs are additive; a v3 driver lacks only
/// `IOCTL_UPDATE_MODES`, which the host gates on the handshake and covers with re-arrival.
pub const MIN_DRIVER_PROTOCOL_VERSION: u32 = 3;

/// `CTL_CODE(FILE_DEVICE_UNKNOWN = 0x22, func, METHOD_BUFFERED = 0, FILE_ANY_ACCESS = 0)`.
pub const fn ctl_code(func: u32) -> u32 {
    (0x22u32 << 16) | (func << 2)
}

/// Control (`DeviceIoControl`) plane: add/remove, adapter pin, keepalive, frame-channel delivery.
pub mod control {
    use super::ctl_code;
    use super::frame::RING_LEN;
    use bytemuck::{Pod, Zeroable};

    // Contiguous op space at 0x900 — distinct from SudoVDA's gappy 0x800/0x888/0x8FF numbering.
    /// Add a virtual monitor at a mode → [`AddReply`]. Input [`AddRequest`].
    pub const IOCTL_ADD: u32 = ctl_code(0x900);
    /// Remove a virtual monitor by session id. Input [`RemoveRequest`].
    pub const IOCTL_REMOVE: u32 = ctl_code(0x901);
    /// Pin the IddCx render adapter (hybrid-GPU IDD-push). Input [`SetRenderAdapterRequest`].
    pub const IOCTL_SET_RENDER_ADAPTER: u32 = ctl_code(0x902);
    /// Keepalive (resets the driver watchdog). No payload.
    pub const IOCTL_PING: u32 = ctl_code(0x903);
    /// Version + watchdog handshake → [`InfoReply`]. No input.
    pub const IOCTL_GET_INFO: u32 = ctl_code(0x904);
    /// Tear down every virtual monitor (host-startup orphan reap). First-class op — not the
    /// SudoVDA "send-and-hope-it's-ignored" hack.
    pub const IOCTL_CLEAR_ALL: u32 = ctl_code(0x905);
    /// Deliver handle VALUES of unnamed frame objects duplicated into WUDFHost. Input
    /// [`SetFrameChannelRequest`]. Sent again on every mid-session ring recreate (HDR-mode flip).
    pub const IOCTL_SET_FRAME_CHANNEL: u32 = ctl_code(0x906);
    /// Refresh a LIVE monitor's target-mode list via `IddCxMonitorUpdateModes2`. Input
    /// [`UpdateModesRequest`]. CCD then forces the new mode on the same monitor — no REMOVE→ADD,
    /// so OS identity and the driver's swap-chain survive. A v3 driver fails the unknown IOCTL;
    /// the host falls back to re-arrival.
    pub const IOCTL_UPDATE_MODES: u32 = ctl_code(0x907);
    /// Deliver the unnamed [`cursor::CursorShm`](crate::cursor) mapping (handle VALUE duplicated
    /// into WUDFHost). No event — the host polls the seqlock. Sent once after ADD when
    /// [`AddRequest::hw_cursor`] was set. Input [`SetCursorChannelRequest`].
    pub const IOCTL_SET_CURSOR_CHANNEL: u32 = ctl_code(0x908);
    /// Flip a LIVE monitor's hardware-cursor declaration. `enable = 1` re-declares
    /// (`IddCxMonitorSetupHardwareCursor`); `enable = 0` un-declares so DWM composites the
    /// pointer into the frame. Only meaningful after [`IOCTL_SET_CURSOR_CHANNEL`]. Input
    /// [`SetCursorForwardRequest`].
    pub const IOCTL_SET_CURSOR_FORWARD: u32 = ctl_code(0x909);
    /// Spike S5, `encode-probe` driver builds only: run the encoder backends inside WUDFHost on
    /// one monitor's frames. Input [`EncodeProbeRequest`]. `STATUS_DEVICE_BUSY` while a run is
    /// still going; `STATUS_NOT_FOUND` from a driver built without the feature.
    pub const IOCTL_ENCODE_PROBE_ARM: u32 = ctl_code(0x90A);
    /// The probe's tally → [`EncodeProbeReply`]. No input.
    pub const IOCTL_ENCODE_PROBE_STATUS: u32 = ctl_code(0x90B);

    /// `IOCTL_ADD` input. `session_id` keys the monitor (host refcount owns collisions).
    /// The driver advertises this mode as preferred; the host still CCD-forces the active mode.
    ///
    /// Size: the luminance + `hw_cursor` tail after `preferred_monitor_id` is prefix-compatible
    /// (no protocol bump). An old driver reads [`ADD_REQUEST_LEGACY_SIZE`] bytes; an old host
    /// sends that prefix. Zero tail = unknown / off. Further fields must follow the same rule.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct AddRequest {
        pub session_id: u64,
        pub width: u32,
        pub height: u32,
        pub refresh_hz: u32,
        /// Host-preferred per-client monitor id (`1..=15`) — EDID serial / IddCx `ConnectorIndex` /
        /// `ContainerId`. Stable across reconnects so Windows reapplies per-monitor DPI. `0` = AUTO
        /// (lowest-free id). Occupies the old `_reserved` at offset 20: an old driver ignores it.
        pub preferred_monitor_id: u32,
        /// Client display peak luminance in nits → EDID CTA-861.3 Desired Content Max Luminance.
        /// `0` = unknown → the driver keeps its built-in ~1000-nit block.
        pub max_luminance_nits: u32,
        /// Client max frame-average luminance in nits. `0` = unknown.
        pub max_frame_avg_nits: u32,
        /// Client min luminance in milli-nits (0.001 cd/m² — CTA min lives well below 1 nit).
        /// `0` = unknown.
        pub min_luminance_millinits: u32,
        /// Non-zero = declare an IddCx hardware cursor: DWM excludes the pointer from the frame.
        /// Occupies the old tail `_reserved` at offset 36: an old driver ignores it (stays composited).
        pub hw_cursor: u32,
    }

    /// [`AddRequest`] size before the luminance tail — prefix an old driver reads / old host sends.
    pub const ADD_REQUEST_LEGACY_SIZE: usize = 24;

    /// `IOCTL_ADD` reply: the OS target id + the adapter LUID the IDD landed on (split low/high to
    /// match `windows` `LUID { LowPart: u32, HighPart: i32 }`).
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct AddReply {
        pub adapter_luid_low: u32,
        pub adapter_luid_high: i32,
        pub target_id: u32,
        /// Monitor id the driver actually used. Occupies the old `_reserved` at offset 12: an old
        /// driver leaves it `0`, so the host can tell the preference was ignored.
        pub resolved_monitor_id: u32,
        /// WUDFHost pid — duplication target for unnamed frame-object handles. Reported per-ADD,
        /// not per-open, so a WUDFHost restart cannot leave the host duplicating into a dead process.
        pub wudf_pid: u32,
        /// Non-zero = this adapter already carries an irrevocable hardware-cursor declare.
        /// Exclusion is adapter-wide until the adapter resets; sessions without the cursor channel
        /// must self-composite. Prefix-compatible after [`ADD_REPLY_LEGACY_SIZE`]: zeros = unknown.
        pub cursor_excluded: u32,
    }

    /// [`AddReply`] size before `cursor_excluded` — prefix an old driver writes / old host retrieves.
    pub const ADD_REPLY_LEGACY_SIZE: usize = 20;

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct RemoveRequest {
        pub session_id: u64,
    }

    /// `IOCTL_UPDATE_MODES` input: live monitor (ADD `session_id`) and the new preferred mode.
    /// The driver replaces the stored list (new mode first, then built-in fallbacks) and pushes
    /// it via `IddCxMonitorUpdateModes2`.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct UpdateModesRequest {
        pub session_id: u64,
        pub width: u32,
        pub height: u32,
        pub refresh_hz: u32,
        /// Pads the `u64`-aligned struct to a multiple of 8 (Pod forbids implicit tail padding).
        pub _reserved: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetRenderAdapterRequest {
        pub luid_low: u32,
        pub luid_high: i32,
    }

    /// `IOCTL_GET_INFO` reply. `protocol_version` is asserted against [`super::PROTOCOL_VERSION`].
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct InfoReply {
        pub protocol_version: u32,
        pub watchdog_timeout_s: u32,
    }

    /// `IOCTL_SET_FRAME_CHANNEL` input. Every handle is a VALUE already duplicated into WUDFHost.
    /// Adopt-on-success-only (`design/idd-push-security.md` invariant 5): the driver owns (and
    /// closes) the handles IFF the IOCTL succeeds. On any error the host reaps via
    /// `DUPLICATE_CLOSE_SOURCE`. Closing on error double-closes possibly-reused handle values.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetFrameChannelRequest {
        pub target_id: u32,
        /// Must match the shared header's generation at attach; a stale delivery is dropped.
        pub generation: u32,
        /// Leading valid entries of `texture_handles` (`1..=`[`RING_LEN`]).
        pub ring_len: u32,
        /// Bytes the host allocated for the shared header section (v3; the former `_pad`, same
        /// offset — a pre-v3 host leaves it 0, which every v3 gate reads as "v2 prefix only").
        /// Together with the header's stamped `version` this is the ONLY permission to touch the
        /// v3 tail: see [`frame::v3_readable`](crate::frame::v3_readable).
        pub header_bytes: u32,
        /// The shared-header file-mapping handle (the driver maps it and writes status/publish tokens).
        pub header_handle: u64,
        pub event_handle: u64,
        /// Shared NT handles; the driver opens them via `ID3D11Device1::OpenSharedResource1`.
        pub texture_handles: [u64; RING_LEN_USIZE],
    }

    /// [`RING_LEN`] as usize. The array is sized at the compile-time max; `ring_len` is live count.
    pub const RING_LEN_USIZE: usize = RING_LEN as usize;

    /// `IOCTL_SET_FRAME_CHANNEL` input from a fence-ring host (immunity plan WP7): the v1 request
    /// verbatim as a prefix, then the two shared `ID3D11Fence` handle values (duplicated into
    /// WUDFHost like the textures). Same IOCTL code — the driver tells the two apart by input
    /// length (`>= size_of::<SetFrameChannelRequestV2>()`), and a pre-fence driver reads the v1
    /// prefix of a v2 request unchanged (the ownership contract covers the fence handles too: the
    /// host reaps them on error, the driver closes them on success). Zero fence handles = no fences
    /// (a v2-shaped request from a host that negotiated the keyed-mutex arm).
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetFrameChannelRequestV2 {
        pub v1: SetFrameChannelRequest,
        pub ready_fence_handle: u64,
        pub retire_fence_handle: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetCursorChannelRequest {
        pub target_id: u32,
        pub _pad: u32,
        /// [`cursor::CursorShm`](crate::cursor) mapping handle VALUE, already duplicated into WUDFHost.
        pub header_handle: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetCursorForwardRequest {
        pub target_id: u32,
        /// `1` = declare (exclude + forward), `0` = un-declare (DWM composites).
        pub enable: u32,
    }

    /// [`IOCTL_ENCODE_PROBE_ARM`] input. Zero `frames` / `bitrate_kbps` / `fps` take the
    /// driver's defaults (300 / 20000 / 60).
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct EncodeProbeRequest {
        /// OS target of the monitor to tap; `0` = whichever drain worker offers first.
        pub target_id: u32,
        /// 1 NVENC, 2 AMF, 3 QSV, 4 PyroWave.
        pub backend: u32,
        /// 1 H264, 2 HEVC, 3 AV1, 4 PyroWave (backend 4 only, and only with backend 4).
        pub codec: u32,
        /// `0` = the backend's own input: BGRA straight into NVENC, NV12 for AMF/QSV, planar
        /// Y + CbCr for PyroWave. `1` = BGRA→NV12 on the video engine (the NVENC colour A/B).
        pub input: u32,
        pub frames: u32,
        pub bitrate_kbps: u32,
        pub fps: u32,
        pub flags: u32,
    }

    /// [`IOCTL_ENCODE_PROBE_STATUS`] reply. `mean_*` / `max_*` cover every AU after the first;
    /// `first_au_us` alone carries the backend's lazy session init. `name` is a short
    /// NUL-padded failure tag (the driver log has the full error).
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct EncodeProbeReply {
        /// 0 idle, 1 armed, 2 running, 3 done, 4 failed.
        pub state: u32,
        pub backend_opened: u32,
        pub frames_submitted: u32,
        pub aus: u32,
        pub bytes: u64,
        pub open_us: u32,
        pub first_au_us: u32,
        pub mean_submit_to_au_us: u32,
        pub max_submit_to_au_us: u32,
        /// Frames the drain worker had no free pool slot for.
        pub drops: u32,
        pub error: i32,
        pub name: [u8; 32],
    }

    // Layout is load-bearing across the process boundary. Pod rejects internal padding; these
    // assert the externally-visible sizes. `offset_of!` catches a same-size field reorder.
    const _: () = {
        use core::mem::{offset_of, size_of};

        assert!(size_of::<AddRequest>() == 40);
        assert!(offset_of!(AddRequest, session_id) == 0);
        assert!(offset_of!(AddRequest, width) == 8);
        assert!(offset_of!(AddRequest, height) == 12);
        assert!(offset_of!(AddRequest, refresh_hz) == 16);
        assert!(offset_of!(AddRequest, preferred_monitor_id) == 20);
        // Luminance tail starts at the legacy boundary (prefix-compat).
        assert!(offset_of!(AddRequest, max_luminance_nits) == ADD_REQUEST_LEGACY_SIZE);
        assert!(offset_of!(AddRequest, max_frame_avg_nits) == 28);
        assert!(offset_of!(AddRequest, min_luminance_millinits) == 32);
        // Former tail `_reserved` — same offset, same total size (rename-only).
        assert!(offset_of!(AddRequest, hw_cursor) == 36);
        assert!(size_of::<AddRequest>() == 40);

        assert!(size_of::<AddReply>() == 24);
        assert!(offset_of!(AddReply, adapter_luid_low) == 0);
        assert!(offset_of!(AddReply, adapter_luid_high) == 4);
        assert!(offset_of!(AddReply, target_id) == 8);
        assert!(offset_of!(AddReply, resolved_monitor_id) == 12);
        assert!(offset_of!(AddReply, wudf_pid) == 16);
        // cursor_excluded starts at the legacy boundary (prefix-compat).
        assert!(offset_of!(AddReply, cursor_excluded) == ADD_REPLY_LEGACY_SIZE);

        assert!(size_of::<SetFrameChannelRequest>() == 32 + 8 * RING_LEN_USIZE);
        assert!(offset_of!(SetFrameChannelRequest, target_id) == 0);
        assert!(offset_of!(SetFrameChannelRequest, generation) == 4);
        assert!(offset_of!(SetFrameChannelRequest, ring_len) == 8);
        assert!(offset_of!(SetFrameChannelRequest, header_handle) == 16);
        assert!(offset_of!(SetFrameChannelRequest, event_handle) == 24);
        assert!(offset_of!(SetFrameChannelRequest, texture_handles) == 32);

        assert!(size_of::<RemoveRequest>() == 8);
        assert!(offset_of!(RemoveRequest, session_id) == 0);

        assert!(size_of::<SetCursorChannelRequest>() == 16);
        assert!(offset_of!(SetCursorChannelRequest, target_id) == 0);
        assert!(offset_of!(SetCursorChannelRequest, header_handle) == 8);
        assert!(size_of::<SetCursorForwardRequest>() == 8);
        assert!(offset_of!(SetCursorForwardRequest, target_id) == 0);
        assert!(offset_of!(SetCursorForwardRequest, enable) == 4);

        assert!(size_of::<UpdateModesRequest>() == 24);
        assert!(offset_of!(UpdateModesRequest, session_id) == 0);
        assert!(offset_of!(UpdateModesRequest, width) == 8);
        assert!(offset_of!(UpdateModesRequest, height) == 12);
        assert!(offset_of!(UpdateModesRequest, refresh_hz) == 16);

        assert!(size_of::<SetRenderAdapterRequest>() == 8);
        assert!(offset_of!(SetRenderAdapterRequest, luid_low) == 0);
        assert!(offset_of!(SetRenderAdapterRequest, luid_high) == 4);

        assert!(size_of::<InfoReply>() == 8);
        assert!(offset_of!(InfoReply, protocol_version) == 0);
        assert!(offset_of!(InfoReply, watchdog_timeout_s) == 4);

        assert!(size_of::<EncodeProbeRequest>() == 32);
        assert!(offset_of!(EncodeProbeRequest, target_id) == 0);
        assert!(offset_of!(EncodeProbeRequest, backend) == 4);
        assert!(offset_of!(EncodeProbeRequest, frames) == 16);
        assert!(offset_of!(EncodeProbeRequest, flags) == 28);
        assert!(size_of::<EncodeProbeReply>() == 80);
        assert!(offset_of!(EncodeProbeReply, state) == 0);
        assert!(offset_of!(EncodeProbeReply, bytes) == 16);
        assert!(offset_of!(EncodeProbeReply, open_us) == 24);
        assert!(offset_of!(EncodeProbeReply, error) == 44);
        assert!(offset_of!(EncodeProbeReply, name) == 48);
    };
}

/// The 256-byte EDID the `pf-vdisplay` driver hands IddCx for each virtual monitor: an EDID 1.4
/// base block plus a CTA-861.3 extension carrying a BT.2020 Colorimetry block and an HDR Static
/// Metadata block declaring the SMPTE ST 2084 (PQ) EOTF. Windows reads a display's HDR capability
/// from that CTA block; without it the monitor is SDR-only whatever the IddCx adapter's FP16 /
/// wide-gamut / 10-bit caps say.
///
/// Identity: manufacturer "PNK", product name "Punktfunk" (the 0xFC descriptor Windows shows), and
/// a per-monitor serial at base offset 0x0C that [`get_serial`] reads back out of the EDID the OS
/// hands to the mode callbacks. No HDMI Vendor-Specific Data Block: a VSDB carries physical-sink
/// facts (CEC address, TMDS limits) a virtual display does not have, and Windows drives the
/// monitor without one.
///
/// Lives here, not in the driver: the driver only builds under the WDK, and one wrong byte drops
/// HDR silently. `no_std` + integer-only, so it drops into the driver unchanged.
pub mod edid {
    /// `2^(k/32)` for `k = 0..32` in Q16 fixed point (`round(2^(k/32) * 65536)`) — the fractional
    /// step table for the CTA-861.3 luminance exponent.
    const POW2_Q16: [u32; 32] = [
        65536, 66971, 68438, 69936, 71468, 73032, 74632, 76266, 77936, 79642, 81386, 83169, 84990,
        86851, 88752, 90696, 92682, 94711, 96785, 98905, 101070, 103283, 105545, 107856, 110218,
        112631, 115098, 117618, 120194, 122825, 125515, 128263,
    ];

    /// Decode a CTA-861.3 max / frame-average luminance code to MILLI-nits:
    /// `L = 50 * 2^(CV/32)` cd/m², so `L_millinits = 50_000 * 2^(CV/32)`.
    /// (`CV = 255` ≈ 12_525 nits — comfortably inside u64 at Q16.)
    pub const fn cta_max_millinits(code: u8) -> u64 {
        let whole = code as u32 / 32;
        let frac = code as u32 % 32;
        ((50_000u64 << whole) * POW2_Q16[frac as usize] as u64) >> 16
    }

    /// Largest CTA-861.3 code whose decoded luminance does not exceed `nits` — never advertise
    /// brighter than the glass. Clamped to `1..=255`: `0` is "no data" on the wire; callers gate
    /// on `nits > 0`. A sub-51-nit request (no real HDR panel) still codes as 1.
    pub fn cta_max_luminance_code(nits: u32) -> u8 {
        let target = nits as u64 * 1000;
        let mut code = 1u8;
        while code < 255 && cta_max_millinits(code + 1) <= target {
            code += 1;
        }
        code
    }

    /// Floor integer square root (Newton). `u64::isqrt` needs Rust 1.84, above this crate's 1.82
    /// MSRV. Converges in ≤ 6 iterations from the power-of-two seed.
    fn isqrt_u64(x: u64) -> u64 {
        if x == 0 {
            return 0;
        }
        // Seed strictly above sqrt(x): 2^(ceil(bits/2)).
        let mut r = 1u64 << (64 - x.leading_zeros()).div_ceil(2);
        loop {
            let next = (r + x / r) / 2;
            if next >= r {
                return r;
            }
            r = next;
        }
    }

    /// Code a display's min luminance (MILLI-nits) as the CTA-861.3 min-luminance value, which is
    /// relative to the block's coded max: `L_min = L_max * (CV/255)^2 / 100`, so
    /// `CV = 255 * sqrt(100 * L_min / L_max)` — rounded to nearest. `max_code` is the byte
    /// produced by [`cta_max_luminance_code`]; a result of `0` (a true-black panel, or
    /// `millinits = 0` = unknown) is valid on the wire.
    pub fn cta_min_luminance_code(millinits: u32, max_code: u8) -> u8 {
        let max_millinits = cta_max_millinits(max_code);
        if millinits == 0 || max_millinits == 0 {
            return 0;
        }
        // CV = sqrt(100 * 255^2 * L_min / L_max); round to nearest by comparing the two flanking
        // squares (the integer sqrt floors).
        let x = (100u64 * 255 * 255).saturating_mul(millinits as u64) / max_millinits;
        let floor = isqrt_u64(x);
        let cv = if (floor + 1) * (floor + 1) - x <= x - floor * floor {
            floor + 1
        } else {
            floor
        };
        cv.min(255) as u8
    }

    /// Fixed reduced-blanking geometry for [`dtd`] (CVT-RBv2-shaped): 80 px of horizontal and 45
    /// lines of vertical blanking, front-porch/sync splits within them. A virtual display has no
    /// real scan-out, so the blanking only has to be self-consistent — the pixel clock is derived
    /// from these same totals.
    const DTD_H_BLANK: u32 = 80;
    const DTD_V_BLANK: u32 = 45;
    const DTD_H_SYNC_OFFSET: u32 = 8;
    const DTD_H_SYNC_WIDTH: u32 = 32;
    const DTD_V_SYNC_OFFSET: u32 = 3;
    const DTD_V_SYNC_WIDTH: u32 = 5;

    /// 18-byte EDID detailed timing descriptor for `width`×`height`@`refresh_hz` with the fixed
    /// reduced blanking above. `None` when the mode does not fit: pixel clock above 655.35 MHz
    /// (u16 10 kHz field — 4K120-class) or active dimensions above the 12-bit fields. Flags byte
    /// 0x1E: digital separate sync, +H/+V.
    pub fn dtd(width: u32, height: u32, refresh_hz: u32) -> Option<[u8; 18]> {
        if width == 0 || height == 0 || refresh_hz == 0 || width > 4095 || height > 4095 {
            return None;
        }
        let h_total = u64::from(width + DTD_H_BLANK);
        let v_total = u64::from(height + DTD_V_BLANK);
        let clock_10khz = h_total * v_total * u64::from(refresh_hz) / 10_000;
        let clock_10khz = u16::try_from(clock_10khz).ok()?;
        let mut d = [0u8; 18];
        d[0..2].copy_from_slice(&clock_10khz.to_le_bytes());
        d[2] = (width & 0xFF) as u8;
        d[3] = (DTD_H_BLANK & 0xFF) as u8;
        d[4] = (((width >> 8) & 0x0F) << 4) as u8 | ((DTD_H_BLANK >> 8) & 0x0F) as u8;
        d[5] = (height & 0xFF) as u8;
        d[6] = (DTD_V_BLANK & 0xFF) as u8;
        d[7] = (((height >> 8) & 0x0F) << 4) as u8 | ((DTD_V_BLANK >> 8) & 0x0F) as u8;
        d[8] = (DTD_H_SYNC_OFFSET & 0xFF) as u8;
        d[9] = (DTD_H_SYNC_WIDTH & 0xFF) as u8;
        d[10] = (((DTD_V_SYNC_OFFSET & 0x0F) << 4) | (DTD_V_SYNC_WIDTH & 0x0F)) as u8;
        d[11] = ((((DTD_H_SYNC_OFFSET >> 8) & 0x03) << 6)
            | (((DTD_H_SYNC_WIDTH >> 8) & 0x03) << 4)
            | (((DTD_V_SYNC_OFFSET >> 4) & 0x03) << 2)
            | ((DTD_V_SYNC_WIDTH >> 4) & 0x03)) as u8;
        // Bytes 12..16 (image size mm, borders) stay 0 = undefined.
        d[17] = 0x1E;
        Some(d)
    }

    /// Per-monitor serial number: base-block offset 0x0C, little-endian u32.
    const SERIAL_OFFSET: usize = 0x0C;

    /// EDID 1.4 base block. Differs from a plain SDR virtual EDID by revision 1.4 (byte 19),
    /// 10-bit digital video input (byte 20 = 0xB0) and one extension present (byte 126 = 0x01).
    /// The checksum (byte 127), the serial at 0x0C and the preferred DTD are patched in
    /// [`generate`], so editing the name here needs no hand-computed checksum.
    #[rustfmt::skip]
    const BASE: [u8; 128] = [
        0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, // fixed header
        0x41, 0xCB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // mfr "PNK", product code 1 (0 = "unset" to EDID tooling), serial (patched)
        0xFF, 0x21, 0x01, 0x04, 0xB0, 0x32, 0x1F, 0x78, // week/year, EDID 1.4, 10-bit digital, size, gamma
        0x03, 0x78, 0xB1, 0xB5, 0x4A, 0x2B, 0xCC, 0x21, // feature (sRGB-default CLEARED), BT.2020 primaries...
        0x0B, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01, // ...BT.2020 primaries, established timings, std timings
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x3A, // std timings, DTD 1 (placeholder preferred timing)
        0x80, 0x18, 0x71, 0x38, 0x2D, 0x40, 0x58, 0x2C,
        0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1E,
        0x00, 0x00, 0x00, 0xFD, 0x08, 0x17, 0xF0, 0x0F, // range-limits: offsets H-max+255, 23-240 Hz, min-H 15 kHz...
        0xFF, 0xFF, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20, // ...max-H 510 kHz, max clock 2550 MHz (150 was below the driver's own 1080p120 default)
        0x20, 0x20, 0x00, 0x00, 0x00, 0xFC, 0x00, 0x50, // name descriptor "Punktfunk"
        0x75, 0x6E, 0x6B, 0x74, 0x66, 0x75, 0x6E, 0x6B,
        0x0A, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x00, // empty 4th descriptor...
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, // ...byte 126 = 1 extension, byte 127 = checksum
    ];

    /// CTA-861.3 extension block header (block 1, bytes 0..4). What follows is a Data Block
    /// Collection holding the Colorimetry and HDR Static Metadata blocks; the rest of the block is
    /// padding up to the checksum at byte 255.
    #[rustfmt::skip]
    const CTA_HEADER: [u8; 4] = [
        0x02, // CTA Extension tag
        0x03, // revision 3 (CTA-861.3 — required for the extended-tag data blocks below)
        0x0F, // D = 15: the (empty) DTD region starts at block byte 15, i.e. data blocks occupy bytes 4..15
        0x00, // 0 native DTDs; no basic audio; no YCbCr 4:4:4/4:2:2 (RGB-only, matching the wire format)
    ];

    /// Colorimetry Data Block (CTA extended tag 0x05): declare BT.2020 RGB. YCbCr variants stay
    /// clear — the IddCx wire format is RGB-only — and the gamut-metadata flags are 0.
    #[rustfmt::skip]
    const COLORIMETRY_DB: [u8; 4] = [
        0xE3, // tag 0b111 (use-extended-tag) | length 3
        0x05, // extended tag: Colorimetry
        0x80, // BT2020RGB (bit 7); xvYCC/sYCC/opRGB/BT2020 YCC/cYCC all clear
        0x00, // gamut metadata profiles MD0..MD3: none
    ];

    /// HDR Static Metadata Data Block (CTA extended tag 0x06): EOTFs = Traditional SDR (ET_0) plus
    /// SMPTE ST 2084 / PQ (ET_2), Static Metadata Type 1 (SM_0). The desired-content luminance tail
    /// holds the BUILT-IN defaults, used when the host reported no client volume; [`generate`]
    /// overwrites bytes 4..7 with the client display's coded volume otherwise.
    #[rustfmt::skip]
    const HDR_STATIC_METADATA_DB: [u8; 7] = [
        0xE6, // tag 0b111 (use-extended-tag) | length 6
        0x06, // extended tag: HDR Static Metadata
        0x05, // Supported EOTFs: ET_0 (traditional SDR) | ET_2 (SMPTE ST 2084 / PQ)
        0x01, // Supported Static Metadata Descriptors: SM_0 (Static Metadata Type 1)
        0x8A, // Desired Content Max Luminance      (code 138 ≈ 993 nits)
        0x60, // Desired Content Max Frame-avg Lum. (code  96 = 400 nits)
        0x12, // Desired Content Min Luminance      (code  18 ≈ 0.05 nits)
    ];

    /// The client display's luminance volume for the CTA HDR block — the
    /// [`crate::control::AddRequest`] luminance tail, same units. `max_nits == 0` means unknown (an
    /// SDR client, or an un-upgraded host whose short ADD zero-fills the tail) and keeps the
    /// built-in defaults.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct ClientLuminance {
        /// Peak luminance, nits. `0` = unknown → keep the built-in default block.
        pub max_nits: u32,
        /// Max frame-average luminance, nits. `0` = unknown ("no data" on the wire).
        pub max_frame_avg_nits: u32,
        /// Min luminance, milli-nits. `0` = unknown/true black ("no data" on the wire).
        pub min_millinits: u32,
    }

    /// Build the 256-byte EDID for the monitor identified by `serial`, with both block checksums
    /// recomputed — the serial patch at 0x0C and the CTA edits below both invalidate them.
    ///
    /// `lum` is the CLIENT display's luminance volume, coded into the HDR block's desired-content
    /// bytes so apps tone-map to the panel the stream lands on; all-zero keeps the built-in
    /// ~993-nit defaults. `preferred` is the session's `(width, height, refresh)`: it replaces the
    /// hard-coded 1080p60 preferred-timing DTD when it fits the encoding (4K120-class does not).
    /// The modes the OS OFFERS still come from the IddCx mode list, not this descriptor.
    #[must_use]
    pub fn generate(
        serial: u32,
        lum: ClientLuminance,
        preferred: Option<(u32, u32, u32)>,
    ) -> [u8; 256] {
        let mut edid = [0u8; 256];
        edid[..128].copy_from_slice(&BASE);
        edid[SERIAL_OFFSET..SERIAL_OFFSET + 4].copy_from_slice(&serial.to_le_bytes());
        if let Some(d) = preferred.and_then(|(w, h, r)| dtd(w, h, r)) {
            edid[54..72].copy_from_slice(&d);
        }
        edid[128..132].copy_from_slice(&CTA_HEADER);
        edid[132..136].copy_from_slice(&COLORIMETRY_DB);
        let mut hdr_db = HDR_STATIC_METADATA_DB;
        if lum.max_nits > 0 {
            let max_code = cta_max_luminance_code(lum.max_nits);
            hdr_db[4] = max_code;
            hdr_db[5] = if lum.max_frame_avg_nits > 0 {
                cta_max_luminance_code(lum.max_frame_avg_nits)
            } else {
                0 // "no data" — valid per CTA-861.3
            };
            hdr_db[6] = cta_min_luminance_code(lum.min_millinits, max_code);
        }
        edid[136..143].copy_from_slice(&hdr_db);
        fix_block_checksum(&mut edid, 0);
        fix_block_checksum(&mut edid, 128);
        edid
    }

    /// Read the per-monitor serial (base offset 0x0C, little-endian) out of an EDID the OS handed
    /// back, so a monitor-description callback can find the monitor it belongs to. Takes the full
    /// 256-byte EDID or just the 128-byte base block, and errors rather than panics on a short
    /// buffer so the caller can reject a malformed descriptor.
    pub fn get_serial(edid: &[u8]) -> Result<u32, core::array::TryFromSliceError> {
        let bytes: [u8; 4] = edid
            .get(SERIAL_OFFSET..SERIAL_OFFSET + 4)
            .unwrap_or(&[])
            .try_into()?;
        Ok(u32::from_le_bytes(bytes))
    }

    /// Set the trailing byte of the 128-byte block at `start` so the block's bytes sum to 0
    /// (mod 256) — the standard EDID block checksum, without which a parser rejects the block.
    fn fix_block_checksum(edid: &mut [u8], start: usize) {
        let sum = edid[start..start + 127]
            .iter()
            .fold(0u8, |acc, &b| acc.wrapping_add(b));
        edid[start + 127] = 0u8.wrapping_sub(sum);
    }
}

/// Virtual-monitor mode lists and OS identity for the `pf-vdisplay` driver: what a monitor
/// advertises, which id names it, and the timing numbers behind one `(width, height, refresh)`.
///
/// Lives here, not in the driver: the driver only builds under the WDK, so this arithmetic had
/// no test on any machine. Integer-only and free of OS types — the driver stamps the results
/// into its `wdk_sys` / `iddcx` structs and keeps nothing else.
pub mod vdisplay {
    use alloc::vec;
    use alloc::vec::Vec;

    /// One resolution with the refresh rates it supports.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Mode {
        pub width: u32,
        pub height: u32,
        pub refresh_rates: Vec<u32>,
    }

    /// A single `(width, height, refresh)` tuple — modes flattened across their refresh rates.
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct ModeItem {
        pub width: u32,
        pub height: u32,
        pub refresh_rate: u32,
    }

    /// Flatten a mode list into per-refresh-rate tuples (the order the mode DDIs emit).
    pub fn flatten(modes: &[Mode]) -> impl Iterator<Item = ModeItem> + '_ {
        modes.iter().flat_map(|m| {
            m.refresh_rates.iter().map(|&rr| ModeItem {
                width: m.width,
                height: m.height,
                refresh_rate: rr,
            })
        })
    }

    /// How many distinct resolutions a monitor's advertised list may accumulate (the requested
    /// head + history + the built-in fallbacks). Bounds the union growth across many resizes.
    pub const MODE_LIST_CAP: usize = 12;

    /// Append `from`'s modes to `into`, skipping resolutions already there, capped at
    /// [`MODE_LIST_CAP`] — the accumulate half of the driver's mode-union semantics. The OS pins
    /// a monitor's settable set at arrival, so a list may only ever grow.
    ///
    /// Dedupe is by `(width, height)` alone: a duplicate resolution is dropped WHOLE, refresh
    /// rates and all, never merged into the entry already present. The cap is checked before
    /// each candidate, so reaching it stops the merge rather than skipping one entry.
    pub fn union_modes(into: &mut Vec<Mode>, from: &[Mode]) {
        for m in from {
            if into.len() >= MODE_LIST_CAP {
                break;
            }
            if !into
                .iter()
                .any(|e| (e.width, e.height) == (m.width, m.height))
            {
                into.push(m.clone());
            }
        }
    }

    /// Fallback modes appended after the requested mode, so a topology change still has options.
    #[must_use]
    pub fn default_modes() -> Vec<Mode> {
        vec![
            Mode {
                width: 1920,
                height: 1080,
                refresh_rates: vec![60, 120],
            },
            Mode {
                width: 1280,
                height: 720,
                refresh_rates: vec![60],
            },
        ]
    }

    /// The numbers of a `DISPLAYCONFIG_VIDEO_SIGNAL_INFO`, without the OS struct. Both sync
    /// rationals have denominator 1, and total size equals active size (no fabricated blanking),
    /// so the driver stamps one `(width, height)` region into both size fields.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SignalInfo {
        /// Pixels per second, `refresh · width · height`. u64 because 8K240 alone is 7.96e9.
        pub pixel_rate: u64,
        /// `hSyncFreq` numerator, `refresh · height`, saturated into u32.
        pub h_sync_num: u32,
        /// `vSyncFreq` numerator — the refresh rate itself.
        pub v_sync_num: u32,
        /// The `AdditionalSignalInfo` union read as `videoStandard`: 255 (other), with the
        /// vSync frequency divider in bits 16..21.
        pub video_standard: u32,
    }

    /// THE signal description for both mode DDI families (IddSampleDriver-exact): pixel rate =
    /// `refresh · width · height`, integer sync rationals, total == active. Monitor (description)
    /// and target (scan-out) modes differ ONLY in `v_sync_freq_divider`, which the caller passes
    /// (0 / 1 per the DDI contract).
    ///
    /// Until 2026-07 the monitor side used the virtual-display-rs legacy math instead — a
    /// WIDTH-LESS pixel rate (`rr·(h+4)²+1000`) and a deliberately fractional vSync — so the OS
    /// saw two disagreeing descriptions of the same tuple, one physically meaningless.
    ///
    /// The hSync numerator is computed in u64 and saturated: an unchecked `refresh · height`
    /// past u32 would panic → abort the extern-"C" mode DDI in a debug build.
    #[must_use]
    pub fn signal_info(
        width: u32,
        height: u32,
        refresh_rate: u32,
        v_sync_freq_divider: u32,
    ) -> SignalInfo {
        SignalInfo {
            pixel_rate: u64::from(refresh_rate) * u64::from(width) * u64::from(height),
            h_sync_num: u32::try_from(u64::from(refresh_rate) * u64::from(height))
                .unwrap_or(u32::MAX),
            v_sync_num: refresh_rate,
            video_standard: 255 | (v_sync_freq_divider << 16),
        }
    }

    /// Resolve the id to name a new monitor by, given the ids currently `live`: honour the host's
    /// per-client `preferred` id when it is in `1..=15` (so the IddCx `ConnectorIndex` = id stays
    /// below `MaxMonitorsSupported` = 16) AND not live, so a client keeps a STABLE identity across
    /// reconnects and Windows reapplies its saved per-monitor DPI scaling.
    ///
    /// Otherwise fall back to [`alloc_monitor_id`]. A collision NEVER departs the live holder —
    /// that would tear down an unrelated client — so live ids stay distinct even against a host
    /// bug. `preferred == 0` (anonymous / TOFU / GameStream) always falls through to auto.
    #[must_use]
    pub fn resolve_id(live: &[u32], preferred: u32) -> u32 {
        if (1..=15).contains(&preferred) && !live.contains(&preferred) {
            preferred
        } else {
            alloc_monitor_id(live)
        }
    }

    /// The lowest id ≥ 1 not in `live`. Reusing freed ids (rather than a monotonic counter) keeps
    /// the connector index / EDID serial / container GUID bounded by the number of CONCURRENT
    /// monitors, so a fresh ADD reuses a departed monitor's OS target slot instead of orphaning it
    /// — the ghost accumulation that wedges ADD at 0x80070490.
    ///
    /// The search spans `1..=live.len() + 1`, where pigeonhole guarantees a free id. That bound
    /// is NOT clamped to 15: with 15 live ids the result is 16, one past the connector range
    /// [`resolve_id`] enforces for a preferred id.
    #[must_use]
    pub fn alloc_monitor_id(live: &[u32]) -> u32 {
        (1u32..=live.len() as u32 + 1)
            .find(|id| !live.contains(id))
            .unwrap_or(1)
    }

    /// A deterministic, monitor-unique container GUID (which groups targets into one physical
    /// device), derived from `id` so it is stable and collision-free without a random source.
    /// Returned as `(Data1, Data2, Data3, Data4)` like [`crate::interface_guid_fields`] — this
    /// crate is `no_std` and has no `GUID` type.
    #[must_use]
    pub const fn container_guid(id: u32) -> (u32, u16, u16, [u8; 8]) {
        (
            0x7066_7664u32.wrapping_add(id),
            0x7044,
            0x5350,
            [
                0xa1,
                0xb2,
                0xc3,
                0xd4,
                0xe5,
                0xf6,
                (id >> 8) as u8,
                id as u8,
            ],
        )
    }

    /// Sanity bounds for a mode the host requests over ADD / UPDATE_MODES — generous (any real
    /// client fits) but rejecting the zero and absurd values that would otherwise reach the EDID
    /// and [`signal_info`] math unchecked.
    #[must_use]
    pub fn valid_mode(width: u32, height: u32, refresh_hz: u32) -> bool {
        (1..=16384).contains(&width)
            && (1..=16384).contains(&height)
            && (1..=1000).contains(&refresh_hz)
    }
}

/// IDD-push frame transport: shared ring header, publish token, driver-status codes.
/// Textures are unnamed D3D11 keyed-mutex objects; the driver reaches them only through
/// handles duplicated into its process and delivered via [`crate::control::IOCTL_SET_FRAME_CHANNEL`].
/// Layout/contract only.
pub mod frame {
    use bytemuck::{Pod, Zeroable};

    /// Header magic (`"PFVD"` LE). The host stamps it last (after the ring textures exist) so the
    /// driver only attaches to a fully-published ring.
    pub const MAGIC: u32 = 0x4456_4650;
    /// Frame-plane version (independent bump of the header layout). v2 appended the stall-attribution
    /// telemetry tail (`drain_heartbeat_qpc`/`last_acquire_qpc`/`offered_total`); v3 appended the
    /// ring-health tail (state, capabilities, epochs, source sequence, terminal error, counters) —
    /// see [`VERSION_TELEMETRY`] and [`VERSION_HEALTH`] for the two compatibility gates.
    pub const VERSION: u32 = 3;
    /// The header version that grew the ring-health tail (immunity plan WP4). Reading or writing
    /// past [`HEADER_V2_SIZE`] needs BOTH gates — see [`v3_readable`]: the host-stamped `version`
    /// says the layout exists, the delivery's `header_bytes` says the section is that large. A v2
    /// section is never touched past byte 88; a v3 host may allocate the larger section for a v2
    /// driver, which reads only the prefix and writes no v3 field.
    pub const VERSION_HEALTH: u32 = 3;
    /// The v2 (telemetry-tail) header size — the prefix every driver since v2 understands.
    pub const HEADER_V2_SIZE: usize = 88;
    /// The v3 (ring-health-tail) header size — the section a v3 host allocates.
    pub const HEADER_V3_SIZE: usize = 152;

    /// Both endpoints may read/write the v3 tail only when the host stamped a v3 layout AND the
    /// delivery declared a section at least that large. One gate, shared, so neither side can
    /// guess: an old host leaves `header_bytes` zero (its request `_pad`), which fails here.
    #[must_use]
    pub const fn v3_readable(version: u32, header_bytes: u32) -> bool {
        version >= VERSION_HEALTH && header_bytes as usize >= HEADER_V3_SIZE
    }

    // Capability bits, stamped by each side into its own header word (`host_capabilities` /
    // `capabilities`). A transport or actuator activates only where BOTH sides agree
    // ([`negotiate`]); a mismatch never selects a protocol by guesswork.
    /// Understands the v3 ring-health tail.
    pub const CAP_RING_HEALTH_V3: u32 = 1 << 0;
    /// The driver keeps the ring endpoint across a swap-chain assignment (WP5).
    pub const CAP_ENDPOINT_SURVIVES_ASSIGNMENT: u32 = 1 << 1;
    /// CAS + shared-fence slot transport (WP7).
    pub const CAP_FENCE_RING: u32 = 1 << 2;
    /// The driver accepts a swap-chain reset actuator (WP13).
    pub const CAP_SWAPCHAIN_RESET: u32 = 1 << 3;
    /// The driver stamps `source_sequence` and `qpc_pts` (source present QPC) per real frame.
    pub const CAP_SOURCE_SEQUENCE_QPC: u32 = 1 << 4;

    /// The capabilities both sides advertise — the only ones either may act on.
    #[must_use]
    pub const fn negotiate(host: u32, driver: u32) -> u32 {
        host & driver
    }

    /// `SharedHeader::health_state` values (v3). The driver stores the state LAST, with Release,
    /// after the epoch/sequence fields it describes; a reader loads it with Acquire before
    /// trusting them, and re-reads it after to detect a torn snapshot ([`snapshot_consistent`]).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum HealthState {
        /// Header created, no publisher attached yet (also what a pre-v3 driver leaves: 0).
        Initializing = 0,
        /// A publisher is attached and the ring is live.
        Active = 1,
        /// The publisher retired its attachment (generation superseded / worker exit); a fresh
        /// attach is expected.
        Rebuilding = 2,
        /// The generation is poisoned (abandoned slot, fatal sync/device result). `terminal_error_*`
        /// name the cause. Only a rebuild helps.
        Dead = 3,
    }

    impl HealthState {
        /// Decode a stored word; unknown values read as [`Self::Dead`] — an unrecognised state is
        /// not one to keep streaming on.
        #[must_use]
        pub const fn from_u32(v: u32) -> Self {
            match v {
                0 => Self::Initializing,
                1 => Self::Active,
                2 => Self::Rebuilding,
                _ => Self::Dead,
            }
        }
    }

    /// `terminal_error_domain` values (v3), paired with a raw `terminal_error_code`.
    pub const ERR_DOMAIN_NONE: u32 = 0;
    /// Slot synchronization: an abandoned or failed keyed-mutex/fence operation (code = HRESULT).
    pub const ERR_DOMAIN_TRANSPORT: u32 = 1;
    /// The D3D device: removed/reset (code = `GetDeviceRemovedReason`).
    pub const ERR_DOMAIN_DEVICE: u32 = 2;

    /// Torn-read resistance for a v3 snapshot: the state word read before and after the epoch/
    /// sequence fields must match, or the snapshot spans a driver transition and is discarded.
    #[must_use]
    pub const fn snapshot_consistent(state_before: u32, state_after: u32) -> bool {
        state_before == state_after
    }
    /// Header version that grew the telemetry tail. Gated on the host-stamped `version` field, not
    /// mapping size: a v2 driver writes the tail only when `version >= VERSION_TELEMETRY` (a v1
    /// host mapped 64 bytes — never write past it). A v2 host reads `drain_heartbeat_qpc == 0` as
    /// pre-telemetry (same zero-means-absent as [`OPENED_DETAIL_LIVE`]). Neither side rejects the
    /// other's version — the tail is diagnostics.
    pub const VERSION_TELEMETRY: u32 = 2;
    /// Ring slots. Headroom so a 0 ms-timeout publish always finds a free slot while the host holds
    /// one across convert/copy + pipelined encode.
    pub const RING_LEN: u32 = 6;

    /// `driver_status` values the driver writes into the host header (logged on a timeout).
    pub const DRV_STATUS_NONE: u32 = 0;
    pub const DRV_STATUS_OPENED: u32 = 1;
    /// Could not open the host's textures — render-adapter mismatch. Detail carries the HRESULT.
    pub const DRV_STATUS_TEX_FAIL: u32 = 2;
    /// No `ID3D11Device1` to open shared resources.
    pub const DRV_STATUS_NO_DEVICE1: u32 = 3;
    /// Ring [`SharedHeader::target_id`] ≠ the monitor this delivery landed on. Fail-closed
    /// (`design/idd-push-security.md`); detail carries the target id the ring claims.
    pub const DRV_STATUS_BIND_FAIL: u32 = 4;

    /// Live `driver_status_detail` while [`DRV_STATUS_OPENED`]. Bit 31 (this constant) distinguishes
    /// a pre-detail driver (field = 0) from "zero frames offered". Bits 30..16: surfaces offered
    /// (15-bit, saturating). Bits 15..0: publishes dropped for a descriptor mismatch (16-bit,
    /// saturating). `offered == 0` → DWM never composed; `offered > 0` with `seq` still 0 → every
    /// compose was dropped mismatched (ring sized from a stale GDI mode).
    pub const OPENED_DETAIL_LIVE: u32 = 0x8000_0000;

    /// Pack the live OPENED diagnostic word; both counters saturate.
    #[must_use]
    pub const fn pack_opened_detail(offered: u32, mismatched: u32) -> u32 {
        let o = if offered > 0x7FFF { 0x7FFF } else { offered };
        let m = if mismatched > 0xFFFF {
            0xFFFF
        } else {
            mismatched
        };
        OPENED_DETAIL_LIVE | (o << 16) | m
    }

    /// Unpack → `(offered, mismatched)`. `None` when [`OPENED_DETAIL_LIVE`] was never stamped.
    #[must_use]
    pub const fn unpack_opened_detail(detail: u32) -> Option<(u32, u32)> {
        if detail & OPENED_DETAIL_LIVE == 0 {
            return None;
        }
        Some(((detail >> 16) & 0x7FFF, detail & 0xFFFF))
    }

    /// Shared metadata header. Atomic fields (`magic`, `latest`, `generation`) are accessed via
    /// each side's own atomic view over the mapping; this is the layout.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct SharedHeader {
        pub magic: u32,
        pub version: u32,
        /// Bumped on ring recreate (HDR-mode flip → new texture format + a fresh
        /// [`control::IOCTL_SET_FRAME_CHANNEL`](crate::control::IOCTL_SET_FRAME_CHANNEL)).
        /// A publish carries it so the host rejects a stale-ring publish.
        pub generation: u32,
        pub ring_len: u32,
        pub width: u32,
        pub height: u32,
        pub dxgi_format: u32,
        /// OS target id of the monitor this ring belongs to (former `_pad`, same offset).
        /// Host-stamped before the magic and never changed afterwards. Attach proceeds only when
        /// it equals the monitor's own target id ([`check_attach`]); mismatch → [`DRV_STATUS_BIND_FAIL`].
        pub target_id: u32,
        /// Driver-written after each copy; host loads `Acquire`. See [`FrameToken`].
        pub latest: u64,
        pub qpc_pts: u64,
        /// Adapter the swap-chain actually renders on (mismatch detection).
        pub driver_render_luid_low: u32,
        pub driver_render_luid_high: i32,
        /// Driver-written status. UMDF hides OutputDebugString and the restricted token blocks
        /// file writes, so this header is how the driver reports state.
        pub driver_status: u32,
        pub driver_status_detail: u32,
        /// QPC of the swap-chain worker's most recent drain-loop iteration, E_PENDING included.
        /// Relaxed stores; `0` = pre-v2 driver never wrote it. Fresh heartbeat + stale
        /// [`Self::last_acquire_qpc`] = worker running, DWM composing nothing; stale heartbeat =
        /// worker starved.
        pub drain_heartbeat_qpc: u64,
        /// QPC of the most recent successful swap-chain acquire — last instant DWM composed this
        /// display.
        pub last_acquire_qpc: u64,
        /// Wrapping count of surfaces offered to the publisher — full-width sibling of the packed
        /// 15-bit [`OPENED_DETAIL_LIVE`] counter, which saturates and cannot be delta'd over a stall.
        pub offered_total: u64,
        // ---- v3 ring-health tail (offset 88..152). Every write is gated on `v3_readable`. ----
        /// [`HealthState`], driver-written with Release AFTER the fields below it describes.
        pub health_state: u32,
        /// Driver capability bits (`CAP_*`), stamped at attach.
        pub capabilities: u32,
        /// Host capability bits (`CAP_*`), stamped before the magic.
        pub host_capabilities: u32,
        /// Bumped by the driver on every swap-chain assignment it attaches under.
        pub assignment_epoch: u32,
        /// Bumped by the driver on every D3D device creation — even on the same LUID (a TDR
        /// recreate mints a new epoch; LUID equality is never device-compatibility proof).
        pub device_epoch: u32,
        /// `ERR_DOMAIN_*` for a [`HealthState::Dead`] generation.
        pub terminal_error_domain: u32,
        /// Raw code for `terminal_error_domain` (HRESULT / removed reason).
        pub terminal_error_code: i32,
        pub _pad_v3: u32,
        /// Monotonic count of NEW source frames published (a stash republish does not advance it)
        /// — the driver-side twin of the host's provenance sequence.
        pub source_sequence: u64,
        /// QPC of the most recent successful publish.
        pub last_publish_qpc: u64,
        /// Wrapping counts of publishes that landed and frames dropped (busy/mismatch/fatal).
        pub published_total: u64,
        pub dropped_total: u64,
    }

    /// Why the publisher must not attach a delivered channel — the two [`check_attach`] rejects.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AttachReject {
        /// Magic missing, or the host recreated the ring before attach. Drop silently; no status.
        Stale,
        /// `SharedHeader::target_id` mismatch. Fail closed: write [`DRV_STATUS_BIND_FAIL`].
        BindMismatch,
    }

    /// Attach precondition: header `magic`/`generation`/`target_id` vs delivery generation and
    /// the monitor's own target id. Staleness is checked first — a superseded delivery's binding
    /// is meaningless, so it never false-alarms as a bind failure. Pure so the reject paths are
    /// unit-tested here (the driver workspace is `panic = "abort"`).
    pub fn check_attach(
        magic: u32,
        header_generation: u32,
        header_target_id: u32,
        delivery_generation: u32,
        monitor_target_id: u32,
    ) -> Result<(), AttachReject> {
        if magic != MAGIC || header_generation != delivery_generation {
            return Err(AttachReject::Stale);
        }
        if header_target_id != monitor_target_id {
            return Err(AttachReject::BindMismatch);
        }
        Ok(())
    }

    /// `SharedHeader.latest` token: `(generation << 40) | (seq << 8) | slot`.
    /// `generation` 24-bit, `seq` 32-bit, `slot` 8-bit. The generation tag lets the host reject a
    /// stale-ring publish so it never consumes an unwritten new-ring slot.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FrameToken {
        pub generation: u32,
        pub seq: u32,
        pub slot: u8,
    }

    impl FrameToken {
        /// Low 24 bits of `generation` are significant.
        pub const GENERATION_MASK: u32 = 0x00FF_FFFF;

        pub const fn pack(self) -> u64 {
            (((self.generation & Self::GENERATION_MASK) as u64) << 40)
                | (((self.seq as u64) & 0xFFFF_FFFF) << 8)
                | (self.slot as u64)
        }

        pub const fn unpack(v: u64) -> Self {
            Self {
                generation: ((v >> 40) as u32) & Self::GENERATION_MASK,
                seq: ((v >> 8) & 0xFFFF_FFFF) as u32,
                slot: (v & 0xFF) as u8,
            }
        }
    }

    // Both sides access these via raw atomic views; a same-size reorder silently corrupts.
    // `target_id` (former `_pad`) after `dxgi_format` is what 8-aligns `u64 latest` at offset 32.
    const _: () = {
        use core::mem::{offset_of, size_of};

        assert!(size_of::<SharedHeader>() == HEADER_V3_SIZE);
        assert!(offset_of!(SharedHeader, health_state) == HEADER_V2_SIZE);
        assert!(offset_of!(SharedHeader, capabilities) == 92);
        assert!(offset_of!(SharedHeader, host_capabilities) == 96);
        assert!(offset_of!(SharedHeader, assignment_epoch) == 100);
        assert!(offset_of!(SharedHeader, device_epoch) == 104);
        assert!(offset_of!(SharedHeader, terminal_error_domain) == 108);
        assert!(offset_of!(SharedHeader, terminal_error_code) == 112);
        assert!(offset_of!(SharedHeader, source_sequence) == 120);
        assert!(offset_of!(SharedHeader, last_publish_qpc) == 128);
        assert!(offset_of!(SharedHeader, published_total) == 136);
        assert!(offset_of!(SharedHeader, dropped_total) == 144);
        assert!(offset_of!(SharedHeader, magic) == 0);
        assert!(offset_of!(SharedHeader, version) == 4);
        assert!(offset_of!(SharedHeader, generation) == 8);
        assert!(offset_of!(SharedHeader, ring_len) == 12);
        assert!(offset_of!(SharedHeader, width) == 16);
        assert!(offset_of!(SharedHeader, height) == 20);
        assert!(offset_of!(SharedHeader, dxgi_format) == 24);
        assert!(offset_of!(SharedHeader, target_id) == 28);
        assert!(offset_of!(SharedHeader, latest) == 32);
        assert!(offset_of!(SharedHeader, qpc_pts) == 40);
        assert!(offset_of!(SharedHeader, driver_render_luid_low) == 48);
        assert!(offset_of!(SharedHeader, driver_render_luid_high) == 52);
        assert!(offset_of!(SharedHeader, driver_status) == 56);
        assert!(offset_of!(SharedHeader, driver_status_detail) == 60);
        assert!(offset_of!(SharedHeader, drain_heartbeat_qpc) == 64);
        assert!(offset_of!(SharedHeader, last_acquire_qpc) == 72);
        assert!(offset_of!(SharedHeader, offered_total) == 80);
    };

    /// The CAS + shared-fence slot transport (immunity plan WP7, decision D5; S2 GO on both
    /// vendors). Replaces the keyed mutex as the slot handshake — the sealed handle broker, the
    /// header and the textures stay as they are.
    ///
    /// Layout: a v4 header appends a [`SlotRecord`] table (one per [`RING_LEN`] slot) at
    /// [`SLOT_TABLE_OFFSET`]; the delivery carries two shared `ID3D11Fence` handles
    /// (`producer-ready`, `consumer-retire`) in [`SetFrameChannelRequestV2`](crate::control::
    /// SetFrameChannelRequestV2). Both endpoints may run the fence protocol only when
    /// [`v4_readable`] passes AND [`negotiate`] contains [`CAP_FENCE_RING`]; otherwise the ring is
    /// the keyed-mutex one (WP2 poison rules), which stays negotiable for one compatibility release.
    ///
    /// Protocol (`FREE -> WRITING -> PUBLISHED -> READING -> FREE`, every transition a CAS):
    /// the producer claims a FREE slot, else the OLDEST PUBLISHED (drop-oldest), else drops the
    /// frame — it never touches READING/WRITING and never CPU-waits. It GPU-waits the slot's
    /// `retire_value` on the retire fence, copies, signals `ready` with a new value, stamps
    /// `seq`/`ready_value`, then releases PUBLISHED. The consumer takes the NEWEST PUBLISHED by
    /// `seq` (dropping older ones — newest-wins, the S2 finding), CASes it READING, GPU-waits
    /// `ready_value`, queues its read, signals `retire` with a new value, stamps `retire_value`,
    /// then releases FREE. The pure claim/pick rules live here and are model-tested below.
    pub mod fence {
        use bytemuck::{Pod, Zeroable};

        use super::HEADER_V3_SIZE;

        const RING_LEN_USIZE: usize = super::RING_LEN as usize;

        /// The header version that appends the slot table.
        pub const VERSION_FENCE: u32 = 4;
        /// Where the slot table starts: right after the v3 tail (8-aligned).
        pub const SLOT_TABLE_OFFSET: usize = HEADER_V3_SIZE;
        /// One [`SlotRecord`] per slot.
        pub const SLOT_RECORD_SIZE: usize = 32;
        /// The v4 header size — a v4 host allocates this.
        pub const HEADER_V4_SIZE: usize = SLOT_TABLE_OFFSET + RING_LEN_USIZE * SLOT_RECORD_SIZE;

        /// Both endpoints may touch the slot table only when the host stamped a v4 layout AND the
        /// delivery declared a section at least that large (the v3 gate's shape, one version up).
        #[must_use]
        pub const fn v4_readable(version: u32, header_bytes: u32) -> bool {
            version >= VERSION_FENCE && header_bytes as usize >= HEADER_V4_SIZE
        }

        /// Slot states (`SlotRecord::state`).
        pub const FREE: u32 = 0;
        pub const WRITING: u32 = 1;
        pub const PUBLISHED: u32 = 2;
        pub const READING: u32 = 3;

        /// Per-slot protocol record in the shared header, accessed through atomic views like the
        /// rest of the header. The producer stamps `seq` and `ready_value` before its
        /// `WRITING -> PUBLISHED` release; the consumer stamps `retire_value` before its
        /// `READING -> FREE` release.
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
        pub struct SlotRecord {
            pub state: u32,
            pub _pad: u32,
            /// The PACKED [`FrameToken`](super::FrameToken) of the publish these pixels belong to
            /// (generation, seq, slot). The generation is what lets a consumer free a record a
            /// superseded ring left behind instead of consuming it by sequence number; the pure
            /// rules below compare the unpacked `seq` the caller hands them.
            pub seq: u64,
            /// Producer-ready fence value the consumer must GPU-wait before sampling.
            pub ready_value: u64,
            /// Consumer-retire fence value the producer must GPU-wait before overwriting.
            pub retire_value: u64,
        }

        const _: () = {
            use core::mem::size_of;
            assert!(size_of::<SlotRecord>() == SLOT_RECORD_SIZE);
            assert!(SLOT_TABLE_OFFSET % 8 == 0);
            assert!(HEADER_V4_SIZE == 152 + 6 * 32);
        };

        /// Byte offset of slot `i`'s record inside the header section.
        #[must_use]
        pub const fn slot_offset(i: usize) -> usize {
            SLOT_TABLE_OFFSET + i * SLOT_RECORD_SIZE
        }

        /// What the producer does with the next frame, from one scan of `(state, seq)` per slot.
        /// Pure: the CAS that commits the claim is the caller's (a lost race rescans).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Claim {
            /// Take this FREE slot.
            Free(usize),
            /// No FREE slot: overwrite this OLDEST PUBLISHED one (drop-oldest).
            Overwrite(usize),
            /// Everything is READING/WRITING: drop the frame, never wait.
            Drop,
        }

        /// Producer claim rule: first FREE slot; else the PUBLISHED slot with the lowest `seq`;
        /// else drop. READING and WRITING slots are never candidates.
        #[must_use]
        pub fn producer_claim(slots: &[(u32, u64)]) -> Claim {
            if let Some(i) = slots.iter().position(|&(s, _)| s == FREE) {
                return Claim::Free(i);
            }
            slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.0 == PUBLISHED)
                .min_by_key(|(_, slot)| slot.1)
                .map_or(Claim::Drop, |(i, _)| Claim::Overwrite(i))
        }

        /// What the consumer does, from one scan: take the newest PUBLISHED slot whose `seq` is
        /// above `last_delivered`, and name every PUBLISHED slot at or below it as stale (to be
        /// CASed straight to FREE — a stream must never show an older frame after a newer one).
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Pick {
            pub take: Option<(usize, u64)>,
            pub stale: alloc::vec::Vec<usize>,
        }

        #[must_use]
        pub fn consumer_pick(slots: &[(u32, u64)], last_delivered: u64) -> Pick {
            let mut take: Option<(usize, u64)> = None;
            let mut stale = alloc::vec::Vec::new();
            for (i, &(s, seq)) in slots.iter().enumerate() {
                if s != PUBLISHED {
                    continue;
                }
                if seq <= last_delivered {
                    stale.push(i);
                } else if take.is_none_or(|(_, t)| seq > t) {
                    take = Some((i, seq));
                }
            }
            Pick { take, stale }
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use crate::control::{SetFrameChannelRequest, SetFrameChannelRequestV2};
            use alloc::vec::Vec;

            #[test]
            fn v4_layout_and_gate() {
                assert_eq!(HEADER_V4_SIZE, 344);
                assert_eq!(slot_offset(0), 152);
                assert_eq!(slot_offset(5), 152 + 5 * 32);
                assert!(v4_readable(4, 344));
                assert!(!v4_readable(3, 344), "a v3 host never stamped a slot table");
                assert!(
                    !v4_readable(4, 152),
                    "a v4 version on a v3-sized section is not enough"
                );
                assert!(v4_readable(5, 4096));
                // The v2 request is the v1 request as an exact prefix.
                assert_eq!(core::mem::offset_of!(SetFrameChannelRequestV2, v1), 0);
                assert_eq!(
                    core::mem::offset_of!(SetFrameChannelRequestV2, ready_fence_handle),
                    core::mem::size_of::<SetFrameChannelRequest>()
                );
                assert_eq!(
                    core::mem::size_of::<SetFrameChannelRequestV2>(),
                    core::mem::size_of::<SetFrameChannelRequest>() + 16
                );
            }

            #[test]
            fn producer_prefers_free_then_oldest_published_and_never_reading() {
                assert_eq!(producer_claim(&[(PUBLISHED, 9), (FREE, 0)]), Claim::Free(1));
                assert_eq!(
                    producer_claim(&[(PUBLISHED, 9), (READING, 3), (PUBLISHED, 7)]),
                    Claim::Overwrite(2)
                );
                assert_eq!(producer_claim(&[(READING, 1), (WRITING, 2)]), Claim::Drop);
                assert_eq!(producer_claim(&[]), Claim::Drop);
            }

            #[test]
            fn consumer_takes_newest_and_names_older_publishes_stale() {
                let p = consumer_pick(
                    &[(PUBLISHED, 5), (READING, 6), (PUBLISHED, 8), (FREE, 0)],
                    5,
                );
                assert_eq!(p.take, Some((2, 8)));
                assert_eq!(p.stale, alloc::vec![0], "seq 5 was already delivered");
                let p = consumer_pick(&[(PUBLISHED, 3)], 7);
                assert_eq!((p.take, p.stale), (None, alloc::vec![0]));
                assert_eq!(consumer_pick(&[(FREE, 0); 6], 0).take, None);
            }

            /// Randomized two-party trace over the pure rules with a pixel model: the producer
            /// writes `seq` into the slot's pixels between its WRITING and PUBLISHED steps, the
            /// consumer reads them after READING. Proves the D5 invariants the driver/host arms
            /// inherit: a READING slot is never selected, a delivered frame's pixels always carry
            /// the seq the record names (no relabel), delivery is monotonic, and the producer never
            /// waits (every step makes progress or drops).
            #[test]
            fn interleaved_trace_never_relabels_or_touches_a_reading_slot() {
                struct Model {
                    state: [u32; 6],
                    seq: [u64; 6],
                    pixels: [u64; 6],
                    writing: Option<usize>,
                    reading: Option<(usize, u64)>,
                    next_seq: u64,
                    last_delivered: u64,
                    delivered: u64,
                    dropped: u64,
                    overwritten: u64,
                }
                let mut m = Model {
                    state: [FREE; 6],
                    seq: [0; 6],
                    pixels: [0; 6],
                    writing: None,
                    reading: None,
                    next_seq: 0,
                    last_delivered: 0,
                    delivered: 0,
                    dropped: 0,
                    overwritten: 0,
                };
                let mut rng = 0x9E37_79B9_7F4A_7C15u64;
                let mut next = || {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    rng
                };
                for _ in 0..200_000 {
                    let r = next();
                    let view: Vec<(u32, u64)> = (0..6).map(|i| (m.state[i], m.seq[i])).collect();
                    match r % 4 {
                        // Producer step 1: claim.
                        0 if m.writing.is_none() => match producer_claim(&view) {
                            Claim::Free(i) | Claim::Overwrite(i) => {
                                assert_ne!(m.state[i], READING, "claimed a READING slot");
                                assert_ne!(m.state[i], WRITING, "claimed a WRITING slot");
                                if m.state[i] == PUBLISHED {
                                    m.overwritten += 1;
                                }
                                m.state[i] = WRITING;
                                m.writing = Some(i);
                            }
                            Claim::Drop => m.dropped += 1,
                        },
                        // Producer step 2: copy + publish.
                        0 | 1 => {
                            if let Some(i) = m.writing.take() {
                                m.next_seq += 1;
                                m.pixels[i] = m.next_seq;
                                m.seq[i] = m.next_seq;
                                m.state[i] = PUBLISHED;
                            }
                        }
                        // Consumer step 1: pick + claim.
                        2 if m.reading.is_none() => {
                            let p = consumer_pick(&view, m.last_delivered);
                            for i in p.stale {
                                assert_eq!(m.state[i], PUBLISHED);
                                m.state[i] = FREE;
                            }
                            if let Some((i, seq)) = p.take {
                                assert!(seq > m.last_delivered, "non-monotonic pick");
                                m.state[i] = READING;
                                m.reading = Some((i, seq));
                            }
                        }
                        // Consumer step 2: read pixels + release.
                        _ => {
                            if let Some((i, seq)) = m.reading.take() {
                                assert_eq!(m.state[i], READING, "someone touched a READING slot");
                                assert_eq!(
                                    m.pixels[i], seq,
                                    "pixels relabelled under the consumer"
                                );
                                m.last_delivered = seq;
                                m.delivered += 1;
                                m.state[i] = FREE;
                            }
                        }
                    }
                }
                assert!(
                    m.delivered > 10_000,
                    "the trace must actually deliver frames"
                );
                assert!(m.overwritten > 0, "drop-oldest must have exercised");
                assert!(m.dropped < m.next_seq, "the producer must not be starved");
            }
        }
    }
}

/// Protocol v7: the driver encodes, and the host reads access units instead of pixels.
///
/// Replaces the pixel ring ([`frame`](crate::frame)) rather than joining it — one transport, as
/// `design/windows-video-plane-overhaul.md` §1.1 decided. The host still owns the memory: it
/// creates an unnamed section plus a ready event, duplicates both into WUDFHost and delivers the
/// values over [`IOCTL_SET_ENCODE`], which also carries codec, mode, bitrate, HDR metadata and an
/// ordered backend preference list. The driver opens the first backend that works and answers with
/// [`SetEncodeReply`] — the backend that took, its [`EncoderCapsWire`] and the applied bitrate, or
/// a named failure. No silent fallback.
///
/// Steady state lives in [`au`]: a 128-byte header, a 16-entry slot table and a bitstream heap the
/// encode thread writes into. Publishing reuses the ring's [`FrameToken`], so the host takes a slot
/// only under a generation check. Runtime control — keyframe, RFI, bitrate, HDR metadata, reset,
/// flush — travels as one-shot [`IOCTL_ENCODE_CTL`] calls on the framework queue that already
/// carries `PING`.
///
/// Types and layout only. [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION) still names v6; the bump
/// and the ring's removal are the Phase 3.5 cutover.
pub mod encode {
    use super::ctl_code;
    use bytemuck::{Pod, Zeroable};

    /// The publish token is the ring's, unchanged: `(generation << 40) | (seq << 8) | slot`.
    /// `generation` is bumped on every [`IOCTL_SET_ENCODE`], so a publish an old encoder left in
    /// [`au::AuHeader::latest`] is rejected instead of consumed.
    pub use crate::frame::FrameToken;

    /// Open the encoder for one monitor and adopt its AU section + ready event. Input
    /// [`SetEncodeRequest`], output [`SetEncodeReply`]. A resolution, codec or HDR change is a new
    /// SET_ENCODE — the same tear-down-and-rebuild the host does today.
    pub const IOCTL_SET_ENCODE: u32 = ctl_code(0x90C);
    /// One-shot control on the live encoder. Input [`EncodeCtlRequest`], no output.
    pub const IOCTL_ENCODE_CTL: u32 = ctl_code(0x90D);

    /// [`IOCTL_SET_ENCODE`] input. `section` and `event` are handle VALUES already duplicated into
    /// WUDFHost, adopt-on-success-only exactly as
    /// [`SetFrameChannelRequest`](crate::control::SetFrameChannelRequest): the driver owns and
    /// closes them IFF the IOCTL succeeds, and the host reaps with `DUPLICATE_CLOSE_SOURCE` on any
    /// error. Closing on error double-closes a possibly-reused handle value.
    ///
    /// Everything the backend needs to open travels in one request, so opening is not a
    /// negotiation: the driver walks `backends` in order and reports what took.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetEncodeRequest {
        /// OS target id of the monitor to encode (the [`AddReply`](crate::control::AddReply) one).
        pub target_id: u32,
        /// Aligns `section` to 8 (Pod forbids implicit padding).
        pub _pad: u32,
        /// AU section mapping handle VALUE; the driver maps it and writes [`au::AuHeader`].
        pub section: u64,
        /// Event handle VALUE the driver signals after each publish.
        pub event: u64,
        /// Bytes the host allocated for the section — [`au::section_bytes`].
        pub section_bytes: u32,
        /// 1 H264, 2 HEVC, 3 AV1, 4 PyroWave — the
        /// [`EncodeProbeRequest`](crate::control::EncodeProbeRequest) numbering.
        pub codec: u32,
        /// `0` = 4:2:0, `1` = 4:4:4. Not the H.264/HEVC `chroma_format_idc`.
        pub chroma: u32,
        /// Bits per component the encoder emits: 8 or 10.
        pub bit_depth: u32,
        pub width: u32,
        pub height: u32,
        /// Target frame rate; with `bitrate_kbps` it sizes the heap ([`au::heap_bytes_for`]).
        pub fps: u32,
        pub bitrate_kbps: u32,
        /// `1` = PQ BT.2020 stream and `hdr_meta` is valid.
        pub hdr: u32,
        /// `pf_frame::HdrMeta` as its 28 raw bytes — opaque here, this crate has no HDR types.
        pub hdr_meta: [u8; 28],
        /// Slice-chunk target for `Encoder::set_wire_chunking`; `0` = publish whole AUs.
        pub wire_chunk_bytes: u32,
        /// First `wire_seq` the driver stamps, so the host's `au_seq` domain survives a DriverCycle.
        pub wire_seq_base: u32,
        /// Ordered preference, 0-terminated: 1 NVENC, 2 AMF, 3 QSV, 4 PyroWave.
        pub backends: [u32; 4],
        /// Reserved; send `0`.
        pub flags: u32,
        /// Pads the struct to its 8-byte alignment (Pod forbids implicit tail padding).
        pub _pad_tail: u32,
    }

    /// `pf_encode_win::EncoderCaps` as plain integers — this crate cannot depend on the encoder
    /// crate, which builds only in the driver's graph. Each `bool` there is `0`/`1` here; the
    /// driver fills this from `Encoder::caps()` after the open and the host maps it straight back,
    /// so the session routes by query rather than by a `false` default.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct EncoderCapsWire {
        /// `supports_rfi`: `invalidate_ref_frames` can succeed; else the host keyframes on loss.
        pub supports_rfi: u32,
        /// `chroma_444`: the opened encoder emits 4:4:4. Cross-check against the request's `chroma`.
        pub chroma_444: u32,
        /// `intra_refresh`: a moving intra band instead of periodic IDRs.
        pub intra_refresh: u32,
        /// `intra_refresh_recovery`: the wave has a decoder-visible clean point, so freezes lift.
        pub intra_refresh_recovery: u32,
        /// `intra_refresh_period`: wave length in frames; `0` when the wave is off.
        pub intra_refresh_period: u32,
        /// `blends_cursor`: the encoder composited the pointer, so the host must not.
        pub blends_cursor: u32,
    }

    /// [`SetEncodeReply::status`]: an encoder is open.
    pub const SET_ENCODE_OK: u32 = 0;
    /// No arrived monitor has the request's `target_id`.
    pub const SET_ENCODE_NO_MONITOR: u32 = 1;
    /// The section did not pass [`au::au_readable`], or is smaller than its header claims.
    pub const SET_ENCODE_BAD_SECTION: u32 = 2;
    /// The monitor has no render device yet: no swap-chain has been assigned since arrival.
    pub const SET_ENCODE_NO_DEVICE: u32 = 3;
    /// Every backend in the list refused; `error` and `name` are the last one's.
    pub const SET_ENCODE_NO_BACKEND: u32 = 4;
    /// The pool or its converters could not be built on the render device.
    pub const SET_ENCODE_POOL: u32 = 5;
    /// The encode thread could not be started.
    pub const SET_ENCODE_THREAD: u32 = 6;
    /// The open did not finish within the driver's bound; the thread was abandoned.
    pub const SET_ENCODE_TIMEOUT: u32 = 7;

    /// [`IOCTL_SET_ENCODE`] output. `status` is the driver's own failure domain, not an HRESULT:
    /// [`SET_ENCODE_OK`] means an encoder is open, anything else means none is and the session
    /// ends with a structured error — `error` carries the backend's raw code and `name` a short
    /// NUL-padded tag (the driver log has the rest). `backend_opened` names the entry from
    /// [`SetEncodeRequest::backends`] that took, never a guess.
    ///
    /// The IOCTL itself completes successfully whenever the request was well-formed and named a
    /// monitor; from that point the driver owns the two handles and closes them itself when
    /// `status` is non-zero. Only an NTSTATUS failure leaves them for the host to reap.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct SetEncodeReply {
        /// One of the `SET_ENCODE_*` codes.
        pub status: u32,
        /// 1 NVENC, 2 AMF, 3 QSV, 4 PyroWave; `0` when `status` is non-zero.
        pub backend_opened: u32,
        /// What the opened backend can actually do.
        pub caps: EncoderCapsWire,
        /// Bitrate the backend accepted — may differ from what was asked.
        pub applied_bitrate_kbps: u32,
        /// Raw backend code for a non-zero `status` (HRESULT, NVENC status, AMF result).
        pub error: i32,
        /// Short NUL-padded tag: the backend name on success, the failing stage otherwise.
        pub name: [u8; 32],
    }

    /// [`EncodeCtlRequest::op`]: force the next submitted frame to an IDR.
    pub const ENCODE_CTL_REQUEST_KEYFRAME: u32 = 1;
    /// Invalidate reference frames `arg0..=arg1` in the host's wire-index domain (RFI).
    pub const ENCODE_CTL_INVALIDATE_REF_FRAMES: u32 = 2;
    /// Distrust every reference; the next AU is a clean recovery anchor.
    pub const ENCODE_CTL_DISTRUST_REFERENCES: u32 = 3;
    /// Reconfigure the bitrate to `arg0` kbps without reopening the backend.
    pub const ENCODE_CTL_RECONFIGURE_BITRATE: u32 = 4;
    /// Replace the HDR mastering metadata from `payload` (28 `pf_frame::HdrMeta` bytes).
    pub const ENCODE_CTL_SET_HDR_META: u32 = 5;
    /// Detach the wedged encode thread, open a fresh encoder, restart `wire_seq` at `arg0`.
    pub const ENCODE_CTL_RESET: u32 = 6;
    /// Push the encoder's in-flight AUs into the section.
    pub const ENCODE_CTL_FLUSH: u32 = 7;

    /// [`IOCTL_ENCODE_CTL`] input: one op against one monitor's live encoder. Unused `arg*` /
    /// `payload` bytes are zero. The ops are the `Encoder` trait calls the stream loop already
    /// makes locally on Linux, forwarded by a control proxy — so the wire shape is deliberately
    /// flat. A command ring with a doorbell is the upgrade path if measured IOCTL latency hurts
    /// RFI recovery; not before.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct EncodeCtlRequest {
        /// OS target id of the monitor whose encoder this addresses.
        pub target_id: u32,
        /// One `ENCODE_CTL_*` op.
        pub op: u32,
        /// First argument: reference-range start, bitrate kbps, or new `wire_seq_base`.
        pub arg0: u32,
        /// Second argument: reference-range end (inclusive) for
        /// [`ENCODE_CTL_INVALIDATE_REF_FRAMES`].
        pub arg1: u32,
        /// [`ENCODE_CTL_SET_HDR_META`] payload: 28 `pf_frame::HdrMeta` bytes.
        pub payload: [u8; 28],
    }

    /// The AU section: header, a fixed slot table, and a bitstream heap, in one host-created
    /// mapping the driver writes and the host reads.
    ///
    /// The encode thread is the only writer. It copies an access unit — or one slice chunk of one,
    /// when the host asked for wire chunking — into the heap, fills a [`FREE`] slot, publishes by
    /// storing a packed [`FrameToken`] into [`AuHeader::latest`], and signals the ready event. The
    /// host loads `latest`, checks the generation, takes the slot and releases it back to `FREE`.
    /// Sixteen slots is where back-pressure lands: with none free the encode thread skips the next
    /// pool slot, so the drop falls on pixels, which are free to drop, and never on an access unit,
    /// which is not.
    ///
    /// The host sizes the heap from the session's peak bitrate ([`heap_bytes_for`]) and the section
    /// from the heap ([`section_bytes`]), both before
    /// [`IOCTL_SET_ENCODE`](super::IOCTL_SET_ENCODE). A reader trusts no field here until
    /// [`au_readable`] passes.
    pub mod au {
        use bytemuck::{Pod, Zeroable};

        /// Header magic (`"PFAU"` LE), stamped by the host before the handle is delivered.
        pub const AU_MAGIC: u32 = 0x5541_4650;
        /// AU-section layout version; moves with `PROTOCOL_VERSION` v7.
        pub const AU_VERSION: u32 = 7;
        /// Slots in the table. Fixed: the host allocates exactly this many.
        pub const AU_SLOTS: u32 = 16;
        /// [`AuHeader`] size, and therefore where the slot table starts.
        pub const AU_HEADER_SIZE: usize = 128;
        /// [`AuSlot`] size.
        pub const AU_SLOT_SIZE: usize = 32;
        /// Byte offset of the slot table inside the section.
        pub const SLOT_TABLE_OFFSET: usize = AU_HEADER_SIZE;
        /// Byte offset of the heap: past the header and the whole table, still 64-byte aligned so
        /// heap writes never share a cache line with a slot record the host is polling.
        pub const HEAP_OFFSET: usize = SLOT_TABLE_OFFSET + AU_SLOTS as usize * AU_SLOT_SIZE;

        /// Heap sizes round up to this.
        pub const HEAP_GRANULE: u32 = 64 * 1024;
        /// Heap floor: 16 slots need room for 16 access units whatever the bitrate implies.
        pub const HEAP_MIN_BYTES: u32 = 1024 * 1024;
        /// Heap cap. Past this the session is misconfigured, not bandwidth-hungry.
        pub const HEAP_MAX_BYTES: u32 = 64 * 1024 * 1024;
        /// Section page alignment ([`section_bytes`]).
        pub const SECTION_ALIGN: u32 = 4096;

        /// [`AuHeader::encoder_state`]: no encoder — before the first SET_ENCODE, or between
        /// sessions. The pool keeps its stashed slot; nothing is published.
        pub const ENCODER_CLOSED: u32 = 0;
        /// A backend is open and the encode thread is waiting on pool slots.
        pub const ENCODER_OPEN: u32 = 1;
        /// Access units are flowing.
        pub const ENCODER_ENCODING: u32 = 2;
        /// A backend call has not returned. `source_seq` advances, `last_au_qpc` does not; the host
        /// answers with [`ENCODE_CTL_RESET`](super::ENCODE_CTL_RESET) and `detached` gains one.
        pub const ENCODER_WEDGED: u32 = 3;

        /// [`AuSlot::flags`], mirroring `pf_encode_win::AuChunk`: opens an access unit. AU metadata
        /// is authoritative on this slot — the host opens the wire frame from it.
        pub const AU_FIRST: u32 = 1 << 0;
        /// Closes the access unit and releases the encoder's in-flight slot.
        pub const AU_LAST: u32 = 1 << 1;
        /// IDR; sets the client's SOF/keyframe wire flags.
        pub const AU_KEYFRAME: u32 = 1 << 2;
        /// A clean picture after RFI — the client lifts its freeze here without waiting for an IDR.
        pub const AU_RECOVERY_ANCHOR: u32 = 1 << 3;
        /// The AU's chunks are cut on the codec's own window boundaries (PyroWave); the host
        /// forwards it as the wire's chunk-aligned user flag.
        pub const AU_CHUNK_ALIGNED: u32 = 1 << 4;

        /// [`AuSlot::state`]: the encode thread may take this slot. Same FREE/PUBLISHED/READING
        /// vocabulary as [`frame::fence`](crate::frame::fence), but NOT its values — there is no
        /// WRITING here, because the encode thread fills the heap before it claims a slot.
        pub const FREE: u32 = 0;
        /// Written and published; the host may take it.
        pub const PUBLISHED: u32 = 1;
        /// The host is copying the bytes out; the encode thread must not touch it.
        pub const READING: u32 = 2;

        /// Section header. The host stamps the layout fields and the magic last, before delivering
        /// the handle; the driver owns everything from `latest` down and writes it through atomic
        /// views over the mapping, `latest` with Release after the slot it names is complete.
        /// Telemetry here is what the classifier reads instead of inferring a stall from timers:
        /// `source_seq` moving while `last_au_qpc` stands still is a wedged encoder, and both
        /// standing still is a display composing nothing.
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
        pub struct AuHeader {
            /// [`AU_MAGIC`], host-stamped last.
            pub magic: u32,
            /// [`AU_VERSION`], host-stamped.
            pub version: u32,
            /// Host-stamped [`HEAP_OFFSET`].
            pub heap_offset: u32,
            /// Host-stamped heap size — [`heap_bytes_for`].
            pub heap_bytes: u32,
            /// Host-stamped [`SLOT_TABLE_OFFSET`].
            pub slot_table_offset: u32,
            /// Host-stamped [`AU_SLOTS`].
            pub slot_count: u32,
            /// The publish cell: a packed [`FrameToken`](super::FrameToken).
            pub latest: u64,
            /// Bumped by the driver on every SET_ENCODE; a publish carries it.
            pub generation: u32,
            /// Echo of [`SetEncodeRequest::wire_seq_base`](super::SetEncodeRequest::wire_seq_base).
            pub wire_seq_base: u32,
            /// One of the `ENCODER_*` states.
            pub encoder_state: u32,
            /// Encode threads abandoned after a wedge. Two is the `DriverCycle` threshold.
            pub detached: u32,
            /// QPC of the most recent publish — the stall clock the classifier reads.
            pub last_au_qpc: u64,
            /// QPC of the drain worker's most recent pass, stored after `FinishedProcessingFrame`.
            pub drain_heartbeat_qpc: u64,
            /// Frames the drain worker handed the pool — DWM's cadence, ahead of the encoder.
            pub source_seq: u64,
            /// Frames dropped at the pool or skipped for a full slot table.
            pub dropped_total: u64,
            /// Access units published.
            pub published_total: u64,
            /// Driver status word, as the ring header's — how a driver with no debugger reports.
            pub driver_status: u32,
            /// Raw detail for `driver_status`.
            pub driver_status_detail: u32,
            /// Pads the header to [`AU_HEADER_SIZE`]; zero.
            pub _reserved: [u8; 32],
        }

        /// One slot: where an access unit (or one chunk of one) sits in the heap, and what the host
        /// stamps on the wire frame. `offset`/`len` are the driver's to choose, so a reader
        /// bounds-checks them against `heap_bytes` before copying. `wire_seq` continues the host's
        /// `au_seq` domain from
        /// [`SetEncodeRequest::wire_seq_base`](super::SetEncodeRequest::wire_seq_base);
        /// `source_seq` names the frame it encodes, which is how a dropped frame stays visible.
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
        pub struct AuSlot {
            /// Byte offset from the start of the section (not from `heap_offset`).
            pub offset: u32,
            /// Bytes of bitstream.
            pub len: u32,
            /// Wire frame index the packetizer stamps.
            pub wire_seq: u32,
            /// Low half of the source frame counter this AU encodes.
            pub source_seq: u32,
            /// Source present QPC — the ground truth the classifier reads, not a timer inference.
            pub qpc_pts: u64,
            /// `AU_*` flag bits.
            pub flags: u32,
            /// [`FREE`], [`PUBLISHED`] or [`READING`].
            pub state: u32,
        }

        /// Heap bytes for a session: four frames at `max_bitrate_kbps`, doubled for burst, rounded
        /// up to [`HEAP_GRANULE`] and clamped into [`HEAP_MIN_BYTES`]..=[`HEAP_MAX_BYTES`].
        ///
        /// Four frames is the slot table's working set while the host drains; the doubling pays for
        /// an IDR several times the average AU. 400 Mbps at 60 fps lands at 6.4 MiB, inside the
        /// 8 MiB the plan budgets, and the same bitrate at 120 fps needs half of it. `fps == 0`
        /// reads as 1 rather than dividing by zero.
        #[must_use]
        pub const fn heap_bytes_for(max_bitrate_kbps: u32, fps: u32) -> u32 {
            let fps = if fps == 0 { 1 } else { fps as u64 };
            let per_second = (max_bitrate_kbps as u64) * 1000 / 8;
            let granule = HEAP_GRANULE as u64;
            let rounded = (8 * (per_second / fps)).div_ceil(granule) * granule;
            if rounded < HEAP_MIN_BYTES as u64 {
                HEAP_MIN_BYTES
            } else if rounded > HEAP_MAX_BYTES as u64 {
                HEAP_MAX_BYTES
            } else {
                rounded as u32
            }
        }

        /// Section bytes for a heap: header, slot table and heap, rounded up to a 4 KiB page. The
        /// host allocates exactly this and sends it as
        /// [`SetEncodeRequest::section_bytes`](super::SetEncodeRequest::section_bytes).
        #[must_use]
        pub const fn section_bytes(heap_bytes: u32) -> u32 {
            let align = SECTION_ALIGN as u64;
            let total = HEAP_OFFSET as u64 + heap_bytes as u64;
            (total.div_ceil(align) * align) as u32
        }

        /// Byte offset of slot `i`'s record inside the section.
        #[must_use]
        pub const fn slot_offset(i: usize) -> usize {
            SLOT_TABLE_OFFSET + i * AU_SLOT_SIZE
        }

        /// Whether a mapped header may be read past its magic: the host's magic, this version, the
        /// layout constants both sides already agree on, and a heap within the caps. Every other
        /// field is an index a writer chose, so `false` means touch nothing — the fail-closed rule
        /// the ring's `check_attach` follows.
        #[must_use]
        pub const fn au_readable(header: &AuHeader) -> bool {
            header.magic == AU_MAGIC
                && header.version == AU_VERSION
                && header.slot_count == AU_SLOTS
                && header.slot_table_offset as usize == SLOT_TABLE_OFFSET
                && header.heap_offset as usize == HEAP_OFFSET
                && header.heap_bytes >= HEAP_MIN_BYTES
                && header.heap_bytes <= HEAP_MAX_BYTES
        }

        // Layout crosses the process boundary; Pod rejects internal padding and these pin the
        // externally-visible sizes, so a same-size field reorder is a compile error.
        const _: () = {
            use core::mem::{offset_of, size_of};

            assert!(size_of::<AuHeader>() == AU_HEADER_SIZE);
            assert!(offset_of!(AuHeader, magic) == 0);
            assert!(offset_of!(AuHeader, version) == 4);
            assert!(offset_of!(AuHeader, heap_offset) == 8);
            assert!(offset_of!(AuHeader, heap_bytes) == 12);
            assert!(offset_of!(AuHeader, slot_table_offset) == 16);
            assert!(offset_of!(AuHeader, slot_count) == 20);
            assert!(offset_of!(AuHeader, latest) == 24);
            assert!(offset_of!(AuHeader, generation) == 32);
            assert!(offset_of!(AuHeader, wire_seq_base) == 36);
            assert!(offset_of!(AuHeader, encoder_state) == 40);
            assert!(offset_of!(AuHeader, detached) == 44);
            assert!(offset_of!(AuHeader, last_au_qpc) == 48);
            assert!(offset_of!(AuHeader, drain_heartbeat_qpc) == 56);
            assert!(offset_of!(AuHeader, source_seq) == 64);
            assert!(offset_of!(AuHeader, dropped_total) == 72);
            assert!(offset_of!(AuHeader, published_total) == 80);
            assert!(offset_of!(AuHeader, driver_status) == 88);
            assert!(offset_of!(AuHeader, driver_status_detail) == 92);
            assert!(offset_of!(AuHeader, _reserved) == 96);

            assert!(size_of::<AuSlot>() == AU_SLOT_SIZE);
            assert!(offset_of!(AuSlot, offset) == 0);
            assert!(offset_of!(AuSlot, len) == 4);
            assert!(offset_of!(AuSlot, wire_seq) == 8);
            assert!(offset_of!(AuSlot, source_seq) == 12);
            assert!(offset_of!(AuSlot, qpc_pts) == 16);
            assert!(offset_of!(AuSlot, flags) == 24);
            assert!(offset_of!(AuSlot, state) == 28);

            assert!(HEAP_OFFSET == 640 && HEAP_OFFSET % 64 == 0);
            assert!(SLOT_TABLE_OFFSET % 8 == 0);
        };
    }

    // Same reason as the ring's asserts: the IOCTL buffers are raw bytes on the far side.
    const _: () = {
        use core::mem::{offset_of, size_of};

        assert!(size_of::<SetEncodeRequest>() == 120);
        assert!(offset_of!(SetEncodeRequest, target_id) == 0);
        assert!(offset_of!(SetEncodeRequest, section) == 8);
        assert!(offset_of!(SetEncodeRequest, event) == 16);
        assert!(offset_of!(SetEncodeRequest, hdr_meta) == 60);
        assert!(offset_of!(SetEncodeRequest, backends) == 96);
        assert!(offset_of!(SetEncodeRequest, flags) == 112);

        assert!(size_of::<EncoderCapsWire>() == 24);
        assert!(size_of::<SetEncodeReply>() == 72);
        assert!(offset_of!(SetEncodeReply, caps) == 8);
        assert!(offset_of!(SetEncodeReply, applied_bitrate_kbps) == 32);
        assert!(offset_of!(SetEncodeReply, error) == 36);
        assert!(offset_of!(SetEncodeReply, name) == 40);

        assert!(size_of::<EncodeCtlRequest>() == 44);
        assert!(offset_of!(EncodeCtlRequest, op) == 4);
        assert!(offset_of!(EncodeCtlRequest, arg0) == 8);
        assert!(offset_of!(EncodeCtlRequest, arg1) == 12);
        assert!(offset_of!(EncodeCtlRequest, payload) == 16);
    };
}

/// Gamepad shared-memory layouts (host ↔ UMDF drivers `pf_xusb` / `pf_gamepad`).
///
/// Sealed channel (`design/gamepad-channel-sealing.md`): the host creates the DATA section
/// ([`XusbShm`]/[`PadShm`]) unnamed (SYSTEM-only DACL) and duplicates its handle into WUDFHost;
/// only the tiny [`PadBootstrap`] mailbox stays named. `Pod` + `offset_of!` asserts pin the
/// historical `OFF_*` / `view.add(N)` layout. Layout only; the sections are host-created.
pub mod gamepad {
    use alloc::string::String;
    use bytemuck::{Pod, Zeroable};

    /// XUSB section magic (loosely "PFXU").
    pub const XUSB_MAGIC: u32 = 0x5558_4650;
    /// Pad section magic (loosely "PFDS"). The two magics use opposite byte-order mnemonics;
    /// only the u32 value is the contract.
    pub const PAD_MAGIC: u32 = 0x5046_4453;

    /// `device_type` DualSense. The section is zeroed, so `0` is the default; one driver serves
    /// every identity.
    pub const DEVTYPE_DUALSENSE: u8 = 0;
    /// DualShock 4 (`VID_054C&PID_09CC`).
    pub const DEVTYPE_DUALSHOCK4: u8 = 1;
    /// DualSense Edge (`VID_054C&PID_0DF2`) — DualSense report codec plus the four back/Fn bits.
    pub const DEVTYPE_DUALSENSE_EDGE: u8 = 2;
    /// Steam Deck (`VID_28DE&PID_1205`). Steam Input promotes it on Windows when the synthesized
    /// USB hardware ids carry `&MI_02` (wired controller interface).
    pub const DEVTYPE_STEAMDECK: u8 = 3;
    /// Xbox Wireless Controller (`VID_045E&PID_0B13` — Bluetooth Xbox is a real HID device;
    /// wired `045E:028E`/`045E:02EA` are not). `pf-xusb` registers only `GUID_DEVINTERFACE_XUSB`
    /// and has no HID collection, so Steam/WGI/GameInput never see it.
    ///
    /// Unlike its siblings the Xbox input report is not 64 bytes — it is `XBOX_INPUT_REPORT_LEN`
    /// (16). hidclass sizes its buffer from the descriptor and refuses an over-long source.
    pub const DEVTYPE_XBOX: u8 = 4;
    /// Xbox One S over Bluetooth (`VID_045E&PID_02FD`).
    /// Shares [`DEVTYPE_XBOX`]'s report descriptor byte-for-byte. All three Xbox identities are
    /// the same pad in HID terms and differ only in VID/PID, product string, and INF model line.
    /// Do not hand-write a per-identity descriptor — the shape is shared; identity is VID/PID.
    pub const DEVTYPE_XBOX_ONE_S: u8 = 5;
    /// Xbox Elite Wireless Controller Series 2 (`VID_045E&PID_0B22`).
    /// The four paddles are not in this identity's report yet. The descriptor is shared (see
    /// [`DEVTYPE_XBOX_ONE_S`]); `xinputhid` may claim the HID collection exclusively anyway.
    pub const DEVTYPE_XBOX_ELITE: u8 = 6;
    /// Steam Controller 2 (Triton): wired identity `28DE:1302`. Raw-passthrough — host feeds
    /// captured reports; the driver answers Steam's feature query-dance (see [`crate::triton`]).
    pub const DEVTYPE_TRITON: u8 = 7;

    /// Written into the section's `driver_proto` on attach. The section starts zeroed, so `0`
    /// means no driver has attached. Bump on a gamepad-layout change.
    ///
    /// v3: sealed DATA section + [`ChannelProof`]. The host learns the duplication target over
    /// the device stack, not the mailbox's `driver_pid`. Mixed pairings fail closed both ways.
    /// Evidence: `design/gamepad-channel-sealing.md`.
    pub const GAMEPAD_PROTO_VERSION: u32 = 3;

    // Channel proof: who to hand the DATA section to. Do not take the duplication target from
    // the mailbox's `driver_pid` — LocalService can spawn a world-executable WUDFHost and publish
    // that pid. Ask the devnode the host created (`SwDeviceCreate` instance id). `pf_xusb` answers
    // via IOCTL; `pf_gamepad`/`pf_mouse` have no control device (hidclass owns the stack).

    /// Proof magic ("PFCP"), and the `PFCP` prefix of the text form.
    pub const PROOF_MAGIC: u32 = 0x5043_4650;

    /// HID string index the minidrivers answer with [`ChannelProof`]. 16-bit on purpose: both
    /// `IOCTL_HID_GET_INDEXED_STRING` and `IOCTL_HID_GET_STRING` pack `(language_id << 16) |
    /// string_index`, so only the low word survives. `0x5046` ("PF") is outside USB's 1..=255
    /// string-descriptor range. hidclass currently does not forward an arbitrary indexed-string
    /// request to a UMDF HID minidriver; kept as the first ask. Working transports:
    /// [`proof_is_serial_string`] (`pf_mouse`) and [`HID_FEATURE_REPORT_CHANNEL_PROOF`] (PS pads).
    pub const HID_STRING_INDEX_CHANNEL_PROOF: u32 = 0x5046;

    // Do not retry `WdfDeviceCreateDeviceInterface` for `pf_gamepad`/`pf_mouse`: hidclass owns
    // `IRP_MJ_CREATE` on a devnode it is the FDO for, so `CreateFile` returns ERROR_GEN_FAILURE.

    /// `CTL_CODE(0x8000, 0x0FE0, METHOD_BUFFERED, FILE_ANY_ACCESS)`: function code no xusb22 IOCTL
    /// uses; `FILE_ANY_ACCESS` so the host can ask over a `CreateFile` handle opened with no access
    /// rights (the same way it must open a HID collection).
    pub const IOCTL_PF_GET_CHANNEL_PROOF: u32 = 0x8000_3F80;

    /// Whether a driver serves its channel proof as its HID serial-number string.
    /// `true` for `pf_mouse` only — its serial (`PFMOUSE00`) is inert. Pad serials are what SDL
    /// and Steam dedup on; Steam mangles a pad's displayed name over serial format alone.
    pub const fn proof_is_serial_string(pad_kind_is_mouse: bool) -> bool {
        pad_kind_is_mouse
    }

    /// Feature report the PS pad identities (DualSense / DualShock 4 / Edge) answer the proof on.
    /// `0x85` is already declared as Feature in all three captured descriptors, so this needs no
    /// report-descriptor change — Steam/SDL fingerprint VID/PID, layout, serial, product string.
    pub const HID_FEATURE_REPORT_CHANNEL_PROOF: u8 = 0x85;

    /// Steam Deck private proof command. The Deck descriptor declares one unnumbered feature
    /// report; Steam drives it as `0x83`/`0xAE`. Two bytes, not one, so a Steam command byte we
    /// have not catalogued cannot be mistaken for it. No descriptor change.
    pub const DECK_PROOF_CMD: [u8; 2] = [0xF9, 0x50];

    /// Driver's answer over the device stack: who it is, which pad, which WUDFHost pid.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct ChannelProof {
        pub magic: u32,
        pub proto: u32,
        /// Pad index from the devnode Location — cross-checked so a mis-resolved devnode cannot
        /// cross-wire two pads.
        pub pad_index: u32,
        /// `GetCurrentProcessId()` of the driver's WUDFHost: the duplication target.
        pub wudf_pid: u32,
    }

    impl ChannelProof {
        pub fn new(pad_index: u32, wudf_pid: u32) -> ChannelProof {
            ChannelProof {
                magic: PROOF_MAGIC,
                proto: GAMEPAD_PROTO_VERSION,
                pad_index,
                wudf_pid,
            }
        }

        /// Validate against the pad the host is delivering. `Err` is the operator-facing reason;
        /// every rejection is a refusal to deliver — do not fall back to an untrusted pid.
        pub fn check(&self, expect_pad_index: u32) -> Result<u32, &'static str> {
            if self.magic != PROOF_MAGIC {
                return Err(
                    "the devnode's answer is not a punktfunk channel proof (bad magic) — \
                            some other driver is bound to this device",
                );
            }
            if self.proto != GAMEPAD_PROTO_VERSION {
                return Err(
                    "the driver bound to this devnode speaks a different gamepad protocol \
                            — update the host and the drivers together",
                );
            }
            if self.pad_index != expect_pad_index {
                return Err(
                    "the devnode answered for a DIFFERENT pad index — the interface lookup \
                            resolved the wrong device",
                );
            }
            if self.wudf_pid == 0 {
                return Err("the driver reported pid 0");
            }
            Ok(self.wudf_pid)
        }

        /// 16 wire bytes of the `pf_xusb` IOCTL answer. Driver crates need no `bytemuck`; both
        /// sides go through one length-checked pair with [`from_bytes`](Self::from_bytes).
        pub fn to_bytes(self) -> [u8; 16] {
            let mut out = [0u8; 16];
            out.copy_from_slice(bytemuck::bytes_of(&self));
            out
        }

        /// Parse [`to_bytes`](Self::to_bytes). `None` on a short read — never zero-extend into a pid.
        ///
        /// `pod_read_unaligned`, not `from_bytes`: the feature-report form offsets the proof by one
        /// byte (report id at 0), so the slice is not 4-aligned. Device I/O buffers have no alignment.
        pub fn from_bytes(b: &[u8]) -> Option<ChannelProof> {
            (b.len() >= 16).then(|| bytemuck::pod_read_unaligned::<ChannelProof>(&b[..16]))
        }

        /// HID feature report of exactly `len` bytes: `[report_id, proof(16), 0…]`. Byte 0 is the
        /// report id; the driver pads to the descriptor length. `None` if `len` cannot hold id+proof.
        pub fn to_feature_report(self, report_id: u8, len: usize) -> Option<alloc::vec::Vec<u8>> {
            if len < 17 {
                return None;
            }
            let mut out = alloc::vec![0u8; len];
            out[0] = report_id;
            out[1..17].copy_from_slice(&self.to_bytes());
            Some(out)
        }

        /// Parse [`to_feature_report`](Self::to_feature_report); skips the leading report id.
        pub fn from_feature_report(b: &[u8]) -> Option<ChannelProof> {
            Self::from_bytes(b.get(1..)?)
        }

        /// HID indexed-string form: `PFCP:<proto>:<pad_index>:<wudf_pid>`.
        /// `HidD_GetIndexedString` is a string channel.
        pub fn to_hid_string(self) -> String {
            alloc::format!("PFCP:{}:{}:{}", self.proto, self.pad_index, self.wudf_pid)
        }

        /// Parse [`to_hid_string`](Self::to_hid_string). `None` on any deviation — refuse delivery
        /// rather than guess a pid.
        pub fn from_hid_string(s: &str) -> Option<ChannelProof> {
            let rest = s.strip_prefix("PFCP:")?;
            let mut it = rest.split(':');
            let proto = it.next()?.parse::<u32>().ok()?;
            let pad_index = it.next()?.parse::<u32>().ok()?;
            let wudf_pid = it.next()?.parse::<u32>().ok()?;
            if it.next().is_some() {
                return None; // trailing field: not a shape we mint
            }
            Some(ChannelProof {
                magic: PROOF_MAGIC,
                proto,
                pad_index,
                wudf_pid,
            })
        }
    }

    /// Bootstrap-mailbox magic (`"PFBT"` LE). The host stamps it last (after `host_proto`) so a
    /// driver only trusts a fully-initialized mailbox.
    pub const BOOT_MAGIC: u32 = 0x5442_4650;

    /// `Global\pfxusb-boot-<index>` — Xbox 360 pad bootstrap mailbox ([`PadBootstrap`]).
    pub fn xusb_boot_name(index: u8) -> String {
        alloc::format!("Global\\pfxusb-boot-{index}")
    }
    /// `Global\pfds-boot-<index>` — DualSense / DualShock 4 bootstrap mailbox ([`PadBootstrap`]).
    pub fn pad_boot_name(index: u8) -> String {
        alloc::format!("Global\\pfds-boot-{index}")
    }

    /// Per-pad bootstrap mailbox (32 B, named `Global\pf…-boot-<index>`, SY+LS DACL) — the only
    /// named object on the gamepad channel. UMDF HID minidrivers have no control device (hidclass
    /// owns the stack), so this is the late-bound handshake: host stamps `host_proto` then `magic`;
    /// driver writes `driver_proto`/`driver_pid`; host asks the **devnode** ([`ChannelProof`]) who
    /// the driver is, duplicates the unnamed DATA section, then writes `data_handle`/`handle_pid`
    /// and bumps `handle_seq` last. `driver_pid` is advisory; the mailbox does not choose the
    /// duplication target. Evidence: `design/gamepad-channel-sealing.md`.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct PadBootstrap {
        /// [`BOOT_MAGIC`], host-stamped last at creation.
        pub magic: u32,
        /// Host's [`GAMEPAD_PROTO_VERSION`]. A driver whose version differs must not publish its
        /// pid (fail closed); it still writes `driver_proto` so the host can log the mismatch.
        pub host_proto: u32,
        /// Driver's WUDFHost pid (`0` = none yet). Advisory liveness hint — not the duplication
        /// target; that comes from [`ChannelProof`].
        pub driver_pid: u32,
        /// Driver's [`GAMEPAD_PROTO_VERSION`] (diagnostics only).
        pub driver_proto: u32,
        /// DATA-section handle VALUE duplicated into `handle_pid`'s table; valid only in that process.
        pub data_handle: u64,
        /// Pid `data_handle` was duplicated for — a driver whose pid differs ignores the delivery.
        pub handle_pid: u32,
        /// Host-global monotonic, never 0. Bumped AFTER `data_handle`/`handle_pid` — new-delivery trigger.
        pub handle_seq: u32,
    }

    /// Virtual Xbox 360 (XInput) shared section (64 B). Host writes XInput state; driver answers
    /// `XInputGetState`. Driver writes `XInputSetState` into `rumble_*` (bumping `rumble_seq`).
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct XusbShm {
        pub magic: u32,
        /// XInput `dwPacketNumber` — bumped on every state change.
        pub packet: u32,
        pub buttons: u16,
        pub left_trigger: u8,
        pub right_trigger: u8,
        pub thumb_lx: i16,
        pub thumb_ly: i16,
        pub thumb_rx: i16,
        pub thumb_ry: i16,
        pub _reserved0: u32,
        /// Bumped on a new force-feedback packet.
        pub rumble_seq: u32,
        pub rumble_large: u8,
        pub rumble_small: u8,
        pub _pad0: [u8; 2],
        /// [`GAMEPAD_PROTO_VERSION`] while attached. `0` = no driver — the host health check keys off it.
        pub driver_proto: u32,
        /// Bumped on every serviced XInput IOCTL. Only advances while something polls the slot, so a
        /// static value is not an error.
        pub driver_heartbeat: u32,
        /// Pad index (host-stamped before the magic). The driver checks it against
        /// `pszDeviceLocation` so a cross-pad delivery is rejected. Carved from v1 reserved space.
        pub pad_index: u32,
        pub _reserved1: [u8; 20],
    }

    /// Pre-ring [`PadShm`] size. Every field old binaries know sits below this offset; the ring
    /// keeps bytes `0..256` identical. Pagefile-backed sections are page-granular, so either
    /// generation's view maps against either generation's section — a driver must still fall
    /// back to this size if the full-size map is refused (`ChannelConfig::min_data_size`).
    pub const PAD_SHM_LEGACY_SIZE: usize = 256;

    /// v2.1 output-report ring depth — hardcoded `%` in every pre-v2.2 driver, and the drain
    /// length whenever [`PadShm::out_ring_len`] reads 0. Eight slots at a ~4 ms poll overflow
    /// under a sustained >2 kHz writer (DS5 compat-vibration re-sends per audio quantum).
    pub const OUT_RING_LEN: u32 = 8;
    pub const OUT_RING_LEN_USIZE: usize = OUT_RING_LEN as usize;

    /// v2.2 ring depth: every slot that fits the one-page section ([`PAD_SHM_SIZE`] = 4096).
    /// 56 slots at ~4 ms poll ≈ 14 kHz. Used only when both sides negotiated it (`out_ring_ver
    /// >= 2` and the driver echoed the length in [`PadShm::out_ring_len`]).
    pub const OUT_RING_LEN_V22: u32 = 56;
    pub const OUT_RING_LEN_V22_USIZE: usize = OUT_RING_LEN_V22 as usize;

    /// v2.1 [`PadShm`] size. A v2.1 driver maps this much and gates 8-slot ring writes on
    /// `mapped_len() >= 1024`; v2.2 keeps bytes `0..1024` identical.
    pub const PAD_SHM_V21_SIZE: usize = 1024;

    /// Full [`PadShm`] size — exactly one page. Hard ceiling: pagefile-backed sections round up
    /// to page granularity, which is what lets every generation map its own size against any
    /// other generation's section. Growing past 4096 needs a new negotiation.
    pub const PAD_SHM_SIZE: usize = 4096;

    /// One slot of the lossless output-report ring: report bytes as the game wrote them (id
    /// first), with the exact length. The legacy latest-report slot's fixed 64-byte copy can
    /// carry a stale tail from a previous longer report.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct OutSlot {
        /// Valid bytes in `data` (`0..=64`). `0` = never written.
        pub len: u32,
        pub data: [u8; 64],
    }

    /// DualSense / DualShock 4 shared section ([`PAD_SHM_SIZE`] = 4096). Bytes `0..256` are the
    /// v2 layout ([`PAD_SHM_LEGACY_SIZE`]); `0..1024` are v2.1 ([`PAD_SHM_V21_SIZE`]). Host writes
    /// `input`; driver publishes output into the legacy `output` slot (every host generation reads
    /// it) and, when `out_ring_ver` is stamped, into the lossless `out_ring`. The single slot
    /// coalesces — a rumble-stop overwritten inside one poll is gone (`design/rumble-root-fix.md`).
    ///
    /// Tail extension, not a [`GAMEPAD_PROTO_VERSION`] bump: bootstrap fails closed on a version
    /// mismatch (no pad at all), the wrong failure for a feedback-quality fix. An old host never
    /// stamps `out_ring_ver`, so a new driver stays on the legacy slot.
    ///
    /// Ring-length: each side declares, the shorter wins. Host stamps `out_ring_ver = 2`; the
    /// driver picks (`>= 2` + full map → 56, `1` → 8, `0` → no ring) and echoes into
    /// `out_ring_len` before every `ring_head` bump. Drain keys off the echo (`0` = 8). Store
    /// order slot-bytes → echo → head-bump: an Acquire-observed head bump always has that length.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct PadShm {
        pub magic: u32,
        pub _reserved0: u32,
        /// Host-written HID input (≤ 64 B). Spans `magic`+pad .. `out_seq`.
        pub input: [u8; 64],
        /// Bumped when the driver publishes a new `output` report.
        pub out_seq: u32,
        /// Driver-written output: rumble / lightbar / player-LEDs / adaptive triggers.
        pub output: [u8; 64],
        /// HID identity — [`DEVTYPE_DUALSENSE`] / [`DEVTYPE_DUALSHOCK4`].
        pub device_type: u8,
        pub _pad0: [u8; 3],
        /// [`GAMEPAD_PROTO_VERSION`] while mapped. `0` = no driver — the host health check keys off it.
        pub driver_proto: u32,
        /// Bumped by the driver's ~125 Hz timer each tick — advances whenever loaded, game or not.
        pub driver_heartbeat: u32,
        /// Pad index (host-stamped before the magic) — see [`XusbShm::pad_index`].
        pub pad_index: u32,
        /// Host-stamped `1` ⇔ this section carries `out_ring` and the host drains it. Zeroed
        /// section + old host never writes it, so `0` tells a new driver to stay legacy-only.
        pub out_ring_ver: u32,
        /// Driver-bumped AFTER writing `out_ring[ring_head % len]`. Overflow is `head - tail > len`.
        /// Same publish-then-bump order as `out_seq` (host Acquire load).
        pub ring_head: u32,
        /// Ring length the driver's slot math is using, (re-)stamped before every `ring_head`
        /// bump. `0` = pre-v2.2 driver = [`OUT_RING_LEN`].
        pub out_ring_len: u32,
        /// Seqlock over [`PadShm::input`]: odd while mid-copy, even when the slot holds a whole
        /// report. Host: bump odd, Release-fence, write 64 bytes, Release-store even. Driver
        /// samples before and after and retries on a write in flight. An old host leaves this 0
        /// (constant even), so a new driver's re-check always passes. Inside the v2 legacy region.
        pub input_gen: u32,
        pub _reserved1: [u8; 84],
        /// Lossless output ring. [`OUT_RING_LEN`] under v2.1, [`OUT_RING_LEN_V22`] under v2.2
        /// (slots 8.. overlay what v2.1 called `_reserved2`, which no shipped binary touched).
        pub out_ring: [OutSlot; OUT_RING_LEN_V22_USIZE],
        pub _reserved2: [u8; 32],
    }

    // Offsets are the wire contract the shipped drivers already read by hand. A failing assert
    // means the struct no longer matches the historical `OFF_*` / `view.add(N)` layout.
    const _: () = {
        use core::mem::{offset_of, size_of};

        assert!(size_of::<XusbShm>() == 64);
        assert!(offset_of!(XusbShm, magic) == 0);
        assert!(offset_of!(XusbShm, packet) == 4);
        assert!(offset_of!(XusbShm, buttons) == 8);
        assert!(offset_of!(XusbShm, left_trigger) == 10);
        assert!(offset_of!(XusbShm, right_trigger) == 11);
        assert!(offset_of!(XusbShm, thumb_lx) == 12);
        assert!(offset_of!(XusbShm, thumb_ly) == 14);
        assert!(offset_of!(XusbShm, thumb_rx) == 16);
        assert!(offset_of!(XusbShm, thumb_ry) == 18);
        assert!(offset_of!(XusbShm, rumble_seq) == 24);
        assert!(offset_of!(XusbShm, rumble_large) == 28);
        assert!(offset_of!(XusbShm, rumble_small) == 29);
        assert!(offset_of!(XusbShm, driver_proto) == 32);
        assert!(offset_of!(XusbShm, driver_heartbeat) == 36);
        assert!(offset_of!(XusbShm, pad_index) == 40);

        assert!(size_of::<PadShm>() == PAD_SHM_SIZE);
        assert!(offset_of!(PadShm, magic) == 0);
        assert!(offset_of!(PadShm, input) == 8);
        assert!(offset_of!(PadShm, out_seq) == 72);
        assert!(offset_of!(PadShm, output) == 76);
        assert!(offset_of!(PadShm, device_type) == 140);
        assert!(offset_of!(PadShm, driver_proto) == 144);
        assert!(offset_of!(PadShm, driver_heartbeat) == 148);
        assert!(offset_of!(PadShm, pad_index) == 152);
        // Ring extension — everything below PAD_SHM_LEGACY_SIZE is the v2 layout verbatim.
        assert!(offset_of!(PadShm, out_ring_ver) == 156);
        assert!(offset_of!(PadShm, ring_head) == 160);
        assert!(offset_of!(PadShm, out_ring) == PAD_SHM_LEGACY_SIZE);
        assert!(size_of::<OutSlot>() == 68);
        // Echo field in v2.1 reserved space; slot k stays at 256 + k*68; struct is one page.
        assert!(offset_of!(PadShm, out_ring_len) == 164);
        // Input seqlock: 4-aligned (atomic accessors check it) and inside the v2 legacy region.
        assert!(offset_of!(PadShm, input_gen) == 168);
        assert!(offset_of!(PadShm, input_gen) % 4 == 0);
        assert!(offset_of!(PadShm, input_gen) < PAD_SHM_LEGACY_SIZE);
        assert!(
            PAD_SHM_LEGACY_SIZE + OUT_RING_LEN_USIZE * size_of::<OutSlot>() <= PAD_SHM_V21_SIZE
        );
        assert!(PAD_SHM_SIZE == 4096);

        assert!(size_of::<ChannelProof>() == 16);
        assert!(offset_of!(ChannelProof, magic) == 0);
        assert!(offset_of!(ChannelProof, proto) == 4);
        assert!(offset_of!(ChannelProof, pad_index) == 8);
        assert!(offset_of!(ChannelProof, wudf_pid) == 12);

        assert!(size_of::<PadBootstrap>() == 32);
        assert!(offset_of!(PadBootstrap, magic) == 0);
        assert!(offset_of!(PadBootstrap, host_proto) == 4);
        assert!(offset_of!(PadBootstrap, driver_pid) == 8);
        assert!(offset_of!(PadBootstrap, driver_proto) == 12);
        assert!(offset_of!(PadBootstrap, data_handle) == 16);
        assert!(offset_of!(PadBootstrap, handle_pid) == 24);
        assert!(offset_of!(PadBootstrap, handle_seq) == 28);
    };
}

/// Steam Controller 2 (Triton) wire tables: UMDF driver (answers Steam synchronously) and
/// host/inject (Linux usbip + tests). Pure byte-packing so it tests on any host.
pub mod triton {
    /// Feature-1 command bytes of the Valve query dance.
    pub const ID_GET_ATTRIBUTES_VALUES: u8 = 0x83;
    pub const ID_GET_STRING_ATTRIBUTE: u8 = 0xAE;
    pub const ID_GET_FIRMWARE_INFO: u8 = 0xF2;
    /// Output report id Steam rumbles with (`80 | type | intensity16 | Lspeed16 Lgain | Rspeed16 Rgain`).
    pub const ID_OUT_REPORT_HAPTIC_RUMBLE: u8 = 0x80;

    /// Wired Steam Controller 2 identity (`28DE:1302`) — Triton half of the `0x83` attributes reply.
    const WIRED_PRODUCT: u32 = 0x1302;

    /// Firmware build time (unix epoch) as attribute tag `4` (`ATTRIB_FIRMWARE_BUILD_TIME`) in
    /// the `0x83` reply, mirrored at bytes 4..8 of the `0xF2` firmware-info reply — the two must
    /// agree. `0x6A6D_3700` = 2026-08-01T00:00:00Z. An older synthetic epoch (Feb 2016) made
    /// Steam offer to "update" the virtual pad, forwarding SET_REPORTs toward a real controller.
    /// Bump when Steam learns a newer shipping firmware and starts prompting again.
    pub const FW_BUILD_TIME: u32 = 0x6A6D_3700;

    /// Bit 31 of an out-ring slot's `len` marks a FEATURE set (vs interrupt/output). Only Triton's
    /// producer/consumer interpret it; other devtypes write plain lengths, so the bit is additive.
    pub const OUT_FEATURE_BIT: u32 = 0x8000_0000;
    #[inline]
    pub const fn out_len(raw: u32) -> u32 {
        raw & !OUT_FEATURE_BIT
    }
    #[inline]
    pub const fn out_is_feature(raw: u32) -> bool {
        raw & OUT_FEATURE_BIT != 0
    }

    /// Wire length (id byte included) of each input report the wired descriptor declares.
    /// hidclass sizes its read buffer from the largest (0x42 → 54) and refuses over-long
    /// completions. `None` = undeclared id, drop it (0x47 is BLE-only, not in the 372-byte descriptor).
    pub const fn input_len(report_id: u8) -> Option<usize> {
        match report_id {
            0x42 => Some(54),
            0x45 => Some(46),
            0x43 => Some(15),
            0x44 => Some(6),
            0x79 => Some(2),
            0x7B => Some(13),
            _ => None,
        }
    }

    /// Declared wire length (id byte included) of each OUTPUT report. hidclass pads every write
    /// to `OutputReportByteLength` (64), so the host trims before forwarding — a 0x80 rumble is
    /// 10 bytes on GATT, not 64. Unknown id returns 64: no trim, never guess a length.
    /// Hand-mirrored on the Apple client as `Sc2Device.strippedOutputLen` (id-excluded, so
    /// `stripped + 1` == the value here). Edit this table and that one together.
    pub const fn out_report_len(id: u8) -> usize {
        match id {
            0x80 => 10,
            0x81 => 8,
            0x82 => 4,
            0x83 => 10,
            0x84 => 9,
            0x85 => 4,
            0x86 => 4,
            // 0x87/0x88/0x89 are declared full-length (63-byte payload) blobs.
            _ => 64,
        }
    }

    /// Per-pad unit id (`"TRI\0" | index` — same value the Linux leg uses).
    pub const fn unit_id(index: u8) -> u32 {
        0x5452_4900 | index as u32
    }

    /// ASCII serial `FVPF1302<idx:02>D03`. Steam rejects a "PF"-leading serial; the FVPF prefix
    /// is what the host's physical-conflict gate excludes.
    pub fn serial(index: u8, out: &mut [u8; 13]) {
        const D: &[u8; 10] = b"0123456789";
        out.copy_from_slice(b"FVPF130200D03");
        out[8] = D[(index / 10 % 10) as usize];
        out[9] = D[(index % 10) as usize];
    }

    /// Wired Triton's captured 372-byte report descriptor. Byte-identical to the sysfs capture;
    /// do not re-derive.
    #[rustfmt::skip]
    pub static RDESC: [u8; 372] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x40, 0x09, 0x01, 0xA1, 0x00,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x02, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
        0x95, 0x02, 0x81, 0x02, 0x75, 0x06, 0x95, 0x01, 0x81, 0x01, 0x05, 0x01,
        0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02,
        0x81, 0x06, 0x95, 0x01, 0x09, 0x38, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38,
        0x02, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0, 0x05, 0x01, 0x09, 0x06, 0xA1,
        0x01, 0x85, 0x41, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25,
        0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x81, 0x01, 0x19, 0x00, 0x29,
        0x65, 0x15, 0x00, 0x25, 0x65, 0x75, 0x08, 0x95, 0x06, 0x81, 0x00, 0xC0,
        0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x42, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x35, 0x09, 0x42, 0x81, 0x02, 0x85, 0x44,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x05, 0x09, 0x44, 0x81,
        0x02, 0x85, 0x79, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01,
        0x09, 0x79, 0x81, 0x02, 0x85, 0x43, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x0E, 0x09, 0x43, 0x81, 0x02, 0x85, 0x7B, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x0C, 0x09, 0x7B, 0x81, 0x02, 0x85, 0x45,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x2D, 0x09, 0x45, 0x81,
        0x02, 0x85, 0x80, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x09,
        0x09, 0x80, 0x91, 0x02, 0x85, 0x81, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x07, 0x09, 0x81, 0x91, 0x02, 0x85, 0x82, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x03, 0x09, 0x82, 0x91, 0x02, 0x85, 0x83,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x09, 0x09, 0x83, 0x91,
        0x02, 0x85, 0x84, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x08,
        0x09, 0x84, 0x91, 0x02, 0x85, 0x85, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x03, 0x09, 0x85, 0x91, 0x02, 0x85, 0x86, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x03, 0x09, 0x86, 0x91, 0x02, 0x85, 0x87,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0x09, 0x87, 0x91,
        0x02, 0x85, 0x89, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F,
        0x09, 0x89, 0x91, 0x02, 0x85, 0x88, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x3F, 0x09, 0x88, 0x91, 0x02, 0x85, 0x01, 0x95, 0x3F, 0x09,
        0x01, 0xB1, 0x02, 0x85, 0x02, 0x95, 0x3F, 0x09, 0x01, 0xB1, 0x02, 0xC0,
    ];

    /// Feature GET_REPORT reply for Steam's `GetControllerInfo` query dance. The reply's command
    /// byte must echo the last SET's command or Steam never adopts the pad. Frame is feature
    /// report id 1 (`[0x01][cmd][len][payload…]`, matching SDL). `last_set` is id-first
    /// (`[0x01, cmd, …]`); a stack that already stripped the id (`[cmd, …]`, cmd ≥ 0x80) works too.
    pub fn feature_reply(last_set: &[u8], serial: &str, unit_id: u32) -> [u8; 64] {
        const ATTRIB_STR_UNIT_SERIAL: u8 = 0x01;

        let body = match last_set {
            [0x01, rest @ ..] => rest,
            d => d,
        };
        let cmd = body.first().copied().unwrap_or(ID_GET_STRING_ATTRIBUTE);

        let mut r = [0u8; 64];
        r[0] = 0x01;
        match cmd {
            ID_GET_ATTRIBUTES_VALUES => {
                // Captured controller response: 25-byte payload, five id/u32 attributes.
                r[1] = ID_GET_ATTRIBUTES_VALUES;
                r[2] = 0x19;
                let attrs = [
                    (0x01, WIRED_PRODUCT),
                    (0x02, 0),
                    (0x0A, unit_id),
                    // Tag 4 = ATTRIB_FIRMWARE_BUILD_TIME. See [`FW_BUILD_TIME`].
                    (0x04, FW_BUILD_TIME),
                    (0x09, 0x49),
                ];
                let mut o = 3;
                for (id, val) in attrs {
                    r[o] = id;
                    r[o + 1..o + 5].copy_from_slice(&val.to_le_bytes());
                    o += 5;
                }
            }
            ID_GET_STRING_ATTRIBUTE => {
                // Captured replies always declare 20 bytes: attribute id plus a 19-byte padded string.
                let attr = body.get(2).copied().unwrap_or(ATTRIB_STR_UNIT_SERIAL);
                let b = serial.as_bytes();
                let len = b.len().min(19);
                r[..4].copy_from_slice(&[0x01, ID_GET_STRING_ATTRIBUTE, 0x14, attr]);
                r[4..4 + len].copy_from_slice(&b[..len]);
            }
            ID_GET_FIRMWARE_INFO => {
                let index = body.get(2).copied().unwrap_or(0);
                r[1] = ID_GET_FIRMWARE_INFO;
                r[3] = index;
                match index {
                    0 => {
                        r[2] = 0x29;
                        // Must agree with the 0x83 reply's tag-4 attribute (Steam may cross-check).
                        r[4..8].copy_from_slice(&FW_BUILD_TIME.to_le_bytes());
                        r[8] = 0x49;
                        r[12..24].copy_from_slice(b"603f69218a85");
                        let b = serial.as_bytes();
                        let len = b.len().min(16);
                        r[28..28 + len].copy_from_slice(&b[..len]);
                    }
                    1 => {
                        r[2] = 0x22;
                        r[4..37].copy_from_slice(&[
                            0x00, 0x57, 0xD0, 0x18, 0x6A, 0x37, 0x30, 0x35, 0x34, 0x32, 0x35, 0x37,
                            0x64, 0x32, 0x64, 0x61, 0x37, 0x00, 0x00, 0x00, 0x00, 0x23, 0x00, 0x00,
                            0x00, 0x00, 0x00, 0x00, 0x00, 0x33, 0x6D, 0x02, 0x00,
                        ]);
                    }
                    _ => {
                        r[2] = 0x09;
                        r[4..12].copy_from_slice(&[0x7C, 0x4F, 0x01, 0x00, 0x01, 0, 0, 0]);
                    }
                }
            }
            _ => {
                let n = body.len().min(63);
                r[1..1 + n].copy_from_slice(&body[..n]);
            }
        }
        r
    }
}

/// Virtual-pointer shared-memory layout (host ↔ UMDF HID-mouse minidriver `pf_mouse`).
///
/// With no pointing device, win32k reports the cursor absent (`SM_MOUSEPRESENT` = 0) and DWM
/// never composites a cursor into the pf-vdisplay frame — `SendInput` still moves it, but the
/// stream shows no pointer. A resident HID mouse devnode makes Windows consider a pointer
/// present. Injection stays `SendInput`; the report path is the higher-fidelity route.
///
/// Same sealed-pad handshake as [`gamepad`] (`design/gamepad-channel-sealing.md`):
/// [`gamepad::PadBootstrap`], [`mouse_boot_name`], mouse DATA magic, `pad_index` 0. Reusing
/// the handshake means `pf-umdf-util`'s `ChannelClient`/`PadChannel` serve the mouse unchanged.
pub mod mouse {
    use alloc::string::String;
    use bytemuck::{Pod, Zeroable};

    /// Mouse DATA-section magic ("PFMO" LE) — distinct from the pad magics so a cross-wire fails.
    pub const MOUSE_MAGIC: u32 = 0x4F4D_4650;

    /// `Global\pfmouse-boot-<index>` — mouse bootstrap mailbox ([`crate::gamepad::PadBootstrap`]).
    pub fn mouse_boot_name(index: u8) -> String {
        alloc::format!("Global\\pfmouse-boot-{index}")
    }

    /// HID identity ("PF" / "MO") — obviously virtual; no software matches on it, unlike the
    /// pads' cloned Sony/Valve ids.
    pub const MOUSE_VID: u16 = 0x5046;
    pub const MOUSE_PID: u16 = 0x4D4F;
    pub const MOUSE_VER: u16 = 0x0100;

    /// Input report id `0x01`: `[id, buttons(5 bits), x_lo, x_hi, y_lo, y_hi, wheel, pan]` —
    /// absolute X/Y over `0..=`[`MOUSE_ABS_MAX`], relative wheel/pan.
    pub const MOUSE_REPORT_ID: u8 = 0x01;
    pub const MOUSE_REPORT_LEN: usize = 8;
    /// Logical maximum of the absolute X/Y axes (15-bit, HID-descriptor convention).
    pub const MOUSE_ABS_MAX: u16 = 0x7FFF;

    /// Build the 8-byte input report. Pure so the layout is unit-tested here (the driver
    /// workspace is `panic = "abort"`); the driver only ferries these bytes.
    #[must_use]
    pub fn input_report(buttons: u8, x: u16, y: u16, wheel: i8, pan: i8) -> [u8; MOUSE_REPORT_LEN] {
        let x = x.min(MOUSE_ABS_MAX);
        let y = y.min(MOUSE_ABS_MAX);
        [
            MOUSE_REPORT_ID,
            buttons & 0x1F,
            (x & 0xFF) as u8,
            (x >> 8) as u8,
            (y & 0xFF) as u8,
            (y >> 8) as u8,
            wheel as u8,
            pan as u8,
        ]
    }

    /// Virtual-mouse shared section (64 B). Host writes a report then bumps `in_seq` (Release);
    /// the driver's timer Acquire-loads it and completes a pended `READ_REPORT`. Idle generates
    /// no HID traffic — a constant report stream would read as user activity to the OS.
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct MouseShm {
        pub magic: u32,
        /// Bumped AFTER `report` is in place (Release). `0` = nothing published yet.
        pub in_seq: u32,
        pub report: [u8; MOUSE_REPORT_LEN],
        /// [`crate::gamepad::GAMEPAD_PROTO_VERSION`] while attached. `0` = no driver.
        pub driver_proto: u32,
        /// Bumped each timer tick — advances whether or not input flows.
        pub driver_heartbeat: u32,
        /// Device index (host-stamped before the magic); driver checks it against the devnode Location.
        pub pad_index: u32,
        pub _reserved: [u8; 36],
    }

    // Offsets are the cross-process wire contract — pin every one.
    const _: () = {
        use core::mem::{offset_of, size_of};

        assert!(size_of::<MouseShm>() == 64);
        assert!(offset_of!(MouseShm, magic) == 0);
        assert!(offset_of!(MouseShm, in_seq) == 4);
        assert!(offset_of!(MouseShm, report) == 8);
        assert!(offset_of!(MouseShm, driver_proto) == 16);
        assert!(offset_of!(MouseShm, driver_heartbeat) == 20);
        assert!(offset_of!(MouseShm, pad_index) == 24);
    };
}

/// Hardware-cursor channel: one unnamed file mapping per monitor, delivered by handle value
/// ([`control::IOCTL_SET_CURSOR_CHANNEL`]). The driver's cursor thread seqlock-writes shape +
/// position; the host reads at encode-tick pace — no event crosses the boundary. Writer: bump
/// [`CursorShm::seq`] odd, write, bump even. Reader: retry while odd, copy, re-read — unchanged
/// ⇒ consistent snapshot. Position-only updates never touch shape bytes, so a reader that
/// skips unchanged `shape_id`s never copies torn pixels.
pub mod cursor {
    use bytemuck::{Pod, Zeroable};

    /// [`CursorShm`] magic (`b"PFCU"` LE); anything else = not attached yet.
    pub const CURSOR_MAGIC: u32 = u32::from_le_bytes(*b"PFCU");

    /// Max cursor side (px) declared to the OS (`IDDCX_CURSOR_CAPS::MaxX/MaxY`). Windows XL
    /// accessibility cursors top out here; the host's wire forwarder downscales anyway.
    pub const CURSOR_SHAPE_MAX: u32 = 256;

    /// Shape-buffer bytes: 32-bpp at the declared max.
    pub const CURSOR_SHAPE_BYTES: usize = (CURSOR_SHAPE_MAX * CURSOR_SHAPE_MAX * 4) as usize;

    /// Byte offset of the shape pixels (64-byte header).
    pub const CURSOR_SHAPE_OFFSET: usize = 64;

    pub const CURSOR_SHM_SIZE: usize = CURSOR_SHAPE_OFFSET + CURSOR_SHAPE_BYTES;

    /// `IDDCX_CURSOR_SHAPE_TYPE` values. The driver writes the OS value into [`CursorShm::cursor_type`].
    pub const CURSOR_TYPE_MASKED_COLOR: u32 = 1;
    pub const CURSOR_TYPE_ALPHA: u32 = 2;

    /// Section header; shape pixels follow at [`CURSOR_SHAPE_OFFSET`]. `x`/`y` are the shape's
    /// top-left in desktop coordinates (IddCx `IDARG_OUT_QUERY_HWCURSOR::X/Y` — position −
    /// hotspot, can be negative). `shape_id` bumps on every shape set. Pixels are 32-bpp rows at
    /// `pitch` (BGRA for ALPHA; color+mask for MASKED_COLOR — the host converts).
    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct CursorShm {
        pub magic: u32,
        /// Seqlock: odd = writer mid-update.
        pub seq: u32,
        pub visible: u32,
        pub cursor_type: u32,
        pub x: i32,
        pub y: i32,
        pub shape_id: u32,
        pub width: u32,
        pub height: u32,
        pub pitch: u32,
        pub hot_x: u32,
        pub hot_y: u32,
        pub _reserved: [u32; 4],
    }

    // Layout is load-bearing across the process boundary — pin it.
    const _: () = {
        use core::mem::{offset_of, size_of};
        assert!(size_of::<CursorShm>() == 64);
        assert!(size_of::<CursorShm>() <= CURSOR_SHAPE_OFFSET);
        assert!(offset_of!(CursorShm, magic) == 0);
        assert!(offset_of!(CursorShm, seq) == 4);
        assert!(offset_of!(CursorShm, visible) == 8);
        assert!(offset_of!(CursorShm, cursor_type) == 12);
        assert!(offset_of!(CursorShm, x) == 16);
        assert!(offset_of!(CursorShm, y) == 20);
        assert!(offset_of!(CursorShm, shape_id) == 24);
        assert!(offset_of!(CursorShm, width) == 28);
        assert!(offset_of!(CursorShm, height) == 32);
        assert!(offset_of!(CursorShm, pitch) == 36);
        assert!(offset_of!(CursorShm, hot_x) == 40);
        assert!(offset_of!(CursorShm, hot_y) == 44);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn dtd_encodes_the_session_mode() {
        // 1920×1080@60 with the fixed RB blanking: totals 2000×1125 → 135.00 MHz = 13500 × 10 kHz.
        let d = edid::dtd(1920, 1080, 60).expect("1080p60 fits the DTD encoding");
        assert_eq!(u16::from_le_bytes([d[0], d[1]]), 13_500);
        // Active dimensions round-trip through the split 8+4-bit fields.
        assert_eq!(u32::from(d[2]) | (u32::from(d[4] >> 4) << 8), 1920);
        assert_eq!(u32::from(d[5]) | (u32::from(d[7] >> 4) << 8), 1080);
        // Blanking fields carry the fixed geometry; flags match the legacy descriptor.
        assert_eq!(u32::from(d[3]) | (u32::from(d[4] & 0x0F) << 8), 80);
        assert_eq!(u32::from(d[6]) | (u32::from(d[7] & 0x0F) << 8), 45);
        assert_eq!(d[17], 0x1E);
    }

    #[test]
    fn dtd_rejects_what_the_encoding_cannot_carry() {
        // 4K120: (3840+80)·(2160+45)·120 ≈ 1.037 GHz — past the u16 10 kHz pixel-clock field.
        assert_eq!(edid::dtd(3840, 2160, 120), None);
        // 4K60 fits (≈518 MHz).
        assert!(edid::dtd(3840, 2160, 60).is_some());
        // Degenerate and over-wide modes are refused, not mis-encoded.
        assert_eq!(edid::dtd(0, 1080, 60), None);
        assert_eq!(edid::dtd(5000, 1080, 10), None);
    }

    /// Sum of a 128-byte EDID block: the trailing checksum byte is what drives this to 0.
    fn block_sum(block: &[u8]) -> u8 {
        block.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
    }

    #[test]
    fn edid_matches_the_golden_bytes() {
        // Byte-for-byte capture of what the driver shipped before this code moved here. One byte
        // out of place and Windows drops HDR, so nothing below may drift silently.
        #[rustfmt::skip]
        const GOLDEN: [u8; 256] = [
            0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00,
            0x41, 0xCB, 0x01, 0x00, 0x07, 0x00, 0x00, 0x00,
            0xFF, 0x21, 0x01, 0x04, 0xB0, 0x32, 0x1F, 0x78,
            0x03, 0x78, 0xB1, 0xB5, 0x4A, 0x2B, 0xCC, 0x21,
            0x0B, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0xC4, 0xB7,
            0x00, 0x50, 0xA0, 0xA0, 0x2D, 0x50, 0x08, 0x20,
            0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1E,
            0x00, 0x00, 0x00, 0xFD, 0x08, 0x17, 0xF0, 0x0F,
            0xFF, 0xFF, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20,
            0x20, 0x20, 0x00, 0x00, 0x00, 0xFC, 0x00, 0x50,
            0x75, 0x6E, 0x6B, 0x74, 0x66, 0x75, 0x6E, 0x6B,
            0x0A, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x3F,
            0x02, 0x03, 0x0F, 0x00, 0xE3, 0x05, 0x80, 0x00,
            0xE6, 0x06, 0x05, 0x01, 0x72, 0x52, 0x0F, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xBF,
        ];
        let lum = edid::ClientLuminance {
            max_nits: 600,
            max_frame_avg_nits: 300,
            min_millinits: 20,
        };
        assert_eq!(edid::generate(7, lum, Some((2560, 1440, 120))), GOLDEN);
    }

    #[test]
    fn edid_blocks_checksum_to_zero() {
        let lum = edid::ClientLuminance {
            max_nits: 1000,
            max_frame_avg_nits: 400,
            min_millinits: 50,
        };
        for (serial, l, mode) in [
            (0, edid::ClientLuminance::default(), None),
            (1, lum, Some((1920, 1080, 60))),
            (u32::MAX, lum, Some((3840, 2160, 120))),
        ] {
            let e = edid::generate(serial, l, mode);
            assert_eq!(block_sum(&e[..128]), 0, "base block, serial {serial}");
            assert_eq!(block_sum(&e[128..]), 0, "CTA block, serial {serial}");
        }
    }

    #[test]
    fn edid_serial_lands_at_0x0c_and_rechecksums() {
        for serial in [0u32, 1, 7, 0x00FF_00FF, u32::MAX] {
            let e = edid::generate(serial, edid::ClientLuminance::default(), None);
            assert_eq!(e[0x0C..0x10], serial.to_le_bytes());
            assert_eq!(edid::get_serial(&e).unwrap(), serial);
            // The base block alone is what the mode callbacks sometimes get handed back.
            assert_eq!(edid::get_serial(&e[..128]).unwrap(), serial);
            assert_eq!(block_sum(&e[..128]), 0);
        }
        // A short descriptor is rejected, not read out of bounds.
        assert!(edid::get_serial(&[0u8; 8]).is_err());
    }

    #[test]
    fn edid_swaps_the_preferred_dtd_only_when_the_mode_fits() {
        let lum = edid::ClientLuminance::default();
        let placeholder = edid::generate(1, lum, None);
        // The stock descriptor is the 148.50 MHz 1080p60 timing baked into the base block.
        assert_eq!(
            u16::from_le_bytes([placeholder[54], placeholder[55]]),
            14_850
        );
        // A mode that fits replaces all 18 bytes; 4K120 does not fit, so the stock one stays.
        let swapped = edid::generate(1, lum, Some((2560, 1440, 120)));
        assert_eq!(swapped[54..72], edid::dtd(2560, 1440, 120).unwrap());
        let too_fast = edid::generate(1, lum, Some((3840, 2160, 120)));
        assert_eq!(too_fast[54..72], placeholder[54..72]);
    }

    #[test]
    fn edid_hdr_block_tracks_the_client_volume() {
        // No client volume reported: the built-in ~993 / 400 / 0.05 nit defaults stay.
        let stock = edid::generate(1, edid::ClientLuminance::default(), None);
        assert_eq!(stock[136..143], [0xE6, 0x06, 0x05, 0x01, 0x8A, 0x60, 0x12]);
        // A known peak overrides all three; the EOTF and descriptor bytes never move.
        let lum = edid::ClientLuminance {
            max_nits: 400,
            max_frame_avg_nits: 400,
            min_millinits: 50,
        };
        let coded = edid::generate(1, lum, None);
        assert_eq!(coded[136..140], [0xE6, 0x06, 0x05, 0x01]);
        assert_eq!(coded[140], edid::cta_max_luminance_code(400));
        // Unknown frame-average and unknown min both code as 0 = "no data" on the wire.
        let partial = edid::generate(
            1,
            edid::ClientLuminance {
                max_nits: 400,
                ..Default::default()
            },
            None,
        );
        assert_eq!(partial[141], 0);
        assert_eq!(partial[142], 0);
    }

    /// One advertised resolution, spelled short enough for the mode-list tests to read.
    fn mode(width: u32, height: u32, refresh_rates: &[u32]) -> vdisplay::Mode {
        vdisplay::Mode {
            width,
            height,
            refresh_rates: refresh_rates.to_vec(),
        }
    }

    #[test]
    fn default_modes_lead_with_1080p_then_720p() {
        let d = vdisplay::default_modes();
        assert_eq!(
            d,
            vec![mode(1920, 1080, &[60, 120]), mode(1280, 720, &[60])]
        );
        // flatten walks resolutions in list order, refresh rates within each.
        let flat: Vec<_> = vdisplay::flatten(&d)
            .map(|i| (i.width, i.height, i.refresh_rate))
            .collect();
        assert_eq!(flat, [(1920, 1080, 60), (1920, 1080, 120), (1280, 720, 60)]);
    }

    #[test]
    fn union_modes_dedupes_by_resolution_and_stops_at_the_cap() {
        // The head keeps its place, and a duplicate resolution is dropped WHOLE: the incoming
        // 60/120 Hz rates are lost rather than merged into the 144 Hz head already there.
        let mut into = vec![mode(1920, 1080, &[144])];
        vdisplay::union_modes(
            &mut into,
            &[mode(1920, 1080, &[60, 120]), mode(1280, 720, &[60])],
        );
        assert_eq!(into, vec![mode(1920, 1080, &[144]), mode(1280, 720, &[60])]);

        // At the cap nothing is appended, and nothing already accumulated is truncated.
        let cap = vdisplay::MODE_LIST_CAP;
        let mut full: Vec<_> = (0..cap as u32).map(|i| mode(640 + i, 480, &[60])).collect();
        vdisplay::union_modes(&mut full, &[mode(3840, 2160, &[60])]);
        assert_eq!(full.len(), cap);
        assert!(!full.contains(&mode(3840, 2160, &[60])));

        // One slot free: the first new resolution takes it and the rest fall off — the merge
        // stops AT the cap, so it is the later candidates that are lost.
        let mut nearly: Vec<_> = (0..cap as u32 - 1)
            .map(|i| mode(640 + i, 480, &[60]))
            .collect();
        vdisplay::union_modes(
            &mut nearly,
            &[mode(3840, 2160, &[60]), mode(2560, 1440, &[60])],
        );
        assert_eq!(nearly.len(), cap);
        assert_eq!(nearly[cap - 1], mode(3840, 2160, &[60]));
    }

    #[test]
    fn monitor_ids_honour_the_preferred_then_take_the_lowest_free() {
        // A free preferred id inside the connector range wins.
        assert_eq!(vdisplay::resolve_id(&[1, 2], 7), 7);
        // A collision falls back to auto — the live holder is never displaced.
        assert_eq!(vdisplay::resolve_id(&[1, 2], 2), 3);
        // 0 (anonymous / TOFU / GameStream) and 16+ (past MaxMonitorsSupported) fall back too.
        assert_eq!(vdisplay::resolve_id(&[1, 2], 0), 3);
        assert_eq!(vdisplay::resolve_id(&[1, 2], 16), 3);

        // Lowest free, not next-highest: a departed monitor's id is refilled.
        assert_eq!(vdisplay::alloc_monitor_id(&[]), 1);
        assert_eq!(vdisplay::alloc_monitor_id(&[1, 3, 4]), 2);

        // Pigeonhole over `1..=len + 1` always finds one — but is NOT clamped to the 1..=15
        // range `resolve_id` enforces for a preferred id, so a full adapter allocates past it.
        let fifteen: Vec<u32> = (1..=15).collect();
        assert_eq!(vdisplay::alloc_monitor_id(&fifteen), 16);
        let sixteen: Vec<u32> = (1..=16).collect();
        assert_eq!(vdisplay::alloc_monitor_id(&sixteen), 17);
    }

    #[test]
    fn container_guids_are_unique_per_monitor_id() {
        let all: Vec<_> = (1u32..=16).map(vdisplay::container_guid).collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // Only Data1 and the last two Data4 bytes carry the id; the rest is a fixed prefix.
        assert_eq!(
            vdisplay::container_guid(1),
            (
                0x7066_7665,
                0x7044,
                0x5350,
                [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x00, 0x01]
            )
        );
        assert_eq!(vdisplay::container_guid(0x1234).3[6..], [0x12, 0x34]);
    }

    #[test]
    fn signal_info_numerics_match_the_ddi_formula() {
        // Monitor modes pass divider 0, target modes 1 — the ONLY difference between the two.
        let m = vdisplay::signal_info(1920, 1080, 60, 0);
        assert_eq!(m.pixel_rate, 60 * 1920 * 1080);
        assert_eq!(m.h_sync_num, 60 * 1080);
        assert_eq!(m.v_sync_num, 60);
        assert_eq!(m.video_standard, 255);
        assert_eq!(
            vdisplay::signal_info(1920, 1080, 60, 1).video_standard,
            255 | (1 << 16)
        );

        // 8K240's pixel rate (7.96e9) is why that field is u64; the hSync numerator still fits
        // a u32 with orders of magnitude to spare.
        let big = vdisplay::signal_info(7680, 4320, 240, 1);
        assert_eq!(big.pixel_rate, 7_962_624_000);
        assert!(big.pixel_rate > u64::from(u32::MAX));
        assert_eq!(big.h_sync_num, 240 * 4320);

        // The hSync saturation is unreachable through `valid_mode` (16384 · 1000 fits); it only
        // keeps an unvalidated tuple from panicking inside the extern-"C" mode DDI.
        assert!(vdisplay::signal_info(16384, 16384, 1000, 0).h_sync_num < u32::MAX);
        assert_eq!(
            vdisplay::signal_info(1, u32::MAX, u32::MAX, 0).h_sync_num,
            u32::MAX
        );
    }

    #[test]
    fn valid_mode_takes_the_edges_and_rejects_zero() {
        assert!(vdisplay::valid_mode(1, 1, 1));
        assert!(vdisplay::valid_mode(1920, 1080, 60));
        assert!(vdisplay::valid_mode(16384, 16384, 1000));
        for bad in [
            (0, 1080, 60),
            (1920, 0, 60),
            (1920, 1080, 0),
            (16385, 1080, 60),
            (1920, 16385, 60),
            (1920, 1080, 1001),
        ] {
            assert!(!vdisplay::valid_mode(bad.0, bad.1, bad.2), "{bad:?}");
        }
    }

    #[test]
    fn frame_token_roundtrips() {
        for (g, s, slot) in [
            (1u32, 0u32, 0u8),
            (5, 12_345, 3),
            (frame::FrameToken::GENERATION_MASK, 0xFFFF_FFFF, 5),
            (0, 1, 255),
        ] {
            let t = frame::FrameToken {
                generation: g,
                seq: s,
                slot,
            };
            assert_eq!(frame::FrameToken::unpack(t.pack()), t);
        }
    }

    #[test]
    fn frame_token_packing_matches_legacy_layout() {
        // Legacy packing was `(gen<<40)|(seq<<8)|slot` by hand; lock the bit positions.
        let t = frame::FrameToken {
            generation: 7,
            seq: 42,
            slot: 3,
        };
        assert_eq!(t.pack(), (7u64 << 40) | (42u64 << 8) | 3u64);
    }

    #[test]
    fn opened_detail_roundtrips_and_saturates() {
        use frame::{pack_opened_detail, unpack_opened_detail, OPENED_DETAIL_LIVE};
        // Zero counters still stamp LIVE — "attached, nothing offered yet" is information.
        assert_eq!(pack_opened_detail(0, 0), OPENED_DETAIL_LIVE);
        assert_eq!(unpack_opened_detail(pack_opened_detail(0, 0)), Some((0, 0)));
        assert_eq!(
            unpack_opened_detail(pack_opened_detail(1234, 567)),
            Some((1234, 567))
        );
        // Saturation: 15-bit offered, 16-bit mismatched.
        assert_eq!(
            unpack_opened_detail(pack_opened_detail(u32::MAX, u32::MAX)),
            Some((0x7FFF, 0xFFFF))
        );
        // Any value without the LIVE bit carries no information.
        assert_eq!(unpack_opened_detail(0), None);
        assert_eq!(unpack_opened_detail(0x7FFF_FFFF), None);
    }

    /// The v3 tail is reachable only when BOTH the stamped layout and the declared section say so
    /// — every old/new pairing from the plan's compatibility contract, pinned.
    #[test]
    fn v3_tail_gate_covers_every_pairing() {
        use frame::{v3_readable, HEADER_V2_SIZE, HEADER_V3_SIZE};
        let (v2, v3) = (HEADER_V2_SIZE as u32, HEADER_V3_SIZE as u32);
        // new host + new driver: the only pairing that opens the tail.
        assert!(v3_readable(3, v3));
        // old host (v2 header, request `_pad` = 0) + new driver: prefix only.
        assert!(!v3_readable(2, 0));
        // new host that stamped v3 but a short section (a bug, or a truncated mapping): refused.
        assert!(!v3_readable(3, v2));
        assert!(!v3_readable(3, v3 - 1));
        // a section large enough but a v2 layout stamp: refused — the stamp is the contract.
        assert!(!v3_readable(2, v3));
        // a future host: still readable as v3 (additive versions).
        assert!(v3_readable(4, v3 + 64));
    }

    /// Capabilities activate only where both sides agree; an unknown bit on one side is inert.
    #[test]
    fn capabilities_negotiate_by_intersection() {
        use frame::*;
        let host = CAP_RING_HEALTH_V3 | CAP_FENCE_RING | CAP_SOURCE_SEQUENCE_QPC;
        let old_driver = 0;
        assert_eq!(
            negotiate(host, old_driver),
            0,
            "a pre-v3 driver advertises nothing"
        );
        let driver = CAP_RING_HEALTH_V3 | CAP_SOURCE_SEQUENCE_QPC | CAP_SWAPCHAIN_RESET;
        let n = negotiate(host, driver);
        assert_eq!(n, CAP_RING_HEALTH_V3 | CAP_SOURCE_SEQUENCE_QPC);
        assert_eq!(n & CAP_FENCE_RING, 0, "fence transport needs BOTH sides");
        assert_eq!(
            n & CAP_SWAPCHAIN_RESET,
            0,
            "an actuator the host did not ask for stays off"
        );
    }

    /// State words decode exactly; anything unrecognised is Dead, never Active by accident.
    #[test]
    fn health_state_decodes_conservatively() {
        use frame::HealthState;
        assert_eq!(HealthState::from_u32(0), HealthState::Initializing);
        assert_eq!(HealthState::from_u32(1), HealthState::Active);
        assert_eq!(HealthState::from_u32(2), HealthState::Rebuilding);
        assert_eq!(HealthState::from_u32(3), HealthState::Dead);
        assert_eq!(HealthState::from_u32(0xdead_beef), HealthState::Dead);
        assert!(frame::snapshot_consistent(1, 1));
        assert!(
            !frame::snapshot_consistent(1, 2),
            "a state flip mid-read tears the snapshot"
        );
    }

    #[test]
    fn shared_header_is_pod_and_152_bytes() {
        let mut h = frame::SharedHeader::zeroed();
        h.magic = frame::MAGIC;
        h.width = 5120;
        h.height = 1440;
        h.target_id = 262;
        h.drain_heartbeat_qpc = 0x1234_5678_9abc_def0;
        h.source_sequence = 77;
        let bytes = bytemuck::bytes_of(&h);
        assert_eq!(bytes.len(), frame::HEADER_V3_SIZE);
        // The v2 prefix is byte-identical: a v2 driver mapping the first 88 bytes sees its layout.
        assert_eq!(
            bytes[80..88],
            0u64.to_le_bytes(),
            "offered_total sits at the v2 tail end"
        );
        assert_eq!(
            bytes[120..128],
            77u64.to_le_bytes(),
            "source_sequence is v3-only, at 120"
        );
        let back: frame::SharedHeader = *bytemuck::from_bytes(bytes);
        assert_eq!(back.magic, frame::MAGIC);
        assert_eq!(back.width, 5120);
        assert_eq!(back.height, 1440);
        // Monitor binding occupies the old `_pad` slot at offset 28 — a v2 host left it zero there.
        assert_eq!(bytes[28..32], 262u32.to_le_bytes());
        // Telemetry tail is appended; the first 64 bytes are the v1 layout.
        assert_eq!(bytes[64..72], 0x1234_5678_9abc_def0u64.to_le_bytes());
        assert_eq!(back.last_acquire_qpc, 0);
        assert_eq!(back.offered_total, 0);
    }

    #[test]
    fn attach_check_binds_ring_to_monitor() {
        use frame::{check_attach, AttachReject, MAGIC};
        assert_eq!(check_attach(MAGIC, 7, 262, 7, 262), Ok(()));
        // Missing magic / superseded generation → Stale. Staleness wins over a binding mismatch.
        assert_eq!(
            check_attach(0, 7, 262, 7, 262),
            Err(AttachReject::Stale),
            "no magic"
        );
        assert_eq!(
            check_attach(MAGIC, 8, 262, 7, 262),
            Err(AttachReject::Stale),
            "recreated ring"
        );
        assert_eq!(
            check_attach(0, 8, 999, 7, 262),
            Err(AttachReject::Stale),
            "stale outranks bind"
        );
        // A fresh, magic-valid ring naming a different monitor fails closed.
        assert_eq!(
            check_attach(MAGIC, 7, 999, 7, 262),
            Err(AttachReject::BindMismatch)
        );
        // A v2-host header (target_id = 0) also fails closed; do not rely on the GET_INFO handshake.
        assert_eq!(
            check_attach(MAGIC, 7, 0, 7, 262),
            Err(AttachReject::BindMismatch)
        );
    }

    #[test]
    fn control_structs_roundtrip_through_bytes() {
        let req = control::AddRequest {
            session_id: 0xDEAD_BEEF_CAFE_F00D,
            width: 3840,
            height: 2160,
            refresh_hz: 120,
            preferred_monitor_id: 7,
            max_luminance_nits: 800,
            max_frame_avg_nits: 400,
            min_luminance_millinits: 50, // 0.05 nits
            hw_cursor: 1,
        };
        let bytes = bytemuck::bytes_of(&req);
        assert_eq!(bytes.len(), 40);
        assert_eq!(*bytemuck::from_bytes::<control::AddRequest>(bytes), req);
        // preferred_monitor_id occupies the old `_reserved` slot at offset 20.
        assert_eq!(bytes[20..24], 7u32.to_le_bytes());
        // Luminance tail rides after the legacy boundary; a zero-filled tail decodes as unknown.
        assert_eq!(bytes[24..28], 800u32.to_le_bytes());
        let mut legacy = [0u8; 40];
        legacy[..control::ADD_REQUEST_LEGACY_SIZE]
            .copy_from_slice(&bytes[..control::ADD_REQUEST_LEGACY_SIZE]);
        // `pod_read_unaligned`, not `from_bytes`: `legacy` is `[u8; 40]` (align 1) but
        // `AddRequest` is align 8. `from_bytes` panics unless the buffer happens to be 8-aligned.
        let old = bytemuck::pod_read_unaligned::<control::AddRequest>(&legacy);
        assert_eq!(old.preferred_monitor_id, 7);
        assert_eq!(
            (
                old.max_luminance_nits,
                old.max_frame_avg_nits,
                old.min_luminance_millinits
            ),
            (0, 0, 0)
        );

        let reply = control::AddReply {
            adapter_luid_low: 0x1234_5678,
            adapter_luid_high: -2,
            target_id: 262,
            resolved_monitor_id: 7,
            wudf_pid: 4242,
            cursor_excluded: 1,
        };
        let rbytes = bytemuck::bytes_of(&reply);
        assert_eq!(rbytes.len(), 24);
        assert_eq!(*bytemuck::from_bytes::<control::AddReply>(rbytes), reply);
        // resolved_monitor_id occupies the old `_reserved` slot at offset 12.
        assert_eq!(rbytes[12..16], 7u32.to_le_bytes());
        // Duplication-target pid trails at offset 16.
        assert_eq!(rbytes[16..20], 4242u32.to_le_bytes());
        // cursor_excluded rides after the legacy boundary; a zero-filled tail reads as unknown.
        assert_eq!(rbytes[20..24], 1u32.to_le_bytes());
        assert_eq!(control::ADD_REPLY_LEGACY_SIZE, 20);
    }

    #[test]
    fn frame_channel_request_roundtrips_through_bytes() {
        let mut req = control::SetFrameChannelRequest {
            target_id: 262,
            generation: 3,
            ring_len: frame::RING_LEN,
            header_bytes: frame::HEADER_V3_SIZE as u32,
            header_handle: 0x0000_0000_0000_1a2c,
            event_handle: 0x0000_0000_0000_1b30,
            texture_handles: [0; control::RING_LEN_USIZE],
        };
        for (k, t) in req.texture_handles.iter_mut().enumerate() {
            *t = 0x2000 + k as u64 * 4;
        }
        let bytes = bytemuck::bytes_of(&req);
        assert_eq!(bytes.len(), 32 + 8 * control::RING_LEN_USIZE);
        assert_eq!(
            *bytemuck::from_bytes::<control::SetFrameChannelRequest>(bytes),
            req
        );
        // Handle values ride 8-aligned from offset 16 (header, event, then the ring).
        assert_eq!(bytes[16..24], 0x1a2cu64.to_le_bytes());
        assert_eq!(bytes[24..32], 0x1b30u64.to_le_bytes());
        assert_eq!(bytes[32..40], 0x2000u64.to_le_bytes());
    }

    #[test]
    fn update_modes_request_roundtrips_and_versions_cohere() {
        let req = control::UpdateModesRequest {
            session_id: 42,
            width: 2560,
            height: 1409, // arbitrary — the in-place path serves window-drag modes
            refresh_hz: 120,
            _reserved: 0,
        };
        let bytes = bytemuck::bytes_of(&req);
        assert_eq!(bytes.len(), 24);
        assert_eq!(
            *bytemuck::from_bytes::<control::UpdateModesRequest>(bytes),
            req
        );
        assert_eq!(bytes[8..12], 2560u32.to_le_bytes());
        // v4–v6 are additive over v3, so the host floor stays at 3.
        assert_eq!(PROTOCOL_VERSION, 6);
        assert_eq!(MIN_DRIVER_PROTOCOL_VERSION, 3);
    }

    #[test]
    fn cursor_shm_layout_is_pinned() {
        use cursor::*;
        // Header must leave the shape offset intact whatever grows inside `_reserved`.
        assert_eq!(core::mem::size_of::<CursorShm>(), 64);
        assert_eq!(CURSOR_SHM_SIZE, 64 + 256 * 256 * 4);
        assert_eq!(CURSOR_MAGIC, u32::from_le_bytes(*b"PFCU"));
        let hdr = CursorShm {
            magic: CURSOR_MAGIC,
            seq: 2,
            visible: 1,
            cursor_type: CURSOR_TYPE_ALPHA,
            x: -3,
            y: 7,
            shape_id: 42,
            width: 32,
            height: 32,
            pitch: 128,
            hot_x: 4,
            hot_y: 5,
            _reserved: [0; 4],
        };
        let bytes = bytemuck::bytes_of(&hdr);
        assert_eq!(*bytemuck::from_bytes::<CursorShm>(bytes), hdr);
        assert_eq!(bytes[16..20], (-3i32).to_le_bytes());
    }

    #[test]
    fn gamepad_names_and_magics_are_stable() {
        assert_eq!(gamepad::xusb_boot_name(0), "Global\\pfxusb-boot-0");
        assert_eq!(gamepad::pad_boot_name(2), "Global\\pfds-boot-2");
        // Lock the exact u32 magics the shipped host/drivers use.
        assert_eq!(gamepad::XUSB_MAGIC, 0x5558_4650);
        assert_eq!(gamepad::PAD_MAGIC, 0x5046_4453);
        // "PFBT" little-endian.
        assert_eq!(gamepad::BOOT_MAGIC.to_le_bytes(), *b"PFBT");
    }

    #[test]
    fn pad_bootstrap_roundtrips_through_bytes() {
        let b = gamepad::PadBootstrap {
            magic: gamepad::BOOT_MAGIC,
            host_proto: gamepad::GAMEPAD_PROTO_VERSION,
            driver_pid: 1234,
            driver_proto: gamepad::GAMEPAD_PROTO_VERSION,
            data_handle: 0x0000_0000_0000_2a4c,
            handle_pid: 1234,
            handle_seq: 7,
        };
        let bytes = bytemuck::bytes_of(&b);
        assert_eq!(bytes.len(), 32);
        assert_eq!(*bytemuck::from_bytes::<gamepad::PadBootstrap>(bytes), b);
        // Handle value rides 8-aligned at offset 16; seq trails at 28 (written last).
        assert_eq!(bytes[16..24], 0x2a4cu64.to_le_bytes());
        assert_eq!(bytes[28..32], 7u32.to_le_bytes());
    }

    #[test]
    fn ctl_codes_are_contiguous_and_distinct() {
        assert_eq!(control::IOCTL_ADD, ctl_code(0x900));
        let all = [
            control::IOCTL_ADD,
            control::IOCTL_REMOVE,
            control::IOCTL_SET_RENDER_ADAPTER,
            control::IOCTL_PING,
            control::IOCTL_GET_INFO,
            control::IOCTL_CLEAR_ALL,
            control::IOCTL_SET_FRAME_CHANNEL,
            control::IOCTL_UPDATE_MODES,
            control::IOCTL_SET_CURSOR_CHANNEL,
            control::IOCTL_SET_CURSOR_FORWARD,
            control::IOCTL_ENCODE_PROBE_ARM,
            control::IOCTL_ENCODE_PROBE_STATUS,
            encode::IOCTL_SET_ENCODE,
            encode::IOCTL_ENCODE_CTL,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(encode::IOCTL_ENCODE_CTL, ctl_code(0x90D));
    }

    #[test]
    fn encode_probe_structs_roundtrip_through_bytes() {
        let req = control::EncodeProbeRequest {
            target_id: 7,
            backend: 1,
            codec: 2,
            input: 0,
            frames: 300,
            bitrate_kbps: 20_000,
            fps: 60,
            flags: 0,
        };
        let bytes = bytemuck::bytes_of(&req);
        assert_eq!(bytes.len(), 32);
        assert_eq!(
            bytemuck::pod_read_unaligned::<control::EncodeProbeRequest>(bytes),
            req
        );
        let mut name = [0u8; 32];
        name[..4].copy_from_slice(b"open");
        let reply = control::EncodeProbeReply {
            state: 4,
            backend_opened: 0,
            frames_submitted: 0,
            aus: 0,
            bytes: 0x1_0000_0001,
            open_us: 1234,
            first_au_us: 0,
            mean_submit_to_au_us: 0,
            max_submit_to_au_us: 0,
            drops: 3,
            error: -1,
            name,
        };
        let bytes = bytemuck::bytes_of(&reply);
        assert_eq!(bytes.len(), 80);
        // `bytes` rides 8-aligned at 16; `error` at 44; the tag opens at 48.
        assert_eq!(bytes[16..24], 0x1_0000_0001u64.to_le_bytes());
        assert_eq!(bytes[44..48], (-1i32).to_le_bytes());
        assert_eq!(&bytes[48..52], b"open");
        assert_eq!(
            bytemuck::pod_read_unaligned::<control::EncodeProbeReply>(bytes),
            reply
        );
    }

    #[test]
    fn cta_luminance_codes_hit_the_reference_points() {
        // Historical built-in EDID block: 0x8A ≈ 993 nits, 0x60 = 400 nits (exact), 0x12 ≈ 0.05 nit.
        assert_eq!(edid::cta_max_millinits(0x60), 400_000); // 50·2^3 exactly
        assert_eq!(edid::cta_max_millinits(0x8A) / 1000, 993);
        assert_eq!(edid::cta_max_luminance_code(400), 0x60);
        // 0x8A decodes to 993.481 nits; 994 is the smallest whole-nit input that reaches it.
        assert_eq!(edid::cta_max_luminance_code(994), 0x8A);
        assert_eq!(edid::cta_min_luminance_code(50, 0x8A), 0x12); // 0.05 nits @ a 993-nit max
                                                                  // Never advertise brighter than the panel. 1000 nits sits between 138 (993) and 139 (~1015).
        assert_eq!(edid::cta_max_luminance_code(1000), 138);
        assert!(edid::cta_max_millinits(edid::cta_max_luminance_code(1000)) <= 1_000_000);
        // Every real code decodes at or below its input, within one step (~2.2%).
        // Starts above code 1's 51.094 nits — beneath that the documented clamp-to-1 wins.
        for nits in [52u32, 80, 120, 250, 400, 604, 800, 1_499, 4_000, 10_000] {
            let c = edid::cta_max_luminance_code(nits);
            let dec = edid::cta_max_millinits(c);
            assert!(dec <= nits as u64 * 1000, "{nits} → {c} decoded {dec}");
            assert!(
                dec * 1023 / 1000 >= nits as u64 * 1000,
                "{nits} → {c} more than a step low"
            );
        }
        // 0/tiny stays a valid on-wire code (callers gate on nits > 0); the ceiling saturates at 255.
        assert_eq!(edid::cta_max_luminance_code(0), 1);
        assert_eq!(edid::cta_max_luminance_code(u32::MAX), 255);
        // Min-luminance: 0 = unknown/true black stays 0; a floor brighter than the max clamps.
        assert_eq!(edid::cta_min_luminance_code(0, 0x8A), 0);
        assert_eq!(edid::cta_min_luminance_code(u32::MAX, 1), 255);
        // HDR400: max 400 nits / min 0.4 nits.
        let max_c = edid::cta_max_luminance_code(400);
        let min_c = edid::cta_min_luminance_code(400, max_c);
        // L_min = L_max·(cv/255)²/100 — must come back within ~10% of 0.4 nits.
        let back =
            edid::cta_max_millinits(max_c) * (min_c as u64 * min_c as u64) / (255 * 255) / 100;
        assert!((360..=440).contains(&back), "min decoded {back} millinits");
    }

    #[test]
    fn mouse_report_and_names_are_stable() {
        assert_eq!(mouse::mouse_boot_name(0), "Global\\pfmouse-boot-0");
        // "PFMO" LE, and never colliding with a pad magic.
        assert_eq!(mouse::MOUSE_MAGIC.to_le_bytes(), *b"PFMO");
        assert_ne!(mouse::MOUSE_MAGIC, gamepad::XUSB_MAGIC);
        assert_ne!(mouse::MOUSE_MAGIC, gamepad::PAD_MAGIC);
        let r = mouse::input_report(0b0000_0101, 0x1234, 0x7FFF, -3, 7);
        assert_eq!(r, [0x01, 0x05, 0x34, 0x12, 0xFF, 0x7F, 0xFD, 0x07]);
        // Axes clamp to the 15-bit logical max; buttons to the declared 5.
        let r = mouse::input_report(0xFF, 0xFFFF, 0, 0, 0);
        assert_eq!((r[1], r[2], r[3]), (0x1F, 0xFF, 0x7F));
        // A zeroed section reads as nothing published (`in_seq` 0).
        let shm = mouse::MouseShm::zeroed();
        assert_eq!(shm.in_seq, 0);
        assert_eq!(bytemuck::bytes_of(&shm).len(), 64);
    }

    #[test]
    fn guid_is_not_sudovda() {
        const SUDOVDA: u128 = 0xE5BC_C234_1E0C_418A_A0D4_EF8B_7501_414D;
        assert_ne!(PF_VDISPLAY_INTERFACE_GUID_U128, SUDOVDA);
    }

    /// Both wire forms (IOCTL struct and HID indexed-string) round-trip; malformed shapes refuse
    /// rather than half-parse into a pid.
    #[test]
    fn channel_proof_round_trips_in_both_wire_forms() {
        use gamepad::*;
        let proof = ChannelProof::new(2, 4242);
        assert_eq!(proof.magic, PROOF_MAGIC);
        assert_eq!(PROOF_MAGIC, u32::from_le_bytes(*b"PFCP"));
        assert_eq!(proof.proto, GAMEPAD_PROTO_VERSION);

        let bytes = bytemuck::bytes_of(&proof);
        assert_eq!(bytes.len(), 16);
        assert_eq!(*bytemuck::from_bytes::<ChannelProof>(bytes), proof);

        let s = proof.to_hid_string();
        assert_eq!(s, alloc::format!("PFCP:{GAMEPAD_PROTO_VERSION}:2:4242"));
        assert_eq!(ChannelProof::from_hid_string(&s), Some(proof));

        // Every malformed shape parses to None.
        for bad in [
            "",
            "PFCP",
            "PFCP:",
            "PFCP:3:0",        // truncated read
            "PFCP:3:0:4242:9", // trailing field we never mint
            "PFCP:3:0:-1",     // not a u32
            "PFCP:3:0:0x10",   // not decimal
            "PFCP:3:0: 4242",  // whitespace is not trimmed away into a valid pid
            "NOPE:3:0:4242",   // another driver answered this string index
            "pfcp:3:0:4242",   // prefix is case-sensitive
        ] {
            assert_eq!(
                ChannelProof::from_hid_string(bad),
                None,
                "malformed proof {bad:?} must not parse"
            );
        }
    }

    /// Pin each `check` refusal: foreign driver, version skew, wrong-devnode (would cross-wire pads).
    #[test]
    fn channel_proof_check_refuses_everything_it_should() {
        use gamepad::*;
        assert_eq!(ChannelProof::new(0, 1234).check(0), Ok(1234));
        assert_eq!(ChannelProof::new(3, 1234).check(3), Ok(1234));

        // Right shape, wrong pad: the interface lookup resolved another pad's devnode.
        assert!(ChannelProof::new(1, 1234).check(0).is_err());
        let mut foreign = ChannelProof::new(0, 1234);
        foreign.magic = 0xDEAD_BEEF;
        assert!(foreign.check(0).is_err());
        // Version skew must fail closed, not "probably compatible".
        let mut old = ChannelProof::new(0, 1234);
        old.proto = GAMEPAD_PROTO_VERSION - 1;
        assert!(old.check(0).is_err());
        // pid 0 is never a duplication target.
        assert!(ChannelProof::new(0, 0).check(0).is_err());
    }

    /// v2 driver answers no proof and a v2 host never asks — version must have moved.
    #[test]
    fn gamepad_proto_is_at_the_channel_proof_version() {
        assert_eq!(gamepad::GAMEPAD_PROTO_VERSION, 3);
    }

    /// Feature-report framing: report id in byte 0, proof in 1..17, zero pad; short reads refuse.
    #[test]
    fn channel_proof_feature_report_round_trips_and_refuses_short_reads() {
        use gamepad::*;
        let proof = ChannelProof::new(1, 4242);
        let rep = proof
            .to_feature_report(HID_FEATURE_REPORT_CHANNEL_PROOF, 64)
            .expect("64 bytes is plenty");
        assert_eq!(rep.len(), 64);
        assert_eq!(
            rep[0], 0x85,
            "byte 0 is the report id, as every HID feature reply is"
        );
        assert!(rep[17..].iter().all(|&b| b == 0), "tail is zero padding");
        assert_eq!(ChannelProof::from_feature_report(&rep), Some(proof));

        assert!(proof.to_feature_report(0x85, 17).is_some());
        assert!(proof.to_feature_report(0x85, 16).is_none());
        // A truncated read must not be zero-extended into a pid.
        assert_eq!(ChannelProof::from_feature_report(&rep[..16]), None);
        assert_eq!(ChannelProof::from_feature_report(&[]), None);

        // Two bytes so a stray Steam command cannot collide.
        assert_eq!(DECK_PROOF_CMD.len(), 2);
        assert!(!DECK_PROOF_CMD.starts_with(&[0x83]) && !DECK_PROOF_CMD.starts_with(&[0xAE]));
        assert!(!DECK_PROOF_CMD.starts_with(&[0xEB]) && !DECK_PROOF_CMD.starts_with(&[0x8F]));
    }

    #[test]
    fn triton_devtype_is_the_next_free_slot() {
        assert_eq!(gamepad::DEVTYPE_TRITON, 7);
    }

    /// GET reply echoes the last SET's command; a mismatch makes Steam drop the pad.
    #[test]
    fn triton_feature_reply_echoes_the_queried_command() {
        // Settings write (lizard-off) reads back as a mirror.
        let set = [0x01, 0x87, 0x03, 0x09, 0x00, 0x00];
        let r = triton::feature_reply(&set, "FVPF130200D03", 0x5452_4900);
        assert_eq!(r[0], 0x01);
        assert_eq!(&r[1..6], &[0x87, 0x03, 0x09, 0x00, 0x00]);
    }

    #[test]
    fn triton_feature_reply_synthesizes_attributes_for_0x83() {
        let set = [0x01, 0x83, 0x00];
        let r = triton::feature_reply(&set, "FVPF130200D03", 0x5452_4900);
        assert_eq!(&r[..3], &[0x01, 0x83, 0x19]); // 25-byte TLV payload
        assert_eq!(r[3], 0x01); // first attribute id: product id
                                // Tag-4 TLV carries FW_BUILD_TIME; a stale epoch makes Steam prompt to update firmware.
        assert_eq!(r[18], 0x04);
        assert_eq!(r[19..23], triton::FW_BUILD_TIME.to_le_bytes());
    }

    #[test]
    fn triton_firmware_info_build_time_agrees_with_the_attributes_reply() {
        let set = [0x01, 0xF2, 0x00, 0x00];
        let r = triton::feature_reply(&set, "FVPF130200D03", 0x5452_4900);
        assert_eq!(&r[..4], &[0x01, 0xF2, 0x29, 0x00]);
        // Bytes 4..8 mirror the 0x83 reply's tag-4 build time — Steam may cross-check.
        assert_eq!(r[4..8], triton::FW_BUILD_TIME.to_le_bytes());
    }

    #[test]
    fn triton_input_len_matches_the_descriptor() {
        assert_eq!(triton::input_len(0x42), Some(54));
        assert_eq!(triton::input_len(0x45), Some(46));
        assert_eq!(triton::input_len(0x43), Some(15));
        assert_eq!(triton::input_len(0x44), Some(6));
        assert_eq!(triton::input_len(0x79), Some(2));
        assert_eq!(triton::input_len(0x7B), Some(13));
        assert_eq!(triton::input_len(0x47), None); // BLE-only id, not in the wired descriptor
        assert_eq!(triton::input_len(0x01), None);
    }

    #[test]
    fn triton_out_report_len_matches_the_descriptor_and_bench_table() {
        assert_eq!(triton::out_report_len(0x80), 10);
        assert_eq!(triton::out_report_len(0x81), 8);
        assert_eq!(triton::out_report_len(0x82), 4);
        assert_eq!(triton::out_report_len(0x83), 10);
        assert_eq!(triton::out_report_len(0x84), 9);
        assert_eq!(triton::out_report_len(0x85), 4);
        assert_eq!(triton::out_report_len(0x86), 4);
        assert_eq!(triton::out_report_len(0x87), 64);
        assert_eq!(triton::out_report_len(0x88), 64);
        assert_eq!(triton::out_report_len(0x89), 64);
        // Undeclared ids stay whole (64 = no trim) — never guess a length.
        assert_eq!(triton::out_report_len(0x00), 64);
        assert_eq!(triton::out_report_len(0x8A), 64);
    }

    #[test]
    fn triton_serial_shape_dodges_the_pf_prefix_rejection() {
        let mut s = [0u8; 13];
        triton::serial(3, &mut s);
        assert_eq!(&s, b"FVPF130203D03");
    }

    #[test]
    fn triton_rdesc_is_the_372_byte_capture() {
        assert_eq!(triton::RDESC.len(), 372);
        // Mouse TLC opens it: Usage Page Generic Desktop, Usage Mouse, Collection App, Report ID 0x40.
        assert_eq!(
            &triton::RDESC[..8],
            &[0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x40]
        );
    }

    #[test]
    fn out_feature_bit_round_trips() {
        let tagged = 64u32 | triton::OUT_FEATURE_BIT;
        assert_eq!(triton::out_len(tagged), 64);
        assert!(triton::out_is_feature(tagged));
        assert!(!triton::out_is_feature(64));
        assert_eq!(triton::out_len(64), 64);
    }

    #[test]
    fn encode_ioctls_follow_the_probe_pair() {
        println!(
            "SET_ENCODE {:#x}, ENCODE_CTL {:#x} (after probe {:#x})",
            encode::IOCTL_SET_ENCODE,
            encode::IOCTL_ENCODE_CTL,
            control::IOCTL_ENCODE_PROBE_STATUS,
        );
        assert_eq!(encode::IOCTL_SET_ENCODE, ctl_code(0x90C));
        assert_eq!(encode::IOCTL_ENCODE_CTL, ctl_code(0x90D));
        // METHOD_BUFFERED on FILE_DEVICE_UNKNOWN, like every other op in the space.
        assert_eq!(encode::IOCTL_SET_ENCODE & 0b11, 0);
        assert_eq!(encode::IOCTL_SET_ENCODE >> 16, 0x22);
    }

    #[test]
    fn set_encode_request_layout_is_pinned() {
        use core::mem::{offset_of, size_of};
        use encode::SetEncodeRequest;

        println!(
            "SetEncodeRequest {} bytes: target_id@{} section@{} event@{} hdr_meta@{} \
             backends@{} flags@{}",
            size_of::<SetEncodeRequest>(),
            offset_of!(SetEncodeRequest, target_id),
            offset_of!(SetEncodeRequest, section),
            offset_of!(SetEncodeRequest, event),
            offset_of!(SetEncodeRequest, hdr_meta),
            offset_of!(SetEncodeRequest, backends),
            offset_of!(SetEncodeRequest, flags),
        );
        assert_eq!(size_of::<SetEncodeRequest>(), 120);
        assert_eq!(offset_of!(SetEncodeRequest, target_id), 0);
        assert_eq!(offset_of!(SetEncodeRequest, section), 8);
        assert_eq!(offset_of!(SetEncodeRequest, event), 16);
        assert_eq!(offset_of!(SetEncodeRequest, hdr_meta), 60);
        assert_eq!(offset_of!(SetEncodeRequest, backends), 96);
        assert_eq!(offset_of!(SetEncodeRequest, flags), 112);
        // The HDR blob is exactly `pf_frame::HdrMeta`, which the driver copies through untouched.
        assert_eq!(size_of::<[u8; 28]>(), 28);
    }

    #[test]
    fn set_encode_reply_and_ctl_layouts_are_pinned() {
        use core::mem::{offset_of, size_of};
        use encode::{EncodeCtlRequest, EncoderCapsWire, SetEncodeReply};

        println!(
            "SetEncodeReply {} bytes (caps {} bytes): caps@{} applied@{} error@{} name@{}",
            size_of::<SetEncodeReply>(),
            size_of::<EncoderCapsWire>(),
            offset_of!(SetEncodeReply, caps),
            offset_of!(SetEncodeReply, applied_bitrate_kbps),
            offset_of!(SetEncodeReply, error),
            offset_of!(SetEncodeReply, name),
        );
        assert_eq!(size_of::<EncoderCapsWire>(), 24);
        assert_eq!(size_of::<SetEncodeReply>(), 72);
        assert_eq!(offset_of!(SetEncodeReply, status), 0);
        assert_eq!(offset_of!(SetEncodeReply, backend_opened), 4);
        assert_eq!(offset_of!(SetEncodeReply, caps), 8);
        assert_eq!(offset_of!(SetEncodeReply, applied_bitrate_kbps), 32);
        assert_eq!(offset_of!(SetEncodeReply, error), 36);
        assert_eq!(offset_of!(SetEncodeReply, name), 40);

        println!(
            "EncodeCtlRequest {} bytes: op@{} arg0@{} arg1@{} payload@{}",
            size_of::<EncodeCtlRequest>(),
            offset_of!(EncodeCtlRequest, op),
            offset_of!(EncodeCtlRequest, arg0),
            offset_of!(EncodeCtlRequest, arg1),
            offset_of!(EncodeCtlRequest, payload),
        );
        assert_eq!(size_of::<EncodeCtlRequest>(), 44);
        assert_eq!(offset_of!(EncodeCtlRequest, target_id), 0);
        assert_eq!(offset_of!(EncodeCtlRequest, op), 4);
        assert_eq!(offset_of!(EncodeCtlRequest, arg0), 8);
        assert_eq!(offset_of!(EncodeCtlRequest, arg1), 12);
        assert_eq!(offset_of!(EncodeCtlRequest, payload), 16);
    }

    #[test]
    fn encode_ctl_ops_are_distinct() {
        use encode::*;
        let ops = [
            ENCODE_CTL_REQUEST_KEYFRAME,
            ENCODE_CTL_INVALIDATE_REF_FRAMES,
            ENCODE_CTL_DISTRUST_REFERENCES,
            ENCODE_CTL_RECONFIGURE_BITRATE,
            ENCODE_CTL_SET_HDR_META,
            ENCODE_CTL_RESET,
            ENCODE_CTL_FLUSH,
        ];
        println!("ENCODE_CTL ops: {ops:?}");
        assert_eq!(ops, [1, 2, 3, 4, 5, 6, 7]);
        // `0` stays unassigned: a zeroed request is not a silent keyframe.
        assert!(!ops.contains(&0));
    }

    #[test]
    fn au_section_layout_is_pinned() {
        use core::mem::{offset_of, size_of};
        use encode::au::{self, AuHeader, AuSlot};

        println!(
            "AuHeader {} bytes: latest@{} generation@{} encoder_state@{} last_au_qpc@{} \
             driver_status@{}; AuSlot {} bytes; slot table @{}, heap @{}",
            size_of::<AuHeader>(),
            offset_of!(AuHeader, latest),
            offset_of!(AuHeader, generation),
            offset_of!(AuHeader, encoder_state),
            offset_of!(AuHeader, last_au_qpc),
            offset_of!(AuHeader, driver_status),
            size_of::<AuSlot>(),
            au::SLOT_TABLE_OFFSET,
            au::HEAP_OFFSET,
        );
        assert_eq!(size_of::<AuHeader>(), 128);
        assert_eq!(offset_of!(AuHeader, magic), 0);
        assert_eq!(offset_of!(AuHeader, version), 4);
        assert_eq!(offset_of!(AuHeader, heap_offset), 8);
        assert_eq!(offset_of!(AuHeader, heap_bytes), 12);
        assert_eq!(offset_of!(AuHeader, slot_table_offset), 16);
        assert_eq!(offset_of!(AuHeader, slot_count), 20);
        assert_eq!(offset_of!(AuHeader, latest), 24);
        assert_eq!(offset_of!(AuHeader, generation), 32);
        assert_eq!(offset_of!(AuHeader, wire_seq_base), 36);
        assert_eq!(offset_of!(AuHeader, encoder_state), 40);
        assert_eq!(offset_of!(AuHeader, detached), 44);
        assert_eq!(offset_of!(AuHeader, last_au_qpc), 48);
        assert_eq!(offset_of!(AuHeader, drain_heartbeat_qpc), 56);
        assert_eq!(offset_of!(AuHeader, source_seq), 64);
        assert_eq!(offset_of!(AuHeader, dropped_total), 72);
        assert_eq!(offset_of!(AuHeader, published_total), 80);
        assert_eq!(offset_of!(AuHeader, driver_status), 88);
        assert_eq!(offset_of!(AuHeader, driver_status_detail), 92);
        assert_eq!(offset_of!(AuHeader, _reserved), 96);

        assert_eq!(size_of::<AuSlot>(), 32);
        assert_eq!(offset_of!(AuSlot, offset), 0);
        assert_eq!(offset_of!(AuSlot, len), 4);
        assert_eq!(offset_of!(AuSlot, wire_seq), 8);
        assert_eq!(offset_of!(AuSlot, source_seq), 12);
        assert_eq!(offset_of!(AuSlot, qpc_pts), 16);
        assert_eq!(offset_of!(AuSlot, flags), 24);
        assert_eq!(offset_of!(AuSlot, state), 28);

        assert_eq!(au::slot_offset(0), 128);
        assert_eq!(au::slot_offset(15), 128 + 15 * 32);
        assert_eq!(au::slot_offset(au::AU_SLOTS as usize), au::HEAP_OFFSET);
        assert_eq!(au::HEAP_OFFSET, 640);
        assert_eq!(&au::AU_MAGIC.to_le_bytes(), b"PFAU");
        assert_ne!(au::AU_MAGIC, frame::MAGIC);
    }

    #[test]
    fn au_flag_bits_and_slot_states_are_distinct() {
        use encode::au::*;
        let flags = [
            AU_FIRST,
            AU_LAST,
            AU_KEYFRAME,
            AU_RECOVERY_ANCHOR,
            AU_CHUNK_ALIGNED,
        ];
        println!("AU flags: {flags:?}; states: {FREE} {PUBLISHED} {READING}");
        assert_eq!(flags, [1, 2, 4, 8, 16]);
        assert_eq!(flags.iter().fold(0, |a, b| a | b), 0b1_1111);
        // A whole non-key AU is FIRST|LAST — the two must not alias.
        assert_eq!(AU_FIRST | AU_LAST, 3);
        assert_eq!([FREE, PUBLISHED, READING], [0, 1, 2]);
        assert_eq!(
            [
                ENCODER_CLOSED,
                ENCODER_OPEN,
                ENCODER_ENCODING,
                ENCODER_WEDGED
            ],
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn heap_and_section_sizing_match_the_plan() {
        use encode::au::{self, heap_bytes_for, section_bytes};

        let mib = 1024 * 1024;
        for (kbps, fps) in [(20_000, 60), (400_000, 60), (400_000, 120)] {
            println!(
                "{kbps} kbps @ {fps} fps -> heap {} B, section {} B",
                heap_bytes_for(kbps, fps),
                section_bytes(heap_bytes_for(kbps, fps)),
            );
        }
        // 20 Mbps is far under the floor — a 16-slot table still needs room.
        assert_eq!(heap_bytes_for(20_000, 60), au::HEAP_MIN_BYTES);
        assert_eq!(au::HEAP_MIN_BYTES, mib);
        // The plan's worst case: 400 Mbps at 60 fps must fit the 8 MiB budget.
        let big = heap_bytes_for(400_000, 60);
        assert!(big <= 8 * mib, "400 Mbps/60 wanted {big} B, over 8 MiB");
        assert!(big > 4 * mib, "and it must not be a token allocation");
        // More frames per second means less bitstream per frame.
        assert!(heap_bytes_for(400_000, 120) < big);
        // Everything lands on the granule, is clamped, and survives absurd inputs.
        assert_eq!(big % au::HEAP_GRANULE, 0);
        assert_eq!(heap_bytes_for(u32::MAX, 1), au::HEAP_MAX_BYTES);
        assert_eq!(heap_bytes_for(400_000, 0), heap_bytes_for(400_000, 1));

        assert_eq!(section_bytes(0), 4096);
        for heap in [au::HEAP_MIN_BYTES, big, au::HEAP_MAX_BYTES] {
            let s = section_bytes(heap);
            assert_eq!(s % au::SECTION_ALIGN, 0);
            assert!(s as usize >= au::HEAP_OFFSET + heap as usize);
            assert!((s as usize) < au::HEAP_OFFSET + heap as usize + 4096);
        }
    }

    #[test]
    fn au_readable_is_fail_closed() {
        use encode::au::{self, au_readable, AuHeader};

        let good = AuHeader {
            magic: au::AU_MAGIC,
            version: au::AU_VERSION,
            heap_offset: au::HEAP_OFFSET as u32,
            heap_bytes: au::heap_bytes_for(50_000, 60),
            slot_table_offset: au::SLOT_TABLE_OFFSET as u32,
            slot_count: au::AU_SLOTS,
            ..AuHeader::zeroed()
        };
        println!("readable header: {good:?}");
        assert!(au_readable(&good));
        // A zeroed section is what an unmapped or half-created one looks like.
        assert!(!au_readable(&AuHeader::zeroed()));
        for bad in [
            AuHeader {
                magic: frame::MAGIC,
                ..good
            },
            AuHeader { version: 6, ..good },
            AuHeader {
                heap_bytes: au::HEAP_MAX_BYTES + 1,
                ..good
            },
            AuHeader {
                heap_bytes: 4096,
                ..good
            },
            AuHeader {
                slot_count: 8,
                ..good
            },
            AuHeader {
                slot_table_offset: 64,
                ..good
            },
            AuHeader {
                heap_offset: 128,
                ..good
            },
        ] {
            assert!(!au_readable(&bad), "accepted {bad:?}");
        }
    }

    #[test]
    fn au_publish_token_round_trips_every_slot() {
        use encode::{au::AU_SLOTS, FrameToken};

        for slot in 0..AU_SLOTS as u8 {
            let t = FrameToken {
                generation: 0x00AB_CDEF,
                seq: 0xDEAD_BEEF,
                slot,
            };
            let packed = t.pack();
            assert_eq!(FrameToken::unpack(packed), t, "slot {slot} did not survive");
        }
        let t = FrameToken {
            generation: 7,
            seq: 15,
            slot: 15,
        };
        println!("token {t:?} packs to {:#x}", t.pack());
        // 16 slots need 4 bits of the 8 the token carries: the table can double without a repack.
        assert_eq!(FrameToken::unpack(t.pack()).slot, 15);
    }

    #[test]
    fn set_encode_request_round_trips_through_bytes() {
        use encode::SetEncodeRequest;

        let mut req = SetEncodeRequest::zeroed();
        req.target_id = 0x1234;
        req.section = 0x0BAD_C0DE_0BAD_C0DE;
        req.event = 0xFEED_FACE_FEED_FACE;
        req.section_bytes = encode::au::section_bytes(encode::au::heap_bytes_for(80_000, 60));
        req.codec = 2;
        req.chroma = 1;
        req.bit_depth = 10;
        req.width = 3840;
        req.height = 2160;
        req.fps = 120;
        req.bitrate_kbps = 80_000;
        req.hdr = 1;
        req.hdr_meta[27] = 0xAB;
        req.wire_chunk_bytes = 0;
        req.wire_seq_base = 0x7FFF_FFFF;
        req.backends = [1, 2, 0, 0];

        let bytes = bytemuck::bytes_of(&req);
        println!("SetEncodeRequest on the wire: {} bytes", bytes.len());
        assert_eq!(bytes.len(), 120);
        let back: SetEncodeRequest = bytemuck::pod_read_unaligned(bytes);
        assert_eq!(back, req);
        // The two handle values sit where the driver reads them, byte for byte.
        assert_eq!(
            u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            req.section
        );
        assert_eq!(
            u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            req.event
        );
        // A zeroed request is inert: no target, no handles, no backend.
        let zero = SetEncodeRequest::zeroed();
        assert_eq!(zero.backends, [0; 4]);
        assert_eq!(bytemuck::bytes_of(&zero), [0u8; 120]);
    }
}
