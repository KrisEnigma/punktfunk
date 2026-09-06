//! An H.264 encode session over libva: config, context, surfaces, and one frame in
//! one frame out.
//!
//! The unsafe half of the native VAAPI encoder. What a picture *is* — the SPS, the
//! PPS, the parameter buffers — comes from [`pf_vaapi`], which is pure and tested
//! anywhere; this drives libva with it.
//!
//! Deliberately narrow for now: H.264, CBR, one slice per picture, one reference
//! per picture. One is not a simplification we chose — `VAConfigAttribEncMaxRefFrames`
//! is `l0=1` on radeonsi, so a second list entry would be advertised and ignored.
//! Which one is the point: every reference lives in a long-term slot, and a loss
//! is answered by predicting from a slot the client still has, not by an IDR.

use std::os::raw::c_int;
use std::os::raw::c_void;

use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context as _;
use anyhow::Result;
use pf_vaapi::enc_h264 as vah;
use pf_vaapi::enc_params::packed_slice_header;
use pf_vaapi::enc_params::va_pic_fields;
use pf_vaapi::enc_params::PictureSlice;
use pf_vaapi::enc_params::SessionParams;
use pf_vaapi::vpp::rt_format_for;
use pf_vaapi::vpp::VA_RT_FORMAT_RGB32;
use pf_vaapi::vpp::VA_RT_FORMAT_YUV420;

use crate::vpp::Vpp;
use crate::Display;
use crate::DmabufSource;
use crate::VaBufferId;
use crate::VaContextId;
use crate::VaSurfaceId;
use crate::VA_INVALID_ID;
use crate::VA_PROGRESSIVE;

const VA_PROFILE_H264_HIGH: c_int = 7;
const VA_CONFIG_ATTRIB_RT_FORMAT: u32 = 0;
const VA_CONFIG_ATTRIB_RATE_CONTROL: u32 = 5;
const VA_CONFIG_ATTRIB_ENC_PACKED_HEADERS: u32 = 10;

/// `VAConfigAttrib`: a type/value pair, and the shape `vaCreateConfig` takes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct VaConfigAttrib {
    kind: u32,
    value: u32,
}

/// What one encoded picture came out as.
#[derive(Debug)]
pub struct EncodedPicture {
    pub bytes: Vec<u8>,
    /// `true` when this picture opened a GOP: it carries SPS, PPS and an IDR slice.
    pub is_idr: bool,
    /// Predicted from a pre-loss slot on request: the wire's recovery anchor.
    pub recovery_anchor: bool,
    /// This picture's index in the session, the number a slot remembers it by.
    pub wire: i64,
}

/// One long-term reference the session still holds.
#[derive(Clone, Copy, Debug)]
struct Slot {
    surface: VaSurfaceId,
    wire: i64,
    /// Cleared by [`H264Encoder::distrust`]; an untrusted slot is never predicted
    /// from again, and is replaced in its turn.
    trusted: bool,
}

/// A live encode session. Owns its config, context, surfaces and coded buffer, and
/// releases them in the order libva requires.
pub struct H264Encoder {
    display: Display,
    config: u32,
    context: VaContextId,
    /// Pictures the caller fills and `vaBeginPicture` reads.
    input: Vec<VaSurfaceId>,
    /// Where the driver writes each reconstruction — what `CurrPic` names and what a
    /// later `ReferenceFrames` entry points at. Distinct from the input: the driver
    /// writes these while it is still reading that, and a surface cannot be both.
    /// One more than there are slots, so the picture being written never shares a
    /// surface with a reference being read.
    recon: Vec<VaSurfaceId>,
    /// Reconstruction surfaces no slot holds.
    free: Vec<VaSurfaceId>,
    /// The long-term references, by `LongTermFrameIdx`.
    slots: Vec<Option<Slot>>,
    /// Pictures encoded so far; the next one's `wire`.
    wire: i64,
    coded_buf: VaBufferId,
    /// Ingest: every capture shape is converted into the input surface here.
    vpp: Vpp,
    /// Where CPU RGB lands before conversion, and the fourcc it was made for.
    staging: Option<(VaSurfaceId, u32)>,
    params: SessionParams,
    /// Frames encoded since the last IDR; `frame_num` in the slice header.
    frame_num: u16,
    /// Toggled on every IDR, so two in a row are told apart.
    idr_pic_id: u16,
    /// Rolling index for the picture being encoded.
    next_surface: usize,
}

impl H264Encoder {
    /// Surfaces: one being encoded, one holding the reference, one spare so the
    /// next submit does not wait on the driver releasing the last.
    const SURFACES: usize = 3;

    /// Open a session on `display`.
    pub fn new(display: Display, params: SessionParams) -> Result<Self> {
        let coded_w = i32::from(params.width_in_mbs()) * 16;
        let coded_h = i32::from(params.height_in_mbs()) * 16;

        // Ask the driver what it supports before telling it what we want.
        // `vaCreateConfig` does not reject an attribute it dislikes — it drops it and
        // returns success, and a dropped packed-header attribute means every header
        // the app later supplies is discarded while the encode still reports fine.
        // The only way to know is to query, then pass back what came out.
        let mut probe = [
            VaConfigAttrib {
                kind: VA_CONFIG_ATTRIB_RT_FORMAT,
                value: 0,
            },
            VaConfigAttrib {
                kind: VA_CONFIG_ATTRIB_RATE_CONTROL,
                value: 0,
            },
            VaConfigAttrib {
                kind: VA_CONFIG_ATTRIB_ENC_PACKED_HEADERS,
                value: 0,
            },
        ];
        // SAFETY: `probe` is a live array of exactly `probe.len()` entries the call
        // fills in place; profile and entrypoint are libva enum values.
        let status = unsafe {
            (display.va.get_config_attributes)(
                display.display,
                VA_PROFILE_H264_HIGH,
                vah::VA_ENTRYPOINT_ENC_SLICE,
                probe.as_mut_ptr().cast::<c_void>(),
                probe.len() as c_int,
            )
        };
        display.va.check("vaGetConfigAttributes", status)?;

        let supported_rc = probe[1].value;
        let supported_packed = probe[2].value;
        if supported_rc & vah::VA_RC_CBR == 0 {
            bail!("this driver offers no CBR rate control for H.264 encode");
        }
        // Both kinds are required: the session writes every header itself, and
        // radeonsi drops the packed SPS and PPS of a picture that brings no slice
        // header.
        let want_packed = vah::VA_ENC_PACKED_HEADER_FLAG_SEQUENCE
            | vah::VA_ENC_PACKED_HEADER_FLAG_PICTURE
            | vah::VA_ENC_PACKED_HEADER_FLAG_SLICE;
        let packed = supported_packed & want_packed;
        if packed & vah::VA_ENC_PACKED_HEADER_FLAG_SEQUENCE == 0 {
            bail!("this driver will not take a packed sequence header ({supported_packed:#x})");
        }
        if packed & vah::VA_ENC_PACKED_HEADER_FLAG_SLICE == 0 {
            bail!("this driver will not take a packed slice header ({supported_packed:#x})");
        }

        let attribs = [
            VaConfigAttrib {
                kind: VA_CONFIG_ATTRIB_RT_FORMAT,
                value: VA_RT_FORMAT_YUV420,
            },
            VaConfigAttrib {
                kind: VA_CONFIG_ATTRIB_RATE_CONTROL,
                value: vah::VA_RC_CBR,
            },
            VaConfigAttrib {
                kind: VA_CONFIG_ATTRIB_ENC_PACKED_HEADERS,
                value: packed,
            },
        ];

        let mut config = VA_INVALID_ID;
        // SAFETY: `attribs` is a live array of `attribs.len()` `VAConfigAttrib`, the
        // profile and entrypoint are libva enum values, and `config` is a local the
        // call writes through. Nothing here outlives the call but `config`.
        let status = unsafe {
            (display.va.create_config)(
                display.display,
                VA_PROFILE_H264_HIGH,
                vah::VA_ENTRYPOINT_ENC_SLICE,
                attribs.as_ptr() as *mut c_void,
                attribs.len() as c_int,
                &mut config,
            )
        };
        display.va.check("vaCreateConfig", status)?;

        let slot_count = usize::from(params.slots.max(1));
        let surface_count = Self::SURFACES + slot_count + 1;
        let mut surfaces = vec![VA_INVALID_ID; surface_count];
        // SAFETY: `surfaces` is a live array of exactly `surface_count` ids the call
        // writes through; no surface attributes are passed, so the null is correct.
        let status = unsafe {
            (display.va.create_surfaces)(
                display.display,
                VA_RT_FORMAT_YUV420,
                coded_w as u32,
                coded_h as u32,
                surfaces.as_mut_ptr(),
                surface_count as u32,
                std::ptr::null_mut(),
                0,
            )
        };
        display.va.check("vaCreateSurfaces", status)?;

        let mut context = VA_INVALID_ID;
        // SAFETY: `config` and every id in `surfaces` were just created on this
        // display; `context` is a local written through.
        let status = unsafe {
            (display.va.create_context)(
                display.display,
                config,
                coded_w,
                coded_h,
                VA_PROGRESSIVE as c_int,
                surfaces.as_mut_ptr(),
                surface_count as c_int,
                &mut context,
            )
        };
        display.va.check("vaCreateContext", status)?;

        // A coded buffer must hold the largest picture the session can emit. An IDR
        // at a low QP is far bigger than the average rate suggests; the uncompressed
        // frame is the bound that cannot be exceeded.
        let coded_size = (coded_w as u32 * coded_h as u32 * 3 / 2).max(1 << 20);
        let mut coded_buf = VA_INVALID_ID;
        // SAFETY: `context` is live, the size is non-zero, one element, and no
        // initial data — which is what a coded (output) buffer takes.
        let status = unsafe {
            (display.va.create_buffer)(
                display.display,
                context,
                vah::VA_ENC_CODED_BUFFER_TYPE,
                coded_size,
                1,
                std::ptr::null_mut(),
                &mut coded_buf,
            )
        };
        display.va.check("vaCreateBuffer(coded)", status)?;

        let vpp = Vpp::new(&display, params.width, params.height)?;

        let recon = surfaces.split_off(Self::SURFACES);
        Ok(Self {
            display,
            config,
            context,
            input: surfaces,
            free: recon.clone(),
            recon,
            slots: vec![None; slot_count],
            wire: 0,
            coded_buf,
            vpp,
            staging: None,
            params,
            frame_num: 0,
            idr_pic_id: 0,
            next_surface: 0,
        })
    }

    /// The trusted references, as `(slot, wire)` — what `rfi::plan_slot_recovery`
    /// takes.
    pub fn slots(&self) -> Vec<(usize, i64)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.filter(|s| s.trusted).map(|s| (i, s.wire)))
            .collect()
    }

    /// Stop predicting from these slots: the plan's `tainted` mask.
    pub fn distrust(&mut self, mask: u32) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                if let Some(s) = slot {
                    s.trusted = false;
                }
            }
        }
    }

    /// Stop predicting from every slot; the next picture is an IDR.
    pub fn distrust_all(&mut self) {
        self.distrust(u32::MAX);
    }

    /// The surface a caller writes its picture into before [`Self::encode`].
    pub fn input_surface(&self) -> VaSurfaceId {
        self.input[self.next_surface]
    }

    /// Retarget in place: the next picture is rate-controlled to `bps`, with no IDR
    /// and no rebuild. This is the ABR step libav cannot take on this hardware.
    pub fn set_bitrate(&mut self, bps: u32) {
        self.params.bitrate_bps = bps;
    }

    /// What the next picture is rate-controlled to.
    pub fn bitrate_bps(&self) -> u32 {
        self.params.bitrate_bps
    }

    /// Fill the next input surface with NV12 from `y` and `uv`, for tests. Capture
    /// goes through [`Self::submit_packed`] or [`Self::submit_dmabuf`].
    pub fn write_nv12(&self, y: &[u8], uv: &[u8]) -> Result<()> {
        self.display.map_image(self.input_surface(), |image, ptr| {
            if image.num_planes < 2 {
                bail!("expected NV12 (2 planes), got {}", image.num_planes);
            }
            let rows = usize::from(image.height);
            let cols = usize::from(image.width);
            if y.len() < rows * cols || uv.len() < rows / 2 * cols {
                bail!("source planes are smaller than the surface");
            }
            for (plane, src, plane_rows) in [(0, y, rows), (1, uv, rows / 2)] {
                let pitch = image.pitches[plane] as usize;
                for row in 0..plane_rows {
                    // SAFETY: the mapped image is at least `data_size` bytes and
                    // the driver's own pitches and offsets bound every row written;
                    // the sources were length-checked above.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            src.as_ptr().add(row * cols),
                            ptr.add(image.offsets[plane] as usize + row * pitch),
                            cols,
                        );
                    }
                }
            }
            Ok(())
        })
    }

    /// Ingest packed 32-bit RGB from the CPU: uploaded to a staging surface of
    /// `fourcc`, converted on the GPU into the next input surface.
    pub fn submit_packed(&mut self, bytes: &[u8], fourcc: u32, row_bytes: usize) -> Result<()> {
        if rt_format_for(fourcc) != Some(VA_RT_FORMAT_RGB32) {
            bail!("no eight-bit RGB ingest for fourcc {fourcc:#x}");
        }
        let staging = match self.staging {
            Some((surface, f)) if f == fourcc => surface,
            _ => {
                if let Some((old, _)) = self.staging.take() {
                    self.display.destroy_surface(old);
                }
                let surface = self.display.create_surface(
                    VA_RT_FORMAT_RGB32,
                    Some(fourcc),
                    self.params.width,
                    self.params.height,
                )?;
                self.staging = Some((surface, fourcc));
                surface
            }
        };
        self.display.write_packed(staging, bytes, row_bytes)?;
        self.vpp
            .convert(&self.display, staging, true, false, self.input_surface())
    }

    /// Ingest a capture dmabuf: imported for this picture, converted into the next
    /// input surface, released. Eight-bit only until the HEVC session exists.
    pub fn submit_dmabuf(&mut self, source: &DmabufSource) -> Result<()> {
        let rt_format = pf_vaapi::vpp::import_format(source.drm_fourcc)
            .map(|(_, rt)| rt)
            .ok_or_else(|| anyhow!("no ingest for DRM fourcc {:#x}", source.drm_fourcc))?;
        if rt_format != VA_RT_FORMAT_RGB32 && rt_format != VA_RT_FORMAT_YUV420 {
            bail!("ten-bit ingest needs the HEVC session");
        }
        // Imported once per picture; a cache keyed on the fd would save the ioctl.
        let surface = self.display.import_dmabuf(source)?;
        let converted = self.vpp.convert(
            &self.display,
            surface,
            rt_format == VA_RT_FORMAT_RGB32,
            false,
            self.input_surface(),
        );
        self.display.destroy_surface(surface);
        converted
    }

    /// Encode the picture currently in [`Self::input_surface`].
    ///
    /// `force_idr` opens a GOP: SPS and PPS are packed ahead of the slice, so a
    /// client that joins here has parameter sets. Every other picture is a P
    /// predicted from the newest trusted slot — or an IDR when there is none.
    pub fn encode(&mut self, force_idr: bool) -> Result<EncodedPicture> {
        let newest = self
            .slots()
            .into_iter()
            .max_by_key(|&(_, wire)| wire)
            .map(|(slot, _)| slot);
        let reference = if force_idr { None } else { newest };
        self.encode_with(reference, false)
    }

    /// Encode the picture predicting from `slot` — the recovery anchor a loss plan
    /// picked. Refuses a slot that is empty or distrusted.
    pub fn encode_anchored(&mut self, slot: usize) -> Result<EncodedPicture> {
        match self.slots.get(slot) {
            Some(Some(s)) if s.trusted => self.encode_with(Some(slot), true),
            _ => bail!("slot {slot} holds no trusted reference"),
        }
    }

    fn encode_with(&mut self, reference: Option<usize>, anchor: bool) -> Result<EncodedPicture> {
        let is_idr = reference.is_none();
        let slot_count = self.slots.len();
        let slice = PictureSlice {
            is_idr,
            // An IDR restarts frame_num at 0, wherever the count stood.
            frame_num: if is_idr { 0 } else { self.frame_num },
            idr_pic_id: self.idr_pic_id ^ u16::from(is_idr),
            slot: if is_idr {
                0
            } else {
                (self.wire as usize % slot_count) as u8
            },
            max_slots: slot_count as u8,
            reference_slot: reference.map(|s| s as u8),
        };
        let surface = self.input[self.next_surface];
        let recon = self
            .free
            .pop()
            .ok_or_else(|| anyhow!("no free reconstruction surface"))?;
        let sps = self.params.sps();
        let pps = self.params.pps(std::rc::Rc::clone(&sps));

        // SAFETY: `context` and `surface` are live on this display; the call takes
        // both by value and starts a picture that `end_picture` below closes.
        let status =
            unsafe { (self.display.va.begin_picture)(self.display.display, self.context, surface) };
        self.display.va.check("vaBeginPicture", status)?;

        let mut owned: Vec<VaBufferId> = Vec::new();
        let result = self
            .render_picture(&mut owned, &sps, &pps, recon, slice)
            .and_then(|()| self.render_ids(&mut owned.clone()));
        if result.is_err() {
            self.free.push(recon);
        }

        // SAFETY: `context` is live; `end_picture` closes the picture `begin_picture`
        // opened, and must run even if a render failed or the driver keeps the
        // context open forever.
        let end = unsafe { (self.display.va.end_picture)(self.display.display, self.context) };
        for buf in owned {
            // SAFETY: every id here came from `vaCreateBuffer` on this display and
            // is destroyed exactly once; the driver has consumed them by end_picture.
            unsafe { (self.display.va.destroy_buffer)(self.display.display, buf) };
        }
        if let Err(e) = result.and_then(|()| self.display.va.check("vaEndPicture", end)) {
            if !self.free.contains(&recon) {
                self.free.push(recon);
            }
            return Err(e);
        }

        // SAFETY: `surface` is live; sync blocks until the encode that names it has
        // completed, which is what makes the coded buffer readable.
        let status = unsafe { (self.display.va.sync_surface)(self.display.display, surface) };
        let synced = self.display.va.check("vaSyncSurface", status);
        let bytes = synced.and_then(|()| self.read_coded());
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => {
                self.free.push(recon);
                return Err(e);
            }
        };

        // The picture is now the long-term reference in its slot. An IDR also
        // emptied the decoder's DPB, so every other slot goes with it.
        if is_idr {
            for slot in self.slots.iter_mut() {
                if let Some(old) = slot.take() {
                    self.free.push(old.surface);
                }
            }
        }
        let held = Slot {
            surface: recon,
            wire: self.wire,
            trusted: true,
        };
        if let Some(old) = self.slots[usize::from(slice.slot)].replace(held) {
            self.free.push(old.surface);
        }
        let wire = self.wire;
        self.wire += 1;
        self.next_surface = (self.next_surface + 1) % Self::SURFACES;
        let max_frame_num = 1u16 << (sps.log2_max_frame_num_minus4 + 4);
        self.frame_num = (slice.frame_num + 1) % max_frame_num;
        self.idr_pic_id = slice.idr_pic_id;
        Ok(EncodedPicture {
            bytes,
            is_idr,
            recovery_anchor: anchor,
            wire,
        })
    }

    /// Build and hand over every buffer this picture needs, in the order the
    /// drivers parse them: sequence, packed parameter sets, picture, slice, then
    /// the packed slice header — which Mesa reads against the sequence and picture
    /// state the earlier buffers set.
    fn render_picture(
        &self,
        owned: &mut Vec<VaBufferId>,
        sps: &cros_codecs::codec::h264::parser::Sps,
        pps: &cros_codecs::codec::h264::parser::Pps,
        recon: VaSurfaceId,
        slice: PictureSlice,
    ) -> Result<()> {
        let is_idr = slice.is_idr;
        let seq = self.params.va_sequence(sps);
        self.render(owned, vah::VA_ENC_SEQUENCE_PARAMETER_BUFFER_TYPE, &seq)?;
        let (rc, hrd, frame_rate) = self.params.rate_control();
        self.render_misc(owned, vah::VA_ENC_MISC_PARAMETER_TYPE_RATE_CONTROL, &rc)?;
        self.render_misc(owned, vah::VA_ENC_MISC_PARAMETER_TYPE_HRD, &hrd)?;
        self.render_misc(
            owned,
            vah::VA_ENC_MISC_PARAMETER_TYPE_FRAME_RATE,
            &frame_rate,
        )?;

        if is_idr {
            let (packed_sps, packed_pps) = pf_vaapi::enc_params::packed_parameter_sets(sps, pps);
            let mut sets = packed_sps;
            sets.extend_from_slice(&packed_pps);
            let bits = (sets.len() * 8) as u32;
            self.render_packed(owned, vah::VA_ENC_PACKED_HEADER_TYPE_SEQUENCE, &sets, bits)?;
        }

        let mut pic = vah::VaEncPictureParameterBufferH264 {
            coded_buf: self.coded_buf,
            frame_num: slice.frame_num,
            pic_init_qp: self.params.initial_qp,
            ..Default::default()
        };
        // For a long-term picture `frame_idx` is its `LongTermFrameIdx` — the slot.
        let long_term = |surface: VaSurfaceId, slot: u8| pf_vaapi::va::VaPictureH264 {
            picture_id: surface,
            frame_idx: u32::from(slot),
            flags: pf_vaapi::va::VA_PICTURE_H264_LONG_TERM_REFERENCE,
            top_field_order_cnt: 0,
            bottom_field_order_cnt: 0,
            va_reserved: [0; 4],
        };
        pic.curr_pic = long_term(recon, slice.slot);
        pic.pic_fields = va_pic_fields(pps, is_idr);
        // The whole DPB, trusted or not: the driver's eviction must match the
        // decoder's, and a distrusted slot is still a picture the decoder holds.
        let mut reference_list_entry = None;
        if !is_idr {
            let live = self
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.map(|s| (i as u8, s.surface)));
            for (n, (slot, surface)) in live.enumerate() {
                pic.reference_frames[n] = long_term(surface, slot);
                if Some(slot) == slice.reference_slot {
                    reference_list_entry = Some(pic.reference_frames[n]);
                }
            }
        }
        self.render(owned, vah::VA_ENC_PICTURE_PARAMETER_BUFFER_TYPE, &pic)?;

        let mut va_slice = vah::VaEncSliceParameterBufferH264 {
            num_macroblocks: self.params.mbs_per_picture(),
            slice_type: if is_idr { 2 } else { 0 },
            idr_pic_id: slice.idr_pic_id,
            num_ref_idx_active_override_flag: u8::from(!is_idr),
            ..Default::default()
        };
        if let Some(entry) = reference_list_entry {
            va_slice.ref_pic_list_0[0] = entry;
        } else if !is_idr {
            bail!("reference slot {:?} is not held", slice.reference_slot);
        }
        self.render(owned, vah::VA_ENC_SLICE_PARAMETER_BUFFER_TYPE, &va_slice)?;

        // Neither driver writes a slice header of its own: radeonsi templates its
        // from this one, iHD copies it.
        let (header, bits) = packed_slice_header(sps, pps, slice);
        self.render_packed(owned, vah::VA_ENC_PACKED_HEADER_TYPE_SLICE, &header, bits)
    }

    /// A misc buffer is a four-byte type tag with the payload inline after it, in
    /// one allocation.
    fn render_misc<T: Copy>(
        &self,
        owned: &mut Vec<VaBufferId>,
        kind: u32,
        value: &T,
    ) -> Result<()> {
        let mut bytes = Vec::with_capacity(4 + std::mem::size_of::<T>());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        // SAFETY: every `T` here is a `repr(C)` struct of `u32` fields with no
        // padding, so all `size_of::<T>()` bytes are initialised.
        bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
        });
        let buf = self.create(
            vah::VA_ENC_MISC_PARAMETER_BUFFER_TYPE,
            bytes.len() as u32,
            bytes.as_ptr().cast::<c_void>(),
        )?;
        owned.push(buf);
        Ok(())
    }

    /// One `vaCreateBuffer` + `vaRenderPicture` for a plain parameter struct.
    fn render<T>(&self, owned: &mut Vec<VaBufferId>, kind: u32, value: &T) -> Result<()> {
        let buf = self.create(
            kind,
            std::mem::size_of::<T>() as u32,
            (value as *const T).cast::<c_void>(),
        )?;
        owned.push(buf);
        Ok(())
    }

    /// A packed header is a descriptor and its bytes, and the two must reach the
    /// driver in **one** `vaRenderPicture` call. Rendered as separate calls they are
    /// silently dropped: the picture still encodes, the driver still reports success,
    /// and the header simply is not in the stream.
    ///
    /// `bit_length` is exact, not `bytes.len() * 8`: a slice header ends mid-byte
    /// and the driver writes slice data from the very next bit.
    fn render_packed(
        &self,
        owned: &mut Vec<VaBufferId>,
        kind: u32,
        bytes: &[u8],
        bit_length: u32,
    ) -> Result<()> {
        let desc = vah::VaEncPackedHeaderParameterBuffer {
            kind,
            bit_length,
            // Our synthesizer already inserted emulation prevention; asking the
            // driver to insert it again would corrupt every 00 00 0x sequence.
            has_emulation_bytes: 1,
            va_reserved: [0; 4],
        };
        let desc_buf = self.create(
            vah::VA_ENC_PACKED_HEADER_PARAMETER_BUFFER_TYPE,
            std::mem::size_of_val(&desc) as u32,
            (&raw const desc).cast::<c_void>(),
        )?;
        owned.push(desc_buf);
        let data_buf = self.create(
            vah::VA_ENC_PACKED_HEADER_DATA_BUFFER_TYPE,
            bytes.len() as u32,
            bytes.as_ptr().cast::<c_void>(),
        )?;
        owned.push(data_buf);
        Ok(())
    }

    /// One `vaCreateBuffer`, returning the id. The caller owns it.
    fn create(&self, kind: u32, size: u32, data: *const c_void) -> Result<VaBufferId> {
        let mut buf = VA_INVALID_ID;
        // SAFETY: `data` points at `size` bytes libva copies out before returning,
        // or is null for an output buffer; `buf` is a local written through.
        let status = unsafe {
            (self.display.va.create_buffer)(
                self.display.display,
                self.context,
                kind,
                size,
                1,
                data.cast_mut(),
                &mut buf,
            )
        };
        self.display
            .va
            .check("vaCreateBuffer", status)
            .with_context(|| format!("buffer type {kind}"))?;
        Ok(buf)
    }

    fn render_ids(&self, ids: &mut [VaBufferId]) -> Result<()> {
        // SAFETY: `ids` is a live array of exactly `ids.len()` buffer ids created on
        // this context; `vaRenderPicture` reads them and does not retain the array.
        let status = unsafe {
            (self.display.va.render_picture)(
                self.display.display,
                self.context,
                ids.as_mut_ptr(),
                ids.len() as c_int,
            )
        };
        self.display.va.check("vaRenderPicture", status)
    }

    /// Copy the picture out of the coded buffer.
    fn read_coded(&self) -> Result<Vec<u8>> {
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // SAFETY: `coded_buf` is live on this display and `ptr` is written through.
        let status =
            unsafe { (self.display.va.map_buffer)(self.display.display, self.coded_buf, &mut ptr) };
        self.display.va.check("vaMapBuffer(coded)", status)?;
        if ptr.is_null() {
            bail!("vaMapBuffer(coded) returned success with a null pointer");
        }

        let collected = (|| -> Result<Vec<u8>> {
            let mut out = Vec::new();
            let mut seg = ptr.cast::<vah::VaCodedBufferSegment>();
            while !seg.is_null() {
                // SAFETY: libva guarantees the mapped coded buffer begins with a
                // `VACodedBufferSegment` and that `next` is either null or another.
                let s = unsafe { *seg };
                if s.buf.is_null() {
                    bail!("coded segment has no data pointer");
                }
                if std::env::var_os("PF_ENC_DEBUG").is_some() {
                    eprintln!(
                        "  segment: size={} bit_offset={} status={:#x}",
                        s.size, s.bit_offset, s.status
                    );
                }
                // SAFETY: `s.buf` points at `s.size` bytes inside the mapping the
                // driver just filled, valid until `vaUnmapBuffer`.
                let bytes =
                    unsafe { std::slice::from_raw_parts(s.buf.cast::<u8>(), s.size as usize) };
                out.extend_from_slice(bytes);
                seg = s.next.cast::<vah::VaCodedBufferSegment>();
            }
            if out.is_empty() {
                bail!("the encoder produced no bytes");
            }
            Ok(out)
        })();

        // SAFETY: the buffer that was mapped, unmapped once, on every path.
        let unmap = unsafe { (self.display.va.unmap_buffer)(self.display.display, self.coded_buf) };
        let out = collected?;
        self.display.va.check("vaUnmapBuffer(coded)", unmap)?;
        Ok(out)
    }
}

impl Drop for H264Encoder {
    /// libva's teardown order: buffers, then context, then surfaces, then config.
    /// The display is dropped last, by its own `Drop`.
    fn drop(&mut self) {
        self.vpp.destroy(&self.display);
        if let Some((staging, _)) = self.staging.take() {
            self.display.destroy_surface(staging);
        }
        // SAFETY: every id was created on this display and is destroyed once. Drop
        // runs after the last encode, and `sync_surface` has completed each one.
        unsafe {
            (self.display.va.destroy_buffer)(self.display.display, self.coded_buf);
            (self.display.va.destroy_context)(self.display.display, self.context);
            (self.display.va.destroy_surfaces)(
                self.display.display,
                self.input.as_mut_ptr(),
                self.input.len() as c_int,
            );
            (self.display.va.destroy_surfaces)(
                self.display.display,
                self.recon.as_mut_ptr(),
                self.recon.len() as c_int,
            );
            (self.display.va.destroy_config)(self.display.display, self.config);
        }
    }
}

/// Open a display and an encoder on it, for callers with no display of their own.
pub fn open(params: SessionParams) -> Result<H264Encoder> {
    let va = crate::Libva::load().context("libva")?;
    let display = Display::open(va).context("no VAAPI display")?;
    H264Encoder::new(display, params).map_err(|e| anyhow!("{e:#}"))
}
