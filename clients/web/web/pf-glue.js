// The one file that names a browser or graphics object (design/web-client-implementation-plan.md
// §1 R2). Emscripten links it with `--js-library`, so these functions run inside the module's
// own scope — which is where `GL`, emscripten's WebGL bookkeeping, lives. Nothing in the Rust
// tree may name a GL or GPU type, and that is a review rule.
//
// Phase 4 replaces the video plane's implementation here and nowhere else: `VideoSurface`
// (`configure` / `present` / `resize` / `set_dynamic_range`) arrives with the decoder in WP2.3
// and its WebGPU twin in WP4.1. The console's context below is not part of that swap — Skia
// keeps its own canvas (R1), so our draw calls and Ganesh's never share a context (R6).

mergeInto(LibraryManager.library, {
  // Bring up a WebGL2 context on the UI canvas and make it current for emscripten's GL layer.
  // Returns 1 on success, 0 if the browser gave us no WebGL2. Idempotent: a second call just
  // re-makes the existing context current.
  pf_gl_setup: function () {
    if (Module.__pfUiCtx) {
      GL.makeContextCurrent(Module.__pfUiCtx);
      return 1;
    }
    var canvas = document.getElementById("pf-ui");
    if (!canvas) return 0;
    // `alpha: true` is what lets the video canvas show through wherever the console draws
    // nothing (R1). `antialias: false` because Skia does its own; `depth`/`stencil` off
    // because `wrap_backend_render_target` asks for neither.
    var handle = GL.createContext(canvas, {
      majorVersion: 2,
      minorVersion: 0,
      alpha: true,
      antialias: false,
      depth: false,
      stencil: false,
      premultipliedAlpha: true,
      preserveDrawingBuffer: false,
      // The console is redrawn every frame from scratch; a discrete GPU is the right ask on a
      // laptop that has both, since this canvas is composited with decoded video.
      powerPreference: "high-performance",
    });
    if (!handle) return 0;
    GL.makeContextCurrent(handle);
    Module.__pfUiCtx = handle;
    return 1;
  },
});
