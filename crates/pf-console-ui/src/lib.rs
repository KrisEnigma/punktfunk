//! Skia console UI: one shell — home, library, settings, add-host, pairing —
//! drawn onto whatever `skia_safe::Canvas` a host hands it. Design:
//! `linux-client-rearchitecture.md`, `android-skia-console-port.md`.
//!
//! Two hosts sit on the portable [`Console`] driver:
//! - the Vulkan session's [`SkiaOverlay`] (feature `vulkan-overlay`) — an
//!   [`Overlay`](pf_presenter::overlay::Overlay) on the presenter's device,
//!   offscreen RGBA, composited as one premultiplied quad. Skia never
//!   touches the swapchain; the overlay draws only when it has something
//!   to show (`skia_overlay.rs`);
//! - the Android GL host (`clients/android/native`), which owns EGL and
//!   drives [`Console`] with `default-features = false`.
//!
//! Everything but `skia_overlay.rs` is platform-free: screens draw to
//! `&Canvas`, settings through [`store::SettingsStore`], keys as
//! [`input::Key`], platform rows as [`platform::Platform`].

#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod anim;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod collate;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod console;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod glyphs;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod icons;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod input;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod launcher_icons;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod library;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod model;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod os_marks;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod os_theme;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod platform;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod pointer;
// In-stream ring is the desktop shell's (Android has Compose). Android
// draws this module only as the settings editor; the host-action cache
// is desktop-gated and is not consulted there.
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod ring;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod screens;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod shell;
#[cfg(all(any(target_os = "linux", windows), feature = "vulkan-overlay"))]
mod skia_overlay;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub mod store;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod theme;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
mod widgets;

#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use console::{Console, ConsoleEntry, ConsoleHandles, InputSource, Insets, Viewport};
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use input::Key;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use library::{LibraryGame, LibraryPhase, LibraryShared, Stale};
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use model::{
    ConsoleBus, ConsoleCmd, ConsoleShared, HostAction, HostRow, PairPhase, ProfileChip, WakeStatus,
};
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use platform::{Platform, PlatformScreen};
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use shell::{ConsoleOptions, DEFAULT_GPU_CACHE_BYTES};
#[cfg(all(any(target_os = "linux", windows), feature = "vulkan-overlay"))]
pub use skia_overlay::SkiaOverlay;
#[cfg(any(
    target_os = "linux",
    windows,
    target_os = "android",
    target_family = "wasm"
))]
pub use store::{SettingsStore, SnapshotStore};
