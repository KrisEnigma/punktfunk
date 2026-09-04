//! The browser client's console host (`design/web-client-implementation-plan.md` WP0.2): the
//! same `pf_console_ui::Console` every other client draws, on Skia's GL backend over a WebGL2
//! canvas.
//!
//! The page owns the loop. `requestAnimationFrame` in `web/index.html` calls [`pf_frame`], which
//! is the whole pacing story for this tier (`web-client.md` §1: no presenter here) and keeps the
//! measurement of it — plan §5.3 — on the JavaScript side where it can be compared against the
//! browser's own timing.
//!
//! Rule R2 of the plan lives here as an absence: **this crate names no GL, WebGPU or browser
//! type**. The context comes up in `web/pf-glue.js` and is already current when [`pf_start`]
//! runs; Skia then assembles its interface over emscripten's GL entry points. Video, when it
//! arrives in Phase 2, lands on the *other* canvas and never crosses into this heap (R3), which
//! is what makes the WebGPU swap a change to that one JavaScript file.

#[cfg(not(target_family = "wasm"))]
fn main() {
    eprintln!(
        "punktfunk-client-web builds for wasm32-unknown-emscripten only: clients/web/build.sh"
    );
}

#[cfg(target_family = "wasm")]
fn main() {
    // Emscripten keeps the runtime alive after `main` returns; the page drives everything from
    // `pf_start` once it has sized the canvas.
}

// Neither is gated: nothing in them is browser-specific, and off wasm is where their tests run.
// The SPAKE2 half in particular is checked against the host's own role B, which a wasm-only
// test would never execute.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod credential;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod ecdsa;
#[cfg(target_family = "wasm")]
mod host;
#[cfg(target_family = "wasm")]
mod session;
#[cfg(target_family = "wasm")]
mod transport;
