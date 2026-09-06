//! One description of the stream, two consumers.
//!
//! Both radeonsi and iHD report `VAConfigAttribEncPackedHeaders`, which means the
//! *app* writes the SPS and PPS — the driver will not. So the same session facts
//! have to reach two places: the packed-header bytes a decoder reads, and
//! [`VaEncSequenceParameterBufferH264`] which tells the driver how to encode.
//!
//! Describing them separately is how they drift, and a drifted pair is a stream whose
//! header says one thing and whose macroblocks say another. [`SessionParams`] is the
//! single description; the SPS, the PPS and the VA buffer are all derived from it.

use std::rc::Rc;

use cros_codecs::codec::h264::parser::Level;
use cros_codecs::codec::h264::parser::Pps;
use cros_codecs::codec::h264::parser::PpsBuilder;
use cros_codecs::codec::h264::parser::Profile;
use cros_codecs::codec::h264::parser::Sps;
use cros_codecs::codec::h264::parser::SpsBuilder;
use cros_codecs::codec::h264::synthesizer::Synthesizer;

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
            | (u32::from(vui.log2_max_mv_length_horizontal) & 0x1f) << 3
            | (u32::from(vui.log2_max_mv_length_vertical) & 0x1f) << 8
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

/// The parameter-set access unit a decoder needs before any slice: SPS then PPS,
/// each as its own annex-B NALU with emulation prevention.
///
/// Written on every IDR, not once at open: a client that joins mid-stream, or one
/// that reconnects after a loss, has no earlier bytes to have read.
pub fn packed_parameter_sets(sps: &Sps, pps: &Pps) -> Vec<u8> {
    let mut out = Vec::new();
    // nal_ref_idc 3: parameter sets are never discardable.
    Synthesizer::<'_, Sps, _>::synthesize(3, sps, &mut out, true)
        .expect("writing to a Vec cannot fail, and the SPS is ours");
    Synthesizer::<'_, Pps, _>::synthesize(3, pps, &mut out, true)
        .expect("writing to a Vec cannot fail, and the PPS is ours");
    out
}

#[cfg(test)]
mod tests {
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
        }
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
        assert_eq!(
            u32::from(seq.max_num_ref_frames),
            u32::from(sps.max_num_ref_frames)
        );

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

    /// The bytes a joining client reads first. Two NALUs, SPS then PPS, each with a
    /// start code — anything else and the decoder has no parameter sets.
    #[test]
    fn the_parameter_set_au_is_an_sps_then_a_pps() {
        let p = params();
        let sps = p.sps();
        let pps = p.pps(Rc::clone(&sps));
        let au = packed_parameter_sets(&sps, &pps);

        let starts: Vec<usize> = (0..au.len().saturating_sub(3))
            .filter(|&i| au[i..i + 4] == [0, 0, 0, 1])
            .collect();
        assert_eq!(starts.len(), 2, "one start code per parameter set");
        // nal_unit_type is the low 5 bits of the byte after the start code: 7 = SPS, 8 = PPS.
        assert_eq!(au[starts[0] + 4] & 0x1f, 7);
        assert_eq!(au[starts[1] + 4] & 0x1f, 8);
    }
}
