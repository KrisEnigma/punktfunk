//! The cursor as the encode pool blends it: one [`CursorCell`] per monitor, written by the
//! cursor worker on every hardware-cursor publish and read by the pool at every frame.
//!
//! `blend` is the render model: on while the client draws no pointer and a hardware cursor is
//! declared on the adapter, which excludes the pointer from every frame for the WUDFHost's
//! life. The image is frame-relative (the desktop position minus the host-stamped origin) so
//! the pool can draw it without knowing where the monitor sits. Pure: no DDI, no handle.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use pf_driver_proto::cursor::{CursorShm, ShapeRgba};

/// Straight-alpha RGBA at its frame-relative top-left. `serial` is the OS shape id.
#[derive(Clone)]
#[cfg_attr(not(feature = "driver-encode"), allow(dead_code))]
pub struct CursorImage {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub rgba: Arc<Vec<u8>>,
    pub serial: u32,
    pub visible: bool,
}

/// See the module docs. The worker and the pool hold clones; neither pins the monitor.
#[derive(Default)]
pub struct CursorCell {
    pub image: Mutex<Option<CursorImage>>,
    pub blend: AtomicBool,
    /// The host's SDR-white scale for an FP16 frame, as `f32` bits; 0 until the first publish.
    pub sdr_white_scale: AtomicU32,
}

impl CursorCell {
    /// The pointer to blend now: `None` unless blending is on and a visible shape was
    /// published. The scale is the FP16 SDR-white factor (1.0 when the host stamped none).
    #[cfg(feature = "driver-encode")]
    pub fn to_blend(&self) -> Option<(CursorImage, f32)> {
        if !self.blend.load(Ordering::Relaxed) {
            return None;
        }
        let image = crate::registry::lock(&self.image).clone()?;
        if !image.visible {
            return None;
        }
        let bits = self.sdr_white_scale.load(Ordering::Relaxed);
        let scale = if bits == 0 { 1.0 } else { f32::from_bits(bits) };
        Some((image, scale))
    }

    /// Fold one worker tick in: a new `shape` replaces the pixels, every tick moves the
    /// position and visibility. `image` is the worker's copy, kept across shape-less ticks;
    /// nothing is published until the first shape.
    pub fn publish(
        &self,
        image: &mut Option<CursorImage>,
        hdr: &CursorShm,
        shape: Option<ShapeRgba>,
        visible: bool,
    ) {
        self.sdr_white_scale
            .store(hdr.sdr_white_scale, Ordering::Relaxed);
        if let Some(s) = shape {
            *image = Some(CursorImage {
                x: 0,
                y: 0,
                w: s.w,
                h: s.h,
                hot_x: s.hot_x,
                hot_y: s.hot_y,
                rgba: Arc::new(s.rgba),
                serial: hdr.shape_id,
                visible,
            });
        }
        let Some(img) = image.as_mut() else {
            return;
        };
        img.x = hdr.x - hdr.origin_x;
        img.y = hdr.y - hdr.origin_y;
        img.visible = visible;
        *crate::registry::lock(&self.image) = Some(img.clone());
    }
}
