//! One description of the stream, two consumers.
//!
//! Both radeonsi and iHD report `VAConfigAttribEncPackedHeaders`, which means the
//! *app* writes the SPS, the PPS and every slice header — the driver will not. So
//! the same session facts have to reach two places: the packed-header bytes a
//! decoder reads, and [`VaEncSequenceParameterBufferH264`] which tells the driver
//! how to encode.
//!
//! Describing them separately is how they drift, and a drifted pair is a stream whose
//! header says one thing and whose macroblocks say another. [`SessionParams`] is the
//! single description; the SPS, the PPS and the VA buffer are all derived from it.

use std::rc::Rc;

use cros_codecs::codec::h264::nalu_writer::NaluWriter;
use cros_codecs::codec::h264::nalu_writer::NaluWriterResult;
use cros_codecs::codec::h264::parser::Level;
use cros_codecs::codec::h264::parser::NaluType;
use cros_codecs::codec::h264::parser::Pps;
use cros_codecs::codec::h264::parser::PpsBuilder;
use cros_codecs::codec::h264::parser::Profile;
use cros_codecs::codec::h264::parser::Sps;
use cros_codecs::codec::h264::parser::SpsBuilder;
use cros_codecs::codec::h264::synthesizer::Synthesizer;

use crate::enc_h264::VaEncMiscParameterFrameRate;
use crate::enc_h264::VaEncMiscParameterHrd;
use crate::enc_h264::VaEncMiscParameterRateControl;
use crate::enc_h264::VaEncSequenceParameterBufferH264;

/// A macroblock is 16×16; every VAAPI dimension is counted in them.
const MB: u32 = 16;

/// What a session is. Everything below is derived from this and nothing else.
#[derive(Clone, Copy, Debug)]
pub struct SessionParams {
    pub width: u32,
    pub height: u32,
    /// Frames per second, as a rational so 59.94 survives.
    pub fps_num: u32,
    pub fps_den: u32,
    pub bitrate_bps: u32,
    /// References the encoder may hold. AMD's `EncMaxRefFrames` is `l0=1`, so a
    /// session that wants an anchor plus a recent reference asks for two and gets
    /// one usable list entry; the anchor mechanism is built for that.
    pub max_num_ref_frames: u8,
    /// The reorder bound this stream actually uses. Zero for the low-delay P-only
    /// shape every punktfunk host emits.
    pub max_num_reorder_frames: u32,
    pub initial_qp: u8,
    /// VBV depth in frames of bits. One holds picture size roughly constant and
    /// takes motion as a QP dip; the libav path's `PUNKTFUNK_VBV_FRAMES` knob.
    pub vbv_frames: f32,
}

impl SessionParams {
    /// Width in macroblocks, rounded up — the coded size, before cropping.
    pub fn width_in_mbs(&self) -> u16 {
        self.width.div_ceil(MB) as u16
    }

    /// Height in macroblocks, rounded up.
    pub fn height_in_mbs(&self) -> u16 {
        self.height.div_ceil(MB) as u16
    }

    /// Macroblocks in a picture — the slice's `num_macroblocks` when it is the
    /// whole frame, which is the only shape WP2 emits.
    pub fn mbs_per_picture(&self) -> u32 {
        u32::from(self.width_in_mbs()) * u32::from(self.height_in_mbs())
    }

    /// The SPS this session codes against.
    ///
    /// `bitstream_restriction` is the point of packing our own: it states the
    /// reorder bound, so a client outputs on it instead of holding pictures until
    /// the DPB fills. AMF and Media Foundation have no API for this at all.
    pub fn sps(&self) -> Rc<Sps> {
        SpsBuilder::new()
            .seq_parameter_set_id(0)
            .profile_idc(Profile::High)
            .level_idc(Level::L4_1)
            .chroma_format_idc(1)
            .bit_depth_luma(8)
            .bit_depth_chroma(8)
            .max_num_ref_frames(self.max_num_ref_frames)
            .frame_mbs_only_flag(true)
            .direct_8x8_inference_flag(true)
            // Type 2 is display order == decode order, which is what a P-only
            // low-delay stream is. It also costs no per-slice POC syntax.
            .pic_order_cnt_type(2)
            .log2_max_frame_num_minus4(4)
            // Takes the VISIBLE size and derives the crop itself, in the right units —
            // `frame_crop_offsets` is (top, bottom, left, right) and easy to transpose.
            // Must follow `chroma_format_idc` and `frame_mbs_only_flag`, which set the
            // crop unit it divides by.
            .resolution(self.width, self.height)
            .timing_info(self.fps_den, self.fps_num * 2, true)
            .bitstream_restriction(self.max_num_reorder_frames)
            .build()
    }

    /// The PPS.
    ///
    /// CAVLC, deliberately: `PpsBuilder` cannot set `entropy_coding_mode_flag`, and
    /// the driver encodes according to the VA `pic_fields` bit — so turning CABAC on
    /// means setting it in two places that must agree. It is a bitrate win worth
    /// having, but as a paired change with its own test, not as a default nobody
    /// checked.
    pub fn pps(&self, sps: Rc<Sps>) -> Rc<Pps> {
        PpsBuilder::new(sps)
            .pic_parameter_set_id(0)
            .pic_init_qp(self.initial_qp)
            .deblocking_filter_control_present_flag(true)
            .build()
    }

    /// The rate-control facts, as the three misc buffers the drivers read them from.
    ///
    /// Sent with every picture: both drivers compare against the last and re-plan
    /// on a change, which is what makes a mid-stream `bitrate_bps` step land with
    /// no IDR. Mesa takes the target from here and nowhere else — the sequence
    /// buffer's `bits_per_second` is never read.
    pub fn rate_control(
        &self,
    ) -> (
        VaEncMiscParameterRateControl,
        VaEncMiscParameterHrd,
        VaEncMiscParameterFrameRate,
    ) {
        let fps = self.fps_num as f64 / self.fps_den.max(1) as f64;
        let vbv_bits = (self.bitrate_bps as f64 / fps.max(1.0) * f64::from(self.vbv_frames))
            .clamp(1.0, u32::MAX as f64) as u32;
        let rc = VaEncMiscParameterRateControl {
            bits_per_second: self.bitrate_bps,
            target_percentage: 100,
            window_size: (1000.0 * f64::from(self.vbv_frames) / fps.max(1.0)).ceil() as u32,
            initial_qp: u32::from(self.initial_qp),
            // disable_frame_skip:1 << 1 | disable_bit_stuffing:1 << 2. A skipped
            // frame is a stall on the client; filler is bits the wire pays for.
            rc_flags: 1 << 1 | 1 << 2,
            ..Default::default()
        };
        let hrd = VaEncMiscParameterHrd {
            buffer_size: vbv_bits,
            initial_buffer_fullness: vbv_bits / 4 * 3,
        };
        let frame_rate = VaEncMiscParameterFrameRate {
            framerate: self.fps_num & 0xffff | (self.fps_den & 0xffff) << 16,
            framerate_flags: 0,
        };
        (rc, hrd, frame_rate)
    }

    /// The same facts in the shape the driver reads.
    ///
    /// Derived from the SPS rather than from `self` a second time: a field that
    /// disagrees with the packed header is a stream whose header and macroblocks
    /// describe different pictures.
    pub fn va_sequence(&self, sps: &Sps) -> VaEncSequenceParameterBufferH264 {
        let mut seq = VaEncSequenceParameterBufferH264 {
            seq_parameter_set_id: sps.seq_parameter_set_id,
            level_idc: sps.level_idc as u8,
            // Infinite GOP: IDRs come from `request_keyframe`, never from a period.
            intra_period: 0,
            intra_idr_period: 0,
            ip_period: 1,
            bits_per_second: self.bitrate_bps,
            max_num_ref_frames: u32::from(sps.max_num_ref_frames),
            picture_width_in_mbs: self.width_in_mbs(),
            picture_height_in_mbs: self.height_in_mbs(),
            bit_depth_luma_minus8: sps.bit_depth_luma_minus8,
            bit_depth_chroma_minus8: sps.bit_depth_chroma_minus8,
            num_units_in_tick: sps.vui_parameters.num_units_in_tick,
            time_scale: sps.vui_parameters.time_scale,
            ..Default::default()
        };
        // seq_fields, low bit first: chroma_format_idc:2, frame_mbs_only:1,
        // mb_adaptive:1, seq_scaling:1, direct_8x8:1, log2_max_frame_num_minus4:4,
        // pic_order_cnt_type:2, log2_max_poc_lsb_minus4:4, delta_poc_always_zero:1.
        seq.seq_fields = u32::from(sps.chroma_format_idc) & 0x3
            | u32::from(sps.frame_mbs_only_flag) << 2
            | u32::from(sps.mb_adaptive_frame_field_flag) << 3
            | u32::from(sps.seq_scaling_matrix_present_flag) << 4
            | u32::from(sps.direct_8x8_inference_flag) << 5
            | (u32::from(sps.log2_max_frame_num_minus4) & 0xf) << 6
            | (u32::from(sps.pic_order_cnt_type) & 0x3) << 10
            | (u32::from(sps.log2_max_pic_order_cnt_lsb_minus4) & 0xf) << 12
            | u32::from(sps.delta_pic_order_always_zero_flag) << 16;
        // vui_fields: aspect_ratio_info:1, timing_info:1, bitstream_restriction:1,
        // log2_max_mv_len_horizontal:5, log2_max_mv_len_vertical:5,
        // fixed_frame_rate:1, low_delay_hrd:1, mvs_over_pic_boundaries:1.
        let vui = &sps.vui_parameters;
        seq.vui_parameters_present_flag = u8::from(sps.vui_parameters_present_flag);
        seq.vui_fields = u32::from(vui.aspect_ratio_info_present_flag)
            | u32::from(vui.timing_info_present_flag) << 1
            | u32::from(vui.bitstream_restriction_flag) << 2
            | (vui.log2_max_mv_length_horizontal & 0x1f) << 3
            | (vui.log2_max_mv_length_vertical & 0x1f) << 8
            | u32::from(vui.fixed_frame_rate_flag) << 13
            | u32::from(vui.low_delay_hrd_flag) << 14
            | u32::from(vui.motion_vectors_over_pic_boundaries_flag) << 15;
        if sps.frame_cropping_flag {
            seq.frame_cropping_flag = 1;
            seq.frame_crop_left_offset = sps.frame_crop_left_offset;
            seq.frame_crop_right_offset = sps.frame_crop_right_offset;
            seq.frame_crop_top_offset = sps.frame_crop_top_offset;
            seq.frame_crop_bottom_offset = sps.frame_crop_bottom_offset;
        }
        seq
    }
}

/// `VAEncPictureParameterBufferH264::pic_fields`, from the PPS the stream carries.
///
/// radeonsi regenerates the PPS from these bits, not from the packed one, so a bit
/// that disagrees with the PPS is a stream whose PPS disagrees with the host's.
/// Every picture here is a reference: the next P has only this one to point at.
pub fn va_pic_fields(pps: &Pps, is_idr: bool) -> u32 {
    // idr_pic_flag:1 | reference_pic_flag:2 | entropy_coding_mode:1 |
    // weighted_pred:1 | weighted_bipred_idc:2 | constrained_intra_pred:1 |
    // transform_8x8_mode:1 | deblocking_filter_control_present:1 |
    // redundant_pic_cnt_present:1 | pic_order_present:1 | pic_scaling_matrix_present:1
    u32::from(is_idr)
        | 1 << 1
        | u32::from(pps.entropy_coding_mode_flag) << 3
        | u32::from(pps.weighted_pred_flag) << 4
        | (u32::from(pps.weighted_bipred_idc) & 0x3) << 5
        | u32::from(pps.constrained_intra_pred_flag) << 7
        | u32::from(pps.transform_8x8_mode_flag) << 8
        | u32::from(pps.deblocking_filter_control_present_flag) << 9
        | u32::from(pps.redundant_pic_cnt_present_flag) << 10
        | u32::from(pps.bottom_field_pic_order_in_frame_present_flag) << 11
        | u32::from(pps.pic_scaling_matrix_present_flag) << 12
}

/// The two parameter sets a decoder needs before any slice, each as its own
/// annex-B NALU with emulation prevention.
///
/// Returned separately because VAAPI describes them separately: the SPS is the
/// *sequence* packed header and the PPS is the *picture* one, and a single buffer
/// holding both is not a shape the descriptor can name.
///
/// Written on every IDR, not once at open: a client that joins mid-stream, or one
/// that reconnects after a loss, has no earlier bytes to have read.
pub fn packed_parameter_sets(sps: &Sps, pps: &Pps) -> (Vec<u8>, Vec<u8>) {
    let mut packed_sps = Vec::new();
    let mut packed_pps = Vec::new();
    // nal_ref_idc 3: parameter sets are never discardable.
    Synthesizer::<'_, Sps, _>::synthesize(3, sps, &mut packed_sps, true)
        .expect("writing to a Vec cannot fail, and the SPS is ours");
    Synthesizer::<'_, Pps, _>::synthesize(3, pps, &mut packed_pps, true)
        .expect("writing to a Vec cannot fail, and the PPS is ours");
    (packed_sps, packed_pps)
}

/// What one picture's slice header says that the parameter sets do not.
#[derive(Clone, Copy, Debug)]
pub struct PictureSlice {
    pub is_idr: bool,
    /// 0 on an IDR, counting up from 1 after it, modulo `MaxFrameNum`.
    pub frame_num: u16,
    /// Consecutive IDRs must differ here, or a decoder takes the second for a
    /// repeat of the first.
    pub idr_pic_id: u16,
}

/// The slice header as a packed header: its bytes, and how many of their bits
/// are header.
///
/// Both drivers want it, differently. radeonsi parses it for the NAL header and
/// the marking syntax, then templates its own; iHD copies exactly `bits` and
/// writes slice data from the next bit. So the count excludes the stop bit and
/// alignment that byte-complete the buffer — a byte length × 8 is off by up to
/// seven bits and the slice data lands mid-header.
///
/// One slice per picture, every picture a reference, the reference list in its
/// default order (newest first).
pub fn packed_slice_header(sps: &Sps, pps: &Pps, slice: PictureSlice) -> (Vec<u8>, u32) {
    // Type 2 is the SPS's choice precisely so there is no per-slice POC syntax.
    debug_assert_eq!(sps.pic_order_cnt_type, 2);
    let mut buf = Vec::new();
    let write = |w: &mut NaluWriter<&mut Vec<u8>>| -> NaluWriterResult<()> {
        if slice.is_idr {
            w.write_header(3, NaluType::SliceIdr as u8)?;
        } else {
            w.write_header(2, NaluType::Slice as u8)?;
        }
        w.write_ue(0u32)?; // first_mb_in_slice
        w.write_ue(if slice.is_idr { 7u32 } else { 5 })?; // slice_type: 7 = I, 5 = P
        w.write_ue(u32::from(pps.pic_parameter_set_id))?;
        w.write_f(
            usize::from(sps.log2_max_frame_num_minus4) + 4,
            u32::from(slice.frame_num),
        )?;
        if slice.is_idr {
            w.write_ue(u32::from(slice.idr_pic_id))?;
        } else {
            w.write_f(1, 0u32)?; // num_ref_idx_active_override_flag: the PPS default of one
            w.write_f(1, 0u32)?; // ref_pic_list_modification_flag_l0
        }
        // dec_ref_pic_marking: sliding window, nothing long-term.
        if slice.is_idr {
            w.write_f(1, 0u32)?; // no_output_of_prior_pics_flag
            w.write_f(1, 0u32)?; // long_term_reference_flag
        } else {
            w.write_f(1, 0u32)?; // adaptive_ref_pic_marking_mode_flag
        }
        if pps.entropy_coding_mode_flag && !slice.is_idr {
            w.write_ue(0u32)?; // cabac_init_idc
        }
        w.write_se(0i32)?; // slice_qp_delta
        if pps.deblocking_filter_control_present_flag {
            w.write_ue(0u32)?; // disable_deblocking_filter_idc
            w.write_se(0i32)?; // slice_alpha_c0_offset_div2
            w.write_se(0i32)?; // slice_beta_offset_div2
        }
        // Not header: the stop bit makes the header's last bit findable from the
        // bytes alone, and the alignment lets the writer flush.
        w.write_f(1, 1u32)?;
        while !w.aligned() {
            w.write_f(1, 0u32)?;
        }
        Ok(())
    };
    write(&mut NaluWriter::new(&mut buf, true))
        .expect("writing to a Vec cannot fail, and every value fits its field");
    let trailing = buf.last().map_or(0, |b| b.trailing_zeros() + 1);
    let bits = buf.len() as u32 * 8 - trailing;
    (buf, bits)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use cros_codecs::codec::h264::parser::Nalu;
    use cros_codecs::codec::h264::parser::Parser;
    use cros_codecs::codec::h264::parser::SliceHeader;
    use cros_codecs::codec::h264::parser::SliceType;

    use super::*;

    fn params() -> SessionParams {
        SessionParams {
            width: 1920,
            height: 1080,
            fps_num: 60,
            fps_den: 1,
            bitrate_bps: 20_000_000,
            max_num_ref_frames: 2,
            max_num_reorder_frames: 0,
            initial_qp: 26,
            vbv_frames: 1.0,
        }
    }

    /// A one-frame VBV at 60 fps is a sixtieth of the rate, and the frame rate
    /// travels as `num | den << 16` — a bare integer means 59.94 arrives as 59.
    #[test]
    fn rate_control_sizes_the_vbv_in_frames() {
        let p = SessionParams {
            bitrate_bps: 6_000_000,
            fps_num: 60000,
            fps_den: 1001,
            ..params()
        };
        let (rc, hrd, fr) = p.rate_control();
        assert_eq!(rc.bits_per_second, 6_000_000);
        assert_eq!(rc.target_percentage, 100, "CBR");
        assert_eq!(rc.window_size, 17, "one frame at 59.94, in whole ms");
        assert_eq!(rc.rc_flags & 0b110, 0b110, "no frame skip, no filler");
        assert_eq!(hrd.buffer_size, 100_100, "6 Mbps / 59.94");
        assert!(hrd.initial_buffer_fullness < hrd.buffer_size);
        assert_eq!(fr.framerate & 0xffff, 60000);
        assert_eq!(fr.framerate >> 16, 1001);
    }

    /// 1080 is not a multiple of 16, so the coded height is 1088 and the difference
    /// rides as a crop. Encoding 1080 rows instead would be a different picture.
    #[test]
    fn a_height_off_the_macroblock_grid_is_cropped_not_shrunk() {
        let p = params();
        assert_eq!(p.width_in_mbs(), 120);
        assert_eq!(p.height_in_mbs(), 68);
        assert_eq!(p.mbs_per_picture(), 120 * 68);

        // (1088 - 1080) / 2 chroma units, on the bottom edge only.
        let sps = p.sps();
        assert!(sps.frame_cropping_flag);
        assert_eq!(sps.frame_crop_bottom_offset, 4);
        assert_eq!(sps.frame_crop_right_offset, 0);
        assert_eq!(sps.frame_crop_top_offset, 0);
        assert_eq!(sps.frame_crop_left_offset, 0);
    }

    /// The reorder bound is the whole reason we pack our own SPS. Round-trip it
    /// through the parser the client actually uses, not through our own opinion.
    #[test]
    fn an_authored_sps_states_the_reorder_bound() {
        let p = params();
        let sps = p.sps();
        assert!(sps.vui_parameters.bitstream_restriction_flag);
        assert_eq!(sps.vui_parameters.max_num_reorder_frames, 0);
        // E.2.1: never below max_num_ref_frames, or a conforming decoder livelocks
        // waiting for a DPB that can never drain (the sweep's S-77).
        assert!(
            sps.vui_parameters.max_dec_frame_buffering >= u32::from(sps.max_num_ref_frames),
            "max_dec_frame_buffering must not sit below max_num_ref_frames"
        );
    }

    /// The two descriptions must agree. A VA buffer that disagrees with the packed
    /// header is a stream whose header and macroblocks describe different pictures.
    #[test]
    fn the_va_sequence_buffer_agrees_with_the_packed_sps() {
        let p = params();
        let sps = p.sps();
        let seq = p.va_sequence(&sps);

        assert_eq!(seq.picture_width_in_mbs, 120);
        assert_eq!(seq.picture_height_in_mbs, 68);
        assert_eq!(seq.bits_per_second, 20_000_000);
        assert_eq!(seq.max_num_ref_frames, u32::from(sps.max_num_ref_frames));

        // chroma_format_idc 1 in the low two bits, frame_mbs_only above it.
        assert_eq!(seq.seq_fields & 0x3, 1);
        assert_eq!((seq.seq_fields >> 2) & 1, 1);
        assert_eq!((seq.seq_fields >> 5) & 1, 1, "direct_8x8_inference");
        assert_eq!((seq.seq_fields >> 10) & 0x3, 2, "pic_order_cnt_type 2");

        // The bit the whole exercise is for.
        assert_eq!((seq.vui_fields >> 2) & 1, 1, "bitstream_restriction");
        assert_eq!(seq.vui_parameters_present_flag, 1);

        // Cropping is carried, not silently dropped.
        assert_eq!(seq.frame_cropping_flag, 1);
        assert_eq!(seq.frame_crop_bottom_offset, 4);
    }

    /// Parse a slice header the way the client does, with our own parameter sets
    /// active.
    fn parse_slice(p: &SessionParams, bytes: &[u8]) -> SliceHeader {
        let sps = p.sps();
        let pps = p.pps(Rc::clone(&sps));
        let (packed_sps, packed_pps) = packed_parameter_sets(&sps, &pps);
        let mut parser = Parser::default();
        let nalu = Nalu::next(&mut Cursor::new(&packed_sps[..])).unwrap();
        parser.parse_sps(&nalu).unwrap();
        let nalu = Nalu::next(&mut Cursor::new(&packed_pps[..])).unwrap();
        parser.parse_pps(&nalu).unwrap();
        let nalu = Nalu::next(&mut Cursor::new(bytes)).unwrap();
        parser.parse_slice_header(nalu).unwrap().header
    }

    /// The parser counts the header's bits from the NAL header byte, with
    /// emulation prevention taken back out; the driver's count starts at the
    /// start code and keeps it in.
    fn driver_bits(header: &SliceHeader) -> u32 {
        (32 + header.header_bit_size + 8 * header.n_emulation_prevention_bytes) as u32
    }

    /// The bit count is the contract: iHD writes slice data from that bit. The
    /// parser that reads the header back is the independent measure of where it
    /// ends, and a P header is deliberately not byte-aligned so a length × 8
    /// cannot pass by luck.
    #[test]
    fn the_packed_slice_header_reports_its_exact_bit_length() {
        let p = params();
        let sps = p.sps();
        let pps = p.pps(Rc::clone(&sps));

        let idr = PictureSlice {
            is_idr: true,
            frame_num: 0,
            idr_pic_id: 1,
        };
        let (bytes, bits) = packed_slice_header(&sps, &pps, idr);
        assert_eq!(bytes[4] & 0x1f, 5, "an IDR slice NALU");
        let header = parse_slice(&p, &bytes);
        assert_eq!(header.slice_type, SliceType::I);
        assert_eq!(header.frame_num, 0);
        assert_eq!(header.idr_pic_id, 1);
        assert_eq!(bits, driver_bits(&header));

        let p_slice = PictureSlice {
            is_idr: false,
            frame_num: 7,
            idr_pic_id: 1,
        };
        let (bytes, bits) = packed_slice_header(&sps, &pps, p_slice);
        assert_eq!(bytes[4] & 0x1f, 1, "a non-IDR slice NALU");
        assert_ne!(
            bytes[4] >> 5,
            0,
            "a reference picture has a non-zero nal_ref_idc"
        );
        let header = parse_slice(&p, &bytes);
        assert_eq!(header.slice_type, SliceType::P);
        assert_eq!(header.frame_num, 7);
        assert!(!header.num_ref_idx_active_override_flag);
        assert!(
            !header
                .dec_ref_pic_marking
                .adaptive_ref_pic_marking_mode_flag
        );
        assert_eq!(bits, driver_bits(&header));
        assert_ne!(bits % 8, 0, "this header is not byte-aligned, by design");
    }

    /// The PPS says deblocking control is present and the driver must be told the
    /// same, or radeonsi regenerates a PPS that disagrees with the one we authored.
    #[test]
    fn the_va_picture_fields_agree_with_the_pps() {
        let p = params();
        let pps = p.pps(p.sps());
        let fields = va_pic_fields(&pps, true);
        assert_eq!(fields & 1, 1, "idr_pic_flag");
        assert_eq!((fields >> 1) & 0x3, 1, "reference_pic_flag");
        assert_eq!((fields >> 3) & 1, 0, "CAVLC");
        assert_eq!((fields >> 9) & 1, 1, "deblocking_filter_control_present");
        assert_eq!(va_pic_fields(&pps, false) & 1, 0);
    }

    /// The bytes a joining client reads first. Two NALUs, SPS then PPS, each with a
    /// start code — anything else and the decoder has no parameter sets.
    #[test]
    fn the_parameter_set_au_is_an_sps_then_a_pps() {
        let p = params();
        let sps = p.sps();
        let pps = p.pps(Rc::clone(&sps));
        let (packed_sps, packed_pps) = packed_parameter_sets(&sps, &pps);

        // nal_unit_type is the low 5 bits of the byte after the start code.
        for (bytes, want) in [(&packed_sps, 7u8), (&packed_pps, 8u8)] {
            let start = (0..bytes.len().saturating_sub(3))
                .find(|&i| bytes[i..i + 4] == [0, 0, 0, 1])
                .expect("each set carries its own start code");
            assert_eq!(bytes[start + 4] & 0x1f, want);
        }
    }
}
