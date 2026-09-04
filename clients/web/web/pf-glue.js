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

  // --- the datagram plane (design/web-client-implementation-plan.md §3) ----------------------
  //
  // Datagrams never become JavaScript objects that outlive one call. The read loop claims a ring
  // slot from Rust and copies the bytes straight into wasm memory; nothing is allocated per
  // packet on either side. At 4-5k/s a `Uint8Array` per datagram is garbage the collector would
  // be chasing during the stream.
  //
  // `$` marks a JavaScript-only helper (not callable from Rust); `__deps` is how emscripten
  // knows to keep one that only other library members use.
  $pfNet: {
    wt: null,
    writer: null,
    reading: false,
  },

  pf_wt_connect__deps: ["$pfNet", "$UTF8ToString"],
  pf_wt_connect: function (urlPtr, hashPtr) {
    var url = UTF8ToString(urlPtr);
    var hex = UTF8ToString(hashPtr);
    try {
      var opts = { allowPooling: false };
      if (hex && hex.length === 64) {
        var bytes = new Uint8Array(32);
        for (var i = 0; i < 32; i++) bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
        // allowPooling must stay false alongside this: the pair is a TypeError otherwise.
        opts.serverCertificateHashes = [{ algorithm: "sha-256", value: bytes }];
      }
      pfNet.wt = new WebTransport(url, opts);
    } catch (e) {
      console.error("punktfunk: WebTransport constructor refused", e);
      return 0;
    }
    pfNet.wt.ready.then(function () {
      // WebKit follows the current spec with `createWritable()`; Chromium still exposes the
      // older `writable` attribute. A client that knows only one fails on the other engine.
      var w = pfNet.wt.datagrams.createWritable
        ? pfNet.wt.datagrams.createWritable()
        : pfNet.wt.datagrams.writable;
      pfNet.writer = w.getWriter();
      if (!pfNet.reading) {
        pfNet.reading = true;
        (function pump(reader) {
          reader.read().then(function (r) {
            if (r.done) { pfNet.reading = false; return; }
            var slot = _pf_rx_claim();
            if (slot >= 0) {
              HEAPU8.set(r.value, _pf_rx_base() + slot * _pf_rx_stride());
              _pf_rx_commit(slot, r.value.length);
            }
            pump(reader);
          }, function () { pfNet.reading = false; });
        })(pfNet.wt.datagrams.readable.getReader());
      }
    }, function (e) {
      console.error("punktfunk: WebTransport session failed", e);
    });
    return 1;
  },

  pf_wt_send__deps: ["$pfNet"],
  pf_wt_send: function (ptr, len) {
    if (!pfNet.writer) return 0;
    // `slice`, not `subarray`: the write is queued, and a view into wasm memory can be detached
    // by a heap growth or overwritten by the next packet before it is read.
    pfNet.writer.write(HEAPU8.slice(ptr, ptr + len)).catch(function () {});
    return 1;
  },

  pf_wt_close__deps: ["$pfNet"],
  pf_wt_close: function () {
    if (pfNet.wt) { try { pfNet.wt.close(); } catch (e) {} }
    pfNet.wt = null;
    pfNet.writer = null;
  },
});
