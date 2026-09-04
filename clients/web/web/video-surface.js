// The video plane. Rule R2 of design/web-client-implementation-plan.md: exactly one seam knows
// the graphics API, and this is it for video. `configure` / `present` / `resize` /
// `setDynamicRange` is the whole interface, and Phase 4 replaces the body with a WebGPU one
// without any caller learning about it.
//
// Two properties this file exists to protect:
//
//   R1  It draws on the LOWER canvas, its own. The Skia console is a separate canvas above with
//       alpha, so our draw calls and Ganesh's never share a context — none of the reset/flush
//       discipline that mixing them would need applies, and the WebGPU swap touches this file
//       alone.
//   R3  Decoded pixels never enter the wasm heap. A `VideoFrame` goes straight from `VideoDecoder`
//       into a texture here. Rust deals in access units and never sees a pixel, which is what
//       makes the swap a JavaScript-only change: `importExternalTexture()` takes a `VideoFrame`
//       directly, where `texImage2D` sits today.
//
// The caller owns the frame it passes to `present` and must `close()` it afterwards. Holding one
// stalls the decoder — WebCodecs bounds how many frames may be outstanding.

(function (global) {
  "use strict";

  const VERT = `#version 300 es
in vec2 pos;
out vec2 uv;
void main() {
  // Full-screen triangle pair from clip space; flip V because a VideoFrame's origin is top-left
  // and a GL texture's is bottom-left.
  uv = vec2(pos.x * 0.5 + 0.5, 0.5 - pos.y * 0.5);
  gl_Position = vec4(pos, 0.0, 1.0);
}`;

  const FRAG = `#version 300 es
precision mediump float;
in vec2 uv;
uniform sampler2D tex;
out vec4 colour;
void main() { colour = texture(tex, uv); }`;

  function compile(gl, type, src) {
    const sh = gl.createShader(type);
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      const log = gl.getShaderInfoLog(sh);
      gl.deleteShader(sh);
      throw new Error("video surface shader: " + log);
    }
    return sh;
  }

  class VideoSurface {
    constructor(canvas) {
      this.canvas = canvas;
      // `alpha: false` — this is the bottom layer and nothing shows through it; opaque lets the
      // compositor skip a blend. `preserveDrawingBuffer: false` because every frame is a full
      // overwrite. `desynchronized` cuts a compositor hop on the plane where latency is the point.
      this.gl = canvas.getContext("webgl2", {
        alpha: false,
        antialias: false,
        depth: false,
        stencil: false,
        desynchronized: true,
        preserveDrawingBuffer: false,
        powerPreference: "high-performance",
      });
      if (!this.gl) throw new Error("no WebGL2 context for the video plane");

      const gl = this.gl;
      const prog = gl.createProgram();
      gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VERT));
      gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FRAG));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
        throw new Error("video surface link: " + gl.getProgramInfoLog(prog));
      }
      gl.useProgram(prog);
      this.program = prog;

      const vao = gl.createVertexArray();
      gl.bindVertexArray(vao);
      const vbo = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(prog, "pos");
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

      this.tex = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, this.tex);
      // No mips and clamped: a video frame is sampled 1:1 and never tiles.
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.uniform1i(gl.getUniformLocation(prog, "tex"), 0);

      this.width = 0;
      this.height = 0;
      this.frames = 0;
      /// Total milliseconds spent in `present`, so a caller can report the per-frame upload cost
      /// plan §5.2 asks for without timing every call itself.
      this.uploadMs = 0;
    }

    /// Source dimensions and colour. `colorSpace` comes from the frame's own `VideoColorSpace`;
    /// v1 is SDR, and the HDR work in Phase 4 is a change here plus a CSS `dynamic-range-limit`,
    /// never a change in a caller.
    configure(width, height, colorSpace) {
      this.colorSpace = colorSpace || null;
      this.resize(width, height);
    }

    resize(width, height) {
      if (width === this.width && height === this.height) return;
      this.width = width;
      this.height = height;
      if (this.canvas.width !== width) this.canvas.width = width;
      if (this.canvas.height !== height) this.canvas.height = height;
      this.gl.viewport(0, 0, width, height);
    }

    /// Upload one frame and draw it. The caller keeps ownership and must `close()` the frame.
    present(frame) {
      const gl = this.gl;
      const w = frame.displayWidth || frame.codedWidth || frame.width;
      const h = frame.displayHeight || frame.codedHeight || frame.height;
      if (w && h) this.resize(w, h);

      const t0 = performance.now();
      gl.bindTexture(gl.TEXTURE_2D, this.tex);
      // The measured line. `texImage2D` from a VideoFrame is the copy Phase 4's
      // `importExternalTexture()` removes; §5.2 is the number that says whether removing it is a
      // performance win or only an HDR one.
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, frame);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      this.uploadMs += performance.now() - t0;
      this.frames++;
    }

    /// SDR or HDR for this plane. The console above stays SDR either way — it is authored that
    /// way, and its own canvas carries `dynamic-range-limit: standard` (§1 R5).
    setDynamicRange(mode) {
      this.canvas.style.dynamicRangeLimit = mode === "high" ? "no-limit" : "standard";
    }

    /// Average milliseconds in `present` since the last call to this.
    takeUploadStats() {
      const stats = {
        frames: this.frames,
        totalMs: this.uploadMs,
        perFrameMs: this.frames ? this.uploadMs / this.frames : 0,
      };
      this.frames = 0;
      this.uploadMs = 0;
      return stats;
    }

    destroy() {
      const gl = this.gl;
      gl.deleteTexture(this.tex);
      gl.deleteProgram(this.program);
      const lose = gl.getExtension("WEBGL_lose_context");
      if (lose) lose.loseContext();
    }
  }

  global.PfVideoSurface = VideoSurface;
})(globalThis);
