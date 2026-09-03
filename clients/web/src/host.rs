//! The console on a canvas: Skia's `DirectContext` over the WebGL2 context `pf-glue.js` made
//! current, one `Surface` wrapping the drawing buffer, re-wrapped when the canvas resizes.
//!
//! Single-threaded by construction — the page's `requestAnimationFrame` and every event handler
//! land on the same wasm thread — so the state is a `RefCell` in a `thread_local!` and there is
//! none of the Android host's command queue.

use pf_client_core::trust::Settings;
use pf_console_ui::{
    Console, ConsoleEntry, ConsoleHandles, ConsoleOptions, Key, Platform, SnapshotStore, Viewport,
};
use skia_safe::gpu::{self, DirectContext, SurfaceOrigin};
use skia_safe::{ColorType, Surface};
use std::cell::RefCell;
use std::sync::Arc;

/// GL_RGBA8 — the sized internal format of a WebGL2 drawing buffer created with `alpha: true`.
/// The UI canvas is the transparent one: video composites underneath it (plan §1 R1).
const GL_RGBA8: u32 = 0x8058;

/// Skia's resource budget. A quarter of the desktop's 160 MB: the page shares one heap with the
/// decoder and the browser, and plan §5.5 makes the wasm ceiling a thing we measure rather than
/// assume. Raise it only against that measurement.
const GPU_CACHE_BYTES: usize = 40 << 20;

unsafe extern "C" {
    /// Bring up a WebGL2 context on the UI canvas and make it current; `1` on success. Defined in
    /// `web/pf-glue.js`, which is the only place a browser or GL object is named (plan §1 R2).
    fn pf_gl_setup() -> i32;
}

struct App {
    console: Console,
    gpu: DirectContext,
    surface: Option<Surface>,
    size: (i32, i32),
    /// The shell reads settings and known hosts through this; the page will fill it from
    /// `localStorage` in Phase 3. Held because `ConsoleOptions` only borrows it as a trait object.
    _store: Arc<SnapshotStore>,
    /// Model handles the page writes host rows into. Same reason.
    _handles: ConsoleHandles,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Bring up GL, Skia and the shell on a canvas already sized to `width` × `height` device pixels.
/// `0` on failure — the page then shows its own message rather than a blank canvas.
#[unsafe(no_mangle)]
pub extern "C" fn pf_start(width: i32, height: i32) -> i32 {
    match start(width, height) {
        Ok(app) => {
            APP.with(|a| *a.borrow_mut() = Some(app));
            1
        }
        Err(e) => {
            println!("punktfunk-web: console start failed: {e:#}");
            0
        }
    }
}

fn start(width: i32, height: i32) -> anyhow::Result<App> {
    // SAFETY: `pf_gl_setup` is the js-library function linked from `web/pf-glue.js`; it takes no
    // arguments, touches no wasm memory, and returns a plain int.
    if unsafe { pf_gl_setup() } != 1 {
        anyhow::bail!("no WebGL2 context on the UI canvas");
    }
    let interface = gpu::gl::Interface::new_native()
        .ok_or_else(|| anyhow::anyhow!("Skia: no GL interface (is a context current?)"))?;
    let mut context = gpu::direct_contexts::make_gl(interface, None)
        .ok_or_else(|| anyhow::anyhow!("Skia: DirectContext over WebGL2 failed"))?;
    context.set_resource_cache_limit(GPU_CACHE_BYTES);

    let store = Arc::new(SnapshotStore::new(Settings::default(), Vec::new()));
    let handles = ConsoleHandles::new();
    let opts = ConsoleOptions {
        device_name: "Browser".to_string(),
        deck: false,
        fallback_ui: false,
        store: Some(store.clone()),
        platform: Platform::Web,
        gpu_cache_bytes: GPU_CACHE_BYTES,
    };
    let console = Console::new(opts, ConsoleEntry::Home, &handles)?;
    println!("punktfunk-web: console up, {width}×{height}");
    Ok(App {
        console,
        gpu: context,
        surface: None,
        size: (0, 0),
        _store: store,
        _handles: handles,
    })
}

/// Draw one frame at the canvas's current device-pixel size. Called from `requestAnimationFrame`.
#[unsafe(no_mangle)]
pub extern "C" fn pf_frame(width: i32, height: i32) {
    APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return };
        app.draw(width, height);
    });
}

/// One key, as `web/pf-glue.js` mapped it from `KeyboardEvent.code`. `key` indexes [`KEYS`];
/// anything else is dropped, which is how the page keeps browser shortcuts working.
#[unsafe(no_mangle)]
pub extern "C" fn pf_key(key: i32, shift: i32, repeat: i32) {
    let Some(&k) = usize::try_from(key).ok().and_then(|i| KEYS.get(i)) else {
        return;
    };
    APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return };
        app.console.key(k, shift != 0, repeat != 0);
    });
}

/// The page's key table, by index. Order is the wire between `pf-glue.js` and here; append only.
const KEYS: [Key; 13] = [
    Key::Left,
    Key::Right,
    Key::Up,
    Key::Down,
    Key::Return,
    Key::Space,
    Key::Escape,
    Key::Backspace,
    Key::PageUp,
    Key::PageDown,
    Key::Tab,
    Key::Y,
    Key::X,
];

impl App {
    fn draw(&mut self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        if self.surface.is_none() || self.size != (width, height) {
            self.surface = self.wrap(width, height);
            self.size = (width, height);
        }
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let viewport = Viewport::plain(width as u32, height as u32);
        self.console
            .frame(surface.canvas(), &viewport, None, None, &[]);
        self.gpu.flush_and_submit();
    }

    /// A Skia surface over the WebGL2 drawing buffer (framebuffer 0). No MSAA and no stencil: the
    /// console asks for neither, and both cost memory on a page that will also hold decoded video.
    fn wrap(&mut self, width: i32, height: i32) -> Option<Surface> {
        let fb = gpu::gl::FramebufferInfo {
            fboid: 0,
            format: GL_RGBA8,
            protected: gpu::Protected::No,
        };
        let target = gpu::backend_render_targets::make_gl((width, height), None, 0, fb);
        gpu::surfaces::wrap_backend_render_target(
            &mut self.gpu,
            &target,
            SurfaceOrigin::BottomLeft,
            ColorType::RGBA8888,
            None,
            None,
        )
    }
}
