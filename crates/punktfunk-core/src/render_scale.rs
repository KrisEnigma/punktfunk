//! Client render-scale: ask the host for `chosen resolution × scale` as a
//! [`Mode`](crate::Mode). The host scales nothing on a virtual display; the
//! presenter downscales (`> 1`) or upscales (`< 1`) after decode. A mirrored
//! head larger than the client is the exception: [`fit_inside`] sizes the encoder.
//!
//! Multiply, keep the aspect ratio, even-floor (host `validate_dimensions`
//! rejects odd sizes), clamp to the codec per-axis ceiling so a connect cannot
//! request a size the encoder will refuse. Twin of
//! `PunktfunkShared/RenderScale.swift`. Pure; tested here.

/// Under-render floor; presenter upscales.
pub const MIN_SCALE: f64 = 0.5;
/// Supersample cap; still clamped per axis by [`max_dimension`].
pub const MAX_SCALE: f64 = 4.0;

/// Picker stops; `1.0` is Native. Shared so every client's list matches.
pub const PRESETS: [f64; 9] = [0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];

/// Per-axis encoder ceiling for a client `codec` string. H.264 is 4096;
/// everything else (including `"auto"`, which negotiates HEVC/AV1) is 8192 —
/// same walls as `pf-encode`'s `codec.rs::max_dimension`.
pub fn max_dimension(codec: &str) -> u32 {
    if codec == "h264" {
        4096
    } else {
        8192
    }
}

/// NaN or `<= 0` becomes `1.0` (Native); otherwise clamp to
/// `[MIN_SCALE, MAX_SCALE]`.
pub fn sanitize(raw: f64) -> f64 {
    if raw.is_nan() || raw <= 0.0 {
        return 1.0;
    }
    raw.clamp(MIN_SCALE, MAX_SCALE)
}

/// Scale a base size: keep aspect, even-floor, uniform-clamp so neither axis
/// exceeds `max_dim`. Each axis floors at 320×200 (host rejects smaller).
pub fn apply(base_w: u32, base_h: u32, scale: f64, max_dim: u32) -> (u32, u32) {
    let scale = sanitize(scale);
    let mut w = base_w.max(1) as f64 * scale;
    let mut h = base_h.max(1) as f64 * scale;
    let cap = max_dim as f64;
    let over = (w / cap).max(h / cap);
    if over > 1.0 {
        w /= over;
        h /= over;
    }
    (even_floor(w, 320), even_floor(h, 200))
}

/// Shrink `w`×`h` to fit inside `max_w`×`max_h`: keep aspect, even-floor, never
/// grow. Integer maths, so the binding axis lands on the box exactly. Not
/// [`apply`]: its `sanitize` floors the scale at [`MIN_SCALE`], and a 4K head
/// into an 800p client needs a third.
pub fn fit_inside(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let (w, h, max_w, max_h) = (
        u64::from(w),
        u64::from(h),
        u64::from(max_w),
        u64::from(max_h),
    );
    let (fw, fh) = if w * max_h >= h * max_w {
        (max_w, h * max_w / w.max(1))
    } else {
        (w * max_h / h.max(1), max_h)
    };
    (even_floor(fw as f64, 320), even_floor(fh as f64, 200))
}

fn even_floor(value: f64, minimum: u32) -> u32 {
    let v = (value.floor() as i64).max(minimum as i64).max(0) as u32;
    v / 2 * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_and_defaults() {
        assert_eq!(sanitize(0.0), 1.0);
        assert_eq!(sanitize(-3.0), 1.0);
        assert_eq!(sanitize(f64::NAN), 1.0);
        assert_eq!(sanitize(0.1), 0.5);
        assert_eq!(sanitize(9.0), 4.0);
        assert_eq!(sanitize(1.5), 1.5);
    }

    #[test]
    fn max_dimension_is_codec_aware() {
        assert_eq!(max_dimension("h264"), 4096);
        assert_eq!(max_dimension("hevc"), 8192);
        assert_eq!(max_dimension("av1"), 8192);
        assert_eq!(max_dimension("auto"), 8192);
    }

    #[test]
    fn native_is_identity() {
        assert_eq!(apply(1920, 1080, 1.0, 8192), (1920, 1080));
    }

    #[test]
    fn supersample_doubles() {
        assert_eq!(apply(1920, 1080, 2.0, 8192), (3840, 2160));
    }

    #[test]
    fn under_render_halves() {
        assert_eq!(apply(1920, 1080, 0.5, 8192), (960, 540));
    }

    #[test]
    fn results_are_even() {
        let (w, h) = apply(1366, 768, 1.5, 8192);
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert_eq!((w, h), (2048, 1152));
    }

    #[test]
    fn over_ceiling_clamps_uniformly() {
        let (w, h) = apply(3840, 2160, 4.0, 8192);
        assert!(w <= 8192 && h <= 8192);
        assert_eq!((w, h), (8192, 4608));
    }

    #[test]
    fn h264_ceiling_is_tighter() {
        assert_eq!(apply(1920, 1080, 4.0, 4096), (4096, 2304));
    }

    #[test]
    fn fit_inside_shrinks_a_4k_head_to_an_800p_client() {
        assert_eq!(fit_inside(3840, 2160, 1280, 800), (1280, 720));
    }

    #[test]
    fn fit_inside_leaves_a_matching_panel_alone() {
        assert_eq!(fit_inside(1920, 1080, 1920, 1080), (1920, 1080));
    }

    #[test]
    fn fit_inside_never_upscales() {
        assert_eq!(fit_inside(1280, 800, 3840, 2160), (1280, 800));
    }

    #[test]
    fn fit_inside_is_even_on_the_free_axis() {
        assert_eq!(fit_inside(3440, 1440, 1280, 800), (1280, 534));
        assert_eq!(fit_inside(2160, 3840, 1280, 800), (450, 800));
    }

    #[test]
    fn minimum_floor_honoured() {
        let (w, h) = apply(400, 300, 0.5, 8192);
        assert!(w >= 320 && h >= 200);
    }
}
