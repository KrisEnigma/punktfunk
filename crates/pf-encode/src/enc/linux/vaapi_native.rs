//! The native VAAPI encoder behind [`Encoder`]: `pf_libva`'s session, no libavcodec.
//!
//! What the libav path structurally cannot do, this does: a loss is answered by
//! predicting from a slot the client still has (`invalidate_ref_frames`), an ABR
//! step retargets in place (`reconfigure_bitrate`), and HEVC Main 10 carries the
//! HDR10 SEI. Synchronous: `submit` encodes and `poll` hands the AU straight
//! back, as the libav path does at `async_depth=1`.
//!
//! `PUNKTFUNK_ENCODER=vaapi-native` opens it; libav VAAPI stays the default and
//! the A/B oracle until this has been measured against it.

use std::os::fd::AsRawFd as _;

use anyhow::{anyhow, bail, ensure, Context, Result};
use pf_frame::{CapturedFrame, FramePayload, PixelFormat};
use pf_libva::encode::{CodecParams, Encoder as Session};
use pf_libva::{Display, DmabufSource, Libva};
use pf_vaapi::drm::ExportedPlane;
use pf_vaapi::enc_params::SessionParams;
use pf_vaapi::hevc::{HdrStatic, COLOUR_BT2020_PQ, COLOUR_BT709};
use pf_vaapi::vpp;

use super::{ChromaFormat, Codec, EncodedFrame, Encoder, EncoderCaps};
use pf_encode_win::rfi::plan_slot_recovery;

/// Slots a session keeps: how far back a recovery anchor may reach. Four is
/// 66 ms at 60 fps, past a LAN loss report; the level's DPB may allow fewer.
const SLOTS: u8 = 4;

pub struct NativeVaapiEncoder {
    /// `None` between a [`Encoder::reset`] and the submit that reopens.
    session: Option<Session>,
    params: SessionParams,
    codec: CodecParams,
    hdr: Option<HdrStatic>,
    force_kf: bool,
    /// A loss plan's anchor, consumed by the next submit.
    anchor: Option<usize>,
    /// The host's wire index minus the session's own picture count.
    wire_offset: i64,
    frames: u64,
    pending: Option<EncodedFrame>,
    /// 24-bit CPU frames are repacked to 32 here.
    repack: Vec<u8>,
}

impl NativeVaapiEncoder {
    pub fn open(
        codec: Codec,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u64,
        bit_depth: u8,
        chroma: ChromaFormat,
    ) -> Result<Self> {
        ensure!(!chroma.is_444(), "the native VAAPI encoder is 4:2:0 only");
        let ten_bit = bit_depth == 10;
        let codec = match codec {
            Codec::H264 => {
                ensure!(
                    !ten_bit,
                    "ten-bit H.264 is not a path here; HEVC Main 10 is"
                );
                CodecParams::H264
            }
            Codec::H265 => CodecParams::Hevc {
                ten_bit,
                colour: if ten_bit {
                    COLOUR_BT2020_PQ
                } else {
                    COLOUR_BT709
                },
            },
            Codec::Av1 | Codec::PyroWave => {
                bail!("the native VAAPI encoder does not encode {codec:?}")
            }
        };
        let mut params = SessionParams {
            width,
            height,
            fps_num: fps,
            fps_den: 1,
            bitrate_bps: bitrate_bps.min(u64::from(u32::MAX)) as u32,
            slots: SLOTS,
            max_num_reorder_frames: 0,
            initial_qp: 26,
            vbv_frames: super::vbv_frames_env() as f32,
        };
        params.slots = SLOTS.min(params.h264_max_slots());
        let mut this = Self {
            session: None,
            params,
            codec,
            hdr: None,
            force_kf: true,
            anchor: None,
            wire_offset: 0,
            frames: 0,
            pending: None,
            repack: Vec::new(),
        };
        this.open_session()?;
        tracing::info!(
            ?codec,
            width,
            height,
            fps,
            bitrate_bps,
            slots = params.slots,
            "native VAAPI encode session open"
        );
        Ok(this)
    }

    /// Open on the GPU the host chose, never the first node that initialises.
    fn open_session(&mut self) -> Result<()> {
        let va = Libva::load().context("libva")?;
        let node = pf_gpu::linux_render_node();
        let display = Display::open_path(va, &node.to_string_lossy())
            .with_context(|| format!("VAAPI display on {}", node.display()))?;
        let mut session =
            Session::new(display, self.params, self.codec).map_err(|e| anyhow!("{e:#}"))?;
        session.set_hdr(self.hdr);
        self.session = Some(session);
        self.force_kf = true;
        Ok(())
    }
}

impl Encoder for NativeVaapiEncoder {
    fn submit(&mut self, frame: &CapturedFrame) -> Result<()> {
        ensure!(
            frame.width == self.params.width && frame.height == self.params.height,
            "captured frame {}x{} != encoder {}x{}",
            frame.width,
            frame.height,
            self.params.width,
            self.params.height
        );
        if self.session.is_none() {
            self.open_session()?;
        }
        let session = self.session.as_mut().expect("opened above");
        match &frame.payload {
            FramePayload::Cpu(bytes) => {
                let (fourcc, bytes) = packed_rgb(frame.format, bytes, &mut self.repack)?;
                session.submit_packed(bytes, fourcc, frame.width as usize * 4)?;
            }
            FramePayload::Dmabuf(d) => {
                let fd = d.fd.as_raw_fd();
                let mut planes = vec![ExportedPlane {
                    fd,
                    offset: d.offset,
                    stride: d.stride,
                }];
                // NV12's chroma: named, or contiguous below the luma rows.
                if let Some((offset, stride)) = d.plane1 {
                    planes.push(ExportedPlane { fd, offset, stride });
                } else if d.fourcc == vpp::DRM_FORMAT_NV12 {
                    planes.push(ExportedPlane {
                        fd,
                        offset: d.offset + d.stride * frame.height,
                        stride: d.stride,
                    });
                }
                session.submit_dmabuf(&DmabufSource {
                    width: frame.width,
                    height: frame.height,
                    drm_fourcc: d.fourcc,
                    modifier: d.modifier,
                    planes: &planes,
                })?;
            }
            FramePayload::Cuda(_) => bail!(
                "a CUDA frame reached the VAAPI encoder — that payload is NVENC-only; unset \
                 PUNKTFUNK_ZEROCOPY or do not pin PUNKTFUNK_ENCODER=vaapi-native on an NVIDIA host"
            ),
        }
        let pic = match self.anchor.take() {
            Some(slot) => session.encode_anchored(slot)?,
            None => session.encode(self.force_kf)?,
        };
        self.force_kf = false;
        let pts_ns = self.frames * 1_000_000_000 / u64::from(self.params.fps_num.max(1));
        self.frames += 1;
        self.pending = Some(EncodedFrame {
            data: pic.bytes,
            pts_ns,
            keyframe: pic.is_idr,
            recovery_anchor: pic.recovery_anchor,
            chunk_aligned: false,
        });
        Ok(())
    }

    /// The session numbers pictures from zero; the host's index is that plus an
    /// offset, re-learnt on every submit so a rebuild cannot desync the two.
    fn submit_indexed(&mut self, frame: &CapturedFrame, wire_index: u32) -> Result<()> {
        if let Some(s) = &self.session {
            self.wire_offset = i64::from(wire_index) - s.next_wire();
        }
        self.submit(frame)
    }

    fn caps(&self) -> EncoderCaps {
        EncoderCaps {
            supports_rfi: true,
            ..Default::default()
        }
    }

    fn request_keyframe(&mut self) {
        self.force_kf = true;
    }

    fn set_hdr_meta(&mut self, meta: Option<pf_frame::HdrMeta>) {
        self.hdr = meta.map(|m| HdrStatic {
            display_primaries: m.display_primaries,
            white_point: m.white_point,
            max_display_mastering_luminance: m.max_display_mastering_luminance,
            min_display_mastering_luminance: m.min_display_mastering_luminance,
            max_cll: m.max_cll,
            max_fall: m.max_fall,
        });
        if let Some(s) = &mut self.session {
            s.set_hdr(self.hdr);
        }
    }

    /// The slot plan on the session's trusted slots, in the host's wire domain:
    /// taint what the loss touched, anchor the next picture on the newest older
    /// one. `false` when nothing older survives — the caller keyframes.
    fn invalidate_ref_frames(&mut self, first: i64, last: i64) -> bool {
        if first < 0 || last < first {
            return false;
        }
        let Some(session) = &mut self.session else {
            return false;
        };
        let refs: Vec<(usize, i64)> = session
            .slots()
            .into_iter()
            .map(|(slot, wire)| (slot, wire + self.wire_offset))
            .collect();
        let plan = plan_slot_recovery(&refs, first);
        session.distrust(plan.tainted);
        self.anchor = plan.anchor.map(|(slot, _)| slot);
        self.anchor.is_some()
    }

    fn distrust_references(&mut self) {
        if let Some(s) = &mut self.session {
            s.distrust_all();
        }
    }

    fn poll(&mut self) -> Result<Option<EncodedFrame>> {
        Ok(self.pending.take())
    }

    /// Drop the session; the next submit reopens it with an IDR.
    fn reset(&mut self) -> bool {
        self.session = None;
        self.pending = None;
        self.anchor = None;
        self.force_kf = true;
        true
    }

    fn reconfigure_bitrate(&mut self, bps: u64) -> bool {
        let bps = bps.min(u64::from(u32::MAX)) as u32;
        self.params.bitrate_bps = bps;
        if let Some(s) = &mut self.session {
            s.set_bitrate(bps);
        }
        true
    }

    fn applied_bitrate_bps(&self) -> Option<u64> {
        Some(u64::from(self.params.bitrate_bps))
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// The packed-RGB fourcc a CPU frame uploads as, repacking 24-bit to 32 on the
/// way. `Bgrx` uploads as `BGRA`: same bytes, and iHD allocates no `BGRX`.
fn packed_rgb<'a>(
    format: PixelFormat,
    bytes: &'a [u8],
    repack: &'a mut Vec<u8>,
) -> Result<(u32, &'a [u8])> {
    Ok(match format {
        PixelFormat::Bgrx | PixelFormat::Bgra => (vpp::VA_FOURCC_BGRA, bytes),
        PixelFormat::Rgbx | PixelFormat::Rgba => (vpp::VA_FOURCC_RGBA, bytes),
        PixelFormat::X2Rgb10 => (vpp::VA_FOURCC_X2R10G10B10, bytes),
        PixelFormat::X2Bgr10 => (vpp::VA_FOURCC_X2B10G10R10, bytes),
        PixelFormat::Bgr | PixelFormat::Rgb => {
            repack.clear();
            repack.reserve(bytes.len() / 3 * 4);
            for px in bytes.chunks_exact(3) {
                repack.extend_from_slice(px);
                repack.push(255);
            }
            let fourcc = if format == PixelFormat::Bgr {
                vpp::VA_FOURCC_BGRA
            } else {
                vpp::VA_FOURCC_RGBA
            };
            (fourcc, repack.as_slice())
        }
        other => bail!("no native VAAPI ingest for CPU {other:?} frames"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 24-bit CPU frames become 32-bit with an opaque alpha; 32-bit ones upload
    /// as they are.
    #[test]
    fn packed_rgb_repacks_24_bit_only() {
        let mut scratch = Vec::new();
        let (fourcc, out) =
            packed_rgb(PixelFormat::Bgr, &[1, 2, 3, 4, 5, 6], &mut scratch).unwrap();
        assert_eq!(fourcc, vpp::VA_FOURCC_BGRA);
        assert_eq!(out, &[1, 2, 3, 255, 4, 5, 6, 255]);
        let bytes = [9u8; 8];
        let (fourcc, out) = packed_rgb(PixelFormat::Bgrx, &bytes, &mut scratch).unwrap();
        assert_eq!(fourcc, vpp::VA_FOURCC_BGRA);
        assert_eq!(out.as_ptr(), bytes.as_ptr(), "no copy");
        assert!(packed_rgb(PixelFormat::Nv12, &bytes, &mut scratch).is_err());
    }

    /// The trait contract on real silicon: an IDR first, P after, a loss answered
    /// by a recovery anchor and not an IDR, and a bitrate step accepted in place.
    ///
    /// `cargo test -p pf-encode native_vaapi_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a real VAAPI device"]
    fn native_vaapi_smoke() {
        let (w, h) = (320u32, 240u32);
        let mut enc =
            NativeVaapiEncoder::open(Codec::H264, w, h, 60, 4_000_000, 8, ChromaFormat::Yuv420)
                .expect("open");
        assert!(enc.caps().supports_rfi);
        let frame = |i: u32| {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            for px in buf.chunks_exact_mut(4) {
                px.copy_from_slice(&[(i * 8) as u8, 0x40, 0xC0, 0xFF]);
            }
            CapturedFrame {
                provenance: Default::default(),
                width: w,
                height: h,
                pts_ns: u64::from(i) * 16_666_666,
                format: PixelFormat::Bgrx,
                payload: FramePayload::Cpu(buf),
                cursor: None,
            }
        };
        let mut aus = Vec::new();
        for i in 0..10 {
            enc.submit_indexed(&frame(i), 100 + i).expect("submit");
            aus.push(enc.poll().expect("poll").expect("an AU per submit"));
        }
        assert!(aus[0].keyframe);
        assert!(aus[1..]
            .iter()
            .all(|au| !au.keyframe && !au.recovery_anchor));
        // Wire 108 and 109 were lost; the anchor must be 107.
        assert!(
            enc.invalidate_ref_frames(108, 109),
            "a slot older than the loss survives"
        );
        assert!(enc.reconfigure_bitrate(2_000_000));
        assert_eq!(enc.applied_bitrate_bps(), Some(2_000_000));
        enc.submit_indexed(&frame(10), 110).expect("submit");
        let recovery = enc.poll().expect("poll").expect("an AU");
        assert!(recovery.recovery_anchor && !recovery.keyframe);
        // Everything is tainted: no anchor, the caller keyframes.
        assert!(!enc.invalidate_ref_frames(100, 110));
        assert!(enc.reset());
        enc.submit(&frame(11)).expect("submit after reset");
        assert!(
            enc.poll().unwrap().unwrap().keyframe,
            "a rebuild starts with an IDR"
        );
    }
}
