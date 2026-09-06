//! VideoProc, hand-declared: the ingest colour conversion and the dmabuf import
//! attributes.
//!
//! Capture hands the host packed RGB — a dmabuf or CPU bytes — and the encoder
//! takes NV12 (P010 at ten bits). Both radeonsi and iHD expose
//! `VAEntrypointVideoProc`, so the conversion is one pipeline buffer on the GPU and
//! `swscale` has no job here. Same rules as [`crate::enc_h264`]: `#[repr(C)]`
//! against `va_vpp.h`, every layout measured by `layout-probe.c` on the target and
//! pinned by a `const _` assert, and no libva.

use std::ffi::c_void;
use std::mem::offset_of;
use std::mem::size_of;

pub const VA_PROFILE_NONE: i32 = -1;
pub const VA_ENTRYPOINT_VIDEO_PROC: i32 = 10;
pub const VA_PROC_PIPELINE_PARAMETER_BUFFER_TYPE: u32 = 41;

/// `VAConfigAttribRTFormat` bits: what a surface pool holds.
pub const VA_RT_FORMAT_YUV420: u32 = 0x0000_0001;
pub const VA_RT_FORMAT_YUV420_10: u32 = 0x0000_0100;
pub const VA_RT_FORMAT_RGB32: u32 = 0x0002_0000;
pub const VA_RT_FORMAT_RGB32_10: u32 = 0x0020_0000;

/// The two surface attributes a dmabuf import passes to `vaCreateSurfaces`.
pub const VA_SURFACE_ATTRIB_MEMORY_TYPE: i32 = 6;
pub const VA_SURFACE_ATTRIB_EXTERNAL_BUFFER_DESCRIPTOR: i32 = 7;
pub const VA_GENERIC_VALUE_TYPE_POINTER: i32 = 3;
pub const VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2: u32 = 0x4000_0000;

/// `VAProcColorStandardType`.
pub const VA_PROC_COLOR_STANDARD_BT709: u32 = 2;
pub const VA_PROC_COLOR_STANDARD_SRGB: u32 = 8;
pub const VA_PROC_COLOR_STANDARD_BT2020: u32 = 12;
/// `VAProcColorProperties::color_range`.
pub const VA_SOURCE_RANGE_REDUCED: u8 = 1;
pub const VA_SOURCE_RANGE_FULL: u8 = 2;

/// libva fourccs name byte order; DRM fourccs name the bit layout of a
/// little-endian word. `XR24` (`DRM_FORMAT_XRGB8888`) is B, G, R, X in memory,
/// which libva calls `BGRX`. The ten-bit and planar codes coincide.
pub const VA_FOURCC_BGRX: u32 = 0x5852_4742;
pub const VA_FOURCC_RGBX: u32 = 0x5842_4752;
pub const VA_FOURCC_BGRA: u32 = 0x4152_4742;
pub const VA_FOURCC_RGBA: u32 = 0x4142_4752;
pub const VA_FOURCC_X2R10G10B10: u32 = 0x3033_5258;
pub const VA_FOURCC_X2B10G10R10: u32 = 0x3033_4258;
pub const VA_FOURCC_A2R10G10B10: u32 = 0x3033_5241;
pub const VA_FOURCC_A2B10G10R10: u32 = 0x3033_4241;

const fn drm(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}
pub const DRM_FORMAT_XRGB8888: u32 = drm(b"XR24");
pub const DRM_FORMAT_ARGB8888: u32 = drm(b"AR24");
pub const DRM_FORMAT_XBGR8888: u32 = drm(b"XB24");
pub const DRM_FORMAT_ABGR8888: u32 = drm(b"AB24");
pub const DRM_FORMAT_XRGB2101010: u32 = drm(b"XR30");
pub const DRM_FORMAT_XBGR2101010: u32 = drm(b"XB30");
pub const DRM_FORMAT_ARGB2101010: u32 = drm(b"AR30");
pub const DRM_FORMAT_ABGR2101010: u32 = drm(b"AB30");
pub const DRM_FORMAT_NV12: u32 = drm(b"NV12");
pub const DRM_FORMAT_P010: u32 = drm(b"P010");

/// What to import a dmabuf as: the libva fourcc, and the render-target format its
/// surface is created with. `None` is a format no ingest takes.
pub fn import_format(drm_fourcc: u32) -> Option<(u32, u32)> {
    let va = match drm_fourcc {
        DRM_FORMAT_XRGB8888 => VA_FOURCC_BGRX,
        DRM_FORMAT_ARGB8888 => VA_FOURCC_BGRA,
        DRM_FORMAT_XBGR8888 => VA_FOURCC_RGBX,
        DRM_FORMAT_ABGR8888 => VA_FOURCC_RGBA,
        DRM_FORMAT_XRGB2101010 => VA_FOURCC_X2R10G10B10,
        DRM_FORMAT_XBGR2101010 => VA_FOURCC_X2B10G10R10,
        DRM_FORMAT_ARGB2101010 => VA_FOURCC_A2R10G10B10,
        DRM_FORMAT_ABGR2101010 => VA_FOURCC_A2B10G10R10,
        DRM_FORMAT_NV12 => crate::drm::VA_FOURCC_NV12,
        DRM_FORMAT_P010 => crate::drm::VA_FOURCC_P010,
        _ => return None,
    };
    Some((va, rt_format_for(va)?))
}

/// The render-target format a surface of `va_fourcc` lives in.
pub fn rt_format_for(va_fourcc: u32) -> Option<u32> {
    Some(match va_fourcc {
        VA_FOURCC_BGRX | VA_FOURCC_BGRA | VA_FOURCC_RGBX | VA_FOURCC_RGBA => VA_RT_FORMAT_RGB32,
        VA_FOURCC_X2R10G10B10
        | VA_FOURCC_X2B10G10R10
        | VA_FOURCC_A2R10G10B10
        | VA_FOURCC_A2B10G10R10 => VA_RT_FORMAT_RGB32_10,
        crate::drm::VA_FOURCC_NV12 => VA_RT_FORMAT_YUV420,
        crate::drm::VA_FOURCC_P010 => VA_RT_FORMAT_YUV420_10,
        _ => return None,
    })
}

/// `VARectangle`: `short` origin, `unsigned short` size.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VaRectangle {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

/// `VAProcColorProperties`: five bytes of H.273 facts and three reserved.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VaProcColorProperties {
    pub chroma_sample_location: u8,
    pub color_range: u8,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub reserved: [u8; 3],
}

/// `VAProcPipelineParameterBuffer`: one source surface into the picture
/// `vaBeginPicture` named. Pointers are null here — no regions, no filters, no
/// reference surfaces — which libva reads as "the whole picture, as is".
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VaProcPipelineParameterBuffer {
    pub surface: u32,
    pub surface_region: *const VaRectangle,
    pub surface_color_standard: u32,
    pub output_region: *const VaRectangle,
    pub output_background_color: u32,
    pub output_color_standard: u32,
    pub pipeline_flags: u32,
    pub filter_flags: u32,
    pub filters: *mut u32,
    pub num_filters: u32,
    pub forward_references: *mut u32,
    pub num_forward_references: u32,
    pub backward_references: *mut u32,
    pub num_backward_references: u32,
    pub rotation_state: u32,
    pub blend_state: *const c_void,
    pub mirror_state: u32,
    pub additional_outputs: *mut u32,
    pub num_additional_outputs: u32,
    pub input_surface_flag: u32,
    pub output_surface_flag: u32,
    pub input_color_properties: VaProcColorProperties,
    pub output_color_properties: VaProcColorProperties,
    pub processing_mode: u32,
    pub output_hdr_metadata: *const c_void,
    pub va_reserved: [u32; 16],
}

impl VaProcPipelineParameterBuffer {
    /// Whole-picture conversion of `source`, with the colour facts `swscale` used to
    /// be told: RGB is full-range sRGB in and limited-range BT.709 out (BT.2020 at
    /// ten bits); NV12 in is already that, and is copied.
    pub fn convert(source: u32, source_is_rgb: bool, ten_bit: bool) -> Self {
        let out_standard = if ten_bit {
            VA_PROC_COLOR_STANDARD_BT2020
        } else {
            VA_PROC_COLOR_STANDARD_BT709
        };
        let (in_standard, in_range) = if source_is_rgb {
            (VA_PROC_COLOR_STANDARD_SRGB, VA_SOURCE_RANGE_FULL)
        } else {
            (out_standard, VA_SOURCE_RANGE_REDUCED)
        };
        Self {
            surface: source,
            surface_region: std::ptr::null(),
            surface_color_standard: in_standard,
            output_region: std::ptr::null(),
            output_background_color: 0xff00_0000,
            output_color_standard: out_standard,
            pipeline_flags: 0,
            filter_flags: 0,
            filters: std::ptr::null_mut(),
            num_filters: 0,
            forward_references: std::ptr::null_mut(),
            num_forward_references: 0,
            backward_references: std::ptr::null_mut(),
            num_backward_references: 0,
            rotation_state: 0,
            blend_state: std::ptr::null(),
            mirror_state: 0,
            additional_outputs: std::ptr::null_mut(),
            num_additional_outputs: 0,
            input_surface_flag: 0,
            output_surface_flag: 0,
            input_color_properties: VaProcColorProperties {
                color_range: in_range,
                ..Default::default()
            },
            output_color_properties: VaProcColorProperties {
                color_range: VA_SOURCE_RANGE_REDUCED,
                ..Default::default()
            },
            processing_mode: 0,
            output_hdr_metadata: std::ptr::null(),
            va_reserved: [0; 16],
        }
    }
}

/// Measured by `layout-probe.c` against libva 2.22 on `.50`.
const _: () = {
    assert!(size_of::<VaRectangle>() == 8);
    assert!(offset_of!(VaRectangle, width) == 4);
    assert!(size_of::<VaProcColorProperties>() == 8);
    assert!(offset_of!(VaProcColorProperties, matrix_coefficients) == 4);
    assert!(size_of::<VaProcPipelineParameterBuffer>() == 224);
    assert!(offset_of!(VaProcPipelineParameterBuffer, surface_region) == 8);
    assert!(offset_of!(VaProcPipelineParameterBuffer, surface_color_standard) == 16);
    assert!(offset_of!(VaProcPipelineParameterBuffer, output_region) == 24);
    assert!(offset_of!(VaProcPipelineParameterBuffer, output_color_standard) == 36);
    assert!(offset_of!(VaProcPipelineParameterBuffer, filters) == 48);
    assert!(offset_of!(VaProcPipelineParameterBuffer, forward_references) == 64);
    assert!(offset_of!(VaProcPipelineParameterBuffer, backward_references) == 80);
    assert!(offset_of!(VaProcPipelineParameterBuffer, rotation_state) == 92);
    assert!(offset_of!(VaProcPipelineParameterBuffer, blend_state) == 96);
    assert!(offset_of!(VaProcPipelineParameterBuffer, additional_outputs) == 112);
    assert!(offset_of!(VaProcPipelineParameterBuffer, input_surface_flag) == 124);
    assert!(offset_of!(VaProcPipelineParameterBuffer, input_color_properties) == 132);
    assert!(offset_of!(VaProcPipelineParameterBuffer, output_color_properties) == 140);
    assert!(offset_of!(VaProcPipelineParameterBuffer, processing_mode) == 148);
    assert!(offset_of!(VaProcPipelineParameterBuffer, output_hdr_metadata) == 152);
    assert!(offset_of!(VaProcPipelineParameterBuffer, va_reserved) == 160);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The capture formats the libav path mapped, each to the surface it imports as.
    /// `XR24` is the everyday one: BGRX bytes, so the libva name reverses.
    #[test]
    fn every_capture_fourcc_imports_as_the_right_surface() {
        assert_eq!(
            import_format(DRM_FORMAT_XRGB8888),
            Some((VA_FOURCC_BGRX, VA_RT_FORMAT_RGB32))
        );
        assert_eq!(
            import_format(DRM_FORMAT_ABGR8888),
            Some((VA_FOURCC_RGBA, VA_RT_FORMAT_RGB32))
        );
        assert_eq!(
            import_format(DRM_FORMAT_XRGB2101010),
            Some((VA_FOURCC_X2R10G10B10, VA_RT_FORMAT_RGB32_10))
        );
        assert_eq!(
            import_format(DRM_FORMAT_NV12),
            Some((crate::drm::VA_FOURCC_NV12, VA_RT_FORMAT_YUV420))
        );
        assert_eq!(import_format(drm(b"YUYV")), None);
        // The ten-bit codes are the same four bytes in both namespaces.
        assert_eq!(DRM_FORMAT_XRGB2101010, VA_FOURCC_X2R10G10B10);
        assert_eq!(DRM_FORMAT_NV12, crate::drm::VA_FOURCC_NV12);
    }

    /// RGB in is full range; the encoder's NV12 is limited BT.709. Getting either
    /// wrong is a picture that decodes fine and looks washed out or crushed.
    #[test]
    fn rgb_ingest_states_full_range_in_and_limited_bt709_out() {
        let p = VaProcPipelineParameterBuffer::convert(7, true, false);
        assert_eq!(p.surface, 7);
        assert_eq!(p.surface_color_standard, VA_PROC_COLOR_STANDARD_SRGB);
        assert_eq!(p.input_color_properties.color_range, VA_SOURCE_RANGE_FULL);
        assert_eq!(p.output_color_standard, VA_PROC_COLOR_STANDARD_BT709);
        assert_eq!(
            p.output_color_properties.color_range,
            VA_SOURCE_RANGE_REDUCED
        );
        assert!(p.surface_region.is_null() && p.filters.is_null());

        let p = VaProcPipelineParameterBuffer::convert(7, false, true);
        assert_eq!(p.surface_color_standard, VA_PROC_COLOR_STANDARD_BT2020);
        assert_eq!(
            p.input_color_properties.color_range,
            VA_SOURCE_RANGE_REDUCED
        );
    }
}
