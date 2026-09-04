// The video plane on WebGPU (design/web-client-implementation-plan.md WP4.1) — the same
// `configure` / `present` / `resize` / `setDynamicRange` seam as `video-surface.js`, so choosing
// between them is one line in the page and nothing else in the client changes. That is what R2
// was buying.
//
// Two things this gets that WebGL2 cannot:
//
//   * `importExternalTexture()` takes a `VideoFrame` directly and the resulting
//     `GPUExternalTexture` stays valid until the frame is closed — so the per-frame
//     `texImage2D` copy disappears. Measurement said that copy is already cheap (~0.5 ms at 4K
//     against decoder output), so this is not why we are here.
//   * `toneMapping: { mode: "extended" }` on the canvas configuration, which is the **only**
//     shipped HDR route in either engine. Chromium's WebGL2 path (`drawingBufferStorage` +
//     float16) is still behind a flag, and Safari has no WebGL2 HDR at all. This is the reason.
//
// `create()` is async because adapter and device are; everything after is synchronous, so the
// per-frame path matches the WebGL2 one call for call.

(function (global) {
  "use strict";

  const SHADER = `
struct VertexOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f };

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VertexOut {
  // One oversized triangle; cheaper than a quad and no seam down the middle.
  var p = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  var out: VertexOut;
  out.pos = vec4f(p[i], 0.0, 1.0);
  // A VideoFrame's origin is top-left, the surface's is bottom-left.
  out.uv = vec2f(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5);
  return out;
}

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var frame: texture_external;

@fragment
fn fs(in: VertexOut) -> @location(0) vec4f {
  // textureSampleBaseClampToEdge is the required entry point for an external texture: it is
  // what applies the frame's own colour-space conversion, which is where BT.2020/PQ survives.
  return textureSampleBaseClampToEdge(frame, samp, in.uv);
}`;

  class VideoSurfaceWebGPU {
    /// Adapter, device and canvas context. Rejects when the engine has no WebGPU, so a caller can
    /// fall back to the WebGL2 surface rather than showing nothing.
    static async create(canvas) {
      if (!navigator.gpu) throw new Error("no WebGPU in this engine");
      const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
      if (!adapter) throw new Error("no WebGPU adapter");
      const device = await adapter.requestDevice();
      return new VideoSurfaceWebGPU(canvas, device);
    }

    constructor(canvas, device) {
      this.canvas = canvas;
      this.device = device;
      this.context = canvas.getContext("webgpu");
      if (!this.context) throw new Error("no WebGPU canvas context");
      this.format = navigator.gpu.getPreferredCanvasFormat();
      this.width = 0;
      this.height = 0;
      this.frames = 0;
      this.uploadMs = 0;
      this.dynamicRange = "standard";

      const module = device.createShaderModule({ code: SHADER });
      this.pipeline = device.createRenderPipeline({
        layout: "auto",
        vertex: { module, entryPoint: "vs" },
        fragment: { module, entryPoint: "fs", targets: [{ format: this.format }] },
        primitive: { topology: "triangle-list" },
      });
      this.sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
      this.#configureContext();
    }

    #configureContext() {
      const config = {
        device: this.device,
        format: this.format,
        alphaMode: "opaque",
      };
      // The HDR switch. `extended` lets values above 1.0 through to the panel; without it the
      // compositor clamps and BT.2020/PQ content is indistinguishable from SDR. Guarded because
      // an engine with WebGPU but without tone mapping must still show a picture.
      if (this.dynamicRange === "high") {
        config.toneMapping = { mode: "extended" };
      }
      try {
        this.context.configure(config);
      } catch (e) {
        delete config.toneMapping;
        this.context.configure(config);
        this.toneMappingUnavailable = true;
      }
    }

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
    }

    /// Upload-free present: the frame is bound as an external texture for this submission only.
    /// The caller still owns the frame and must `close()` it.
    present(frame) {
      const w = frame.displayWidth || frame.codedWidth || frame.width;
      const h = frame.displayHeight || frame.codedHeight || frame.height;
      if (w && h) this.resize(w, h);

      const t0 = performance.now();
      const device = this.device;
      // No copy: this is the line Phase 4 exists for. The external texture is valid for this
      // submission, which is why the bind group is rebuilt per frame rather than cached.
      const external = device.importExternalTexture({ source: frame });
      const bind = device.createBindGroup({
        layout: this.pipeline.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: this.sampler },
          { binding: 1, resource: external },
        ],
      });
      const encoder = device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [{
          view: this.context.getCurrentTexture().createView(),
          loadOp: "clear",
          storeOp: "store",
          clearValue: { r: 0, g: 0, b: 0, a: 1 },
        }],
      });
      pass.setPipeline(this.pipeline);
      pass.setBindGroup(0, bind);
      pass.draw(3);
      pass.end();
      device.queue.submit([encoder.finish()]);
      this.uploadMs += performance.now() - t0;
      this.frames++;
    }

    /// SDR or HDR for this plane. Reconfigures the canvas, because tone mapping is a property of
    /// the configuration rather than of a draw.
    setDynamicRange(mode) {
      const want = mode === "high" ? "high" : "standard";
      if (want === this.dynamicRange) return;
      this.dynamicRange = want;
      this.#configureContext();
      this.canvas.style.dynamicRangeLimit = want === "high" ? "no-limit" : "standard";
    }

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
      this.context.unconfigure();
      this.device.destroy();
    }
  }

  global.PfVideoSurfaceWebGPU = VideoSurfaceWebGPU;
})(globalThis);
