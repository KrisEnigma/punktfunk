# punktfunk-client-web

The browser client: `pf-console-ui` on Skia's GL backend over a WebGL2 canvas, compiled to
`wasm32-unknown-emscripten`. Design of record: `punktfunk-planning/design/web-client.md` and
`web-client-implementation-plan.md`.

**Phase 0 of that plan is what is here** — the console draws, and nothing talks to a host yet.
There is no transport, no decoder and no pairing; those are Phases 1–3.

## Build

```sh
source ~/emsdk/emsdk_env.sh      # see the version pin below
clients/web/build.sh             # or --release
python3 -m http.server -d clients/web/dist 8000
```

Then open `http://localhost:8000/` — localhost is a secure context, so nothing needs a
certificate until there is a host to reach.

## The two things that will bite you

### 1 · emsdk 4.0.9, not `latest`

`emsdk install 6.0.9` (or anything with LLVM 21+) fails the build with

```
error[E0425]: cannot find type `_Traits` in this scope
   --> …/out/skia/bindings.rs: pub type std___hash_table___node_allocator = std___rebind_alloc<_Traits>;
```

skia-bindings 0.99's bindgen cannot digest that libc++'s `__hash_table`. Install `4.0.9`, which
is contemporary with skia-safe 0.99 / Skia m150:

```sh
cd ~/emsdk && ./emsdk install 4.0.9 && ./emsdk activate 4.0.9
```

### 2 · Skia is built from source here — on purpose

Every other target downloads a rust-skia prebuilt, and the manifests carry loud comments saying a
source build must never happen. **This target is the exception**, and the reason is not ours to
fix:

- rust-skia's published wasm archives are compiled for **emscripten** exception handling. True at
  0.99.0 and still true at 0.153.2, so a version bump does not solve it.
- Rust's `wasm32-unknown-emscripten` `std` has used **wasm** exception handling since 1.87, and
  the flag that selected the old mode is gone from the compiler.

Linking the two fails from either side. With the prebuilt:

```
wasm-ld: error: …libskia.a(jpeg_decode.SkJpegCodec.o): undefined symbol: emscripten_longjmp
```

and if you strip rustc's `-fwasm-exceptions` to meet it halfway, `std` refuses instead:

```
wasm-ld: error: …libstd-*.rlib(std-*.rcgu.o): undefined symbol: __cpp_exception
```

So `build.sh` sets `EMCC_CFLAGS=-fwasm-exceptions` and `FORCE_SKIA_BUILD=1`. **The cost is paid
once per machine**: the result is packed into `~/.cache/punktfunk/skia-wasm/` and every later
build downloads that through `SKIA_BINARIES_URL` — the same mechanism the webOS armv7 client uses
for its self-hosted archive. `rm -rf ~/.cache/punktfunk/skia-wasm` forces a rebuild.

CI wants that archive built once and hosted, exactly like armv7's, rather than a source build per
run. Re-check at every skia-safe bump: if rust-skia ever publishes a wasm-EH archive, delete this
whole arrangement and go back to the download.

## Shape

| File | What it is |
|---|---|
| `src/main.rs` | Entry point. Off wasm it prints how to build; on wasm it hands the page the loop. |
| `src/host.rs` | Skia `DirectContext` over the canvas, the `Console`, the exported `pf_*` calls. |
| `web/index.html` | The two canvases, the `requestAnimationFrame` loop, key mapping. |
| `web/pf-glue.js` | Emscripten `--js-library`. **The only file that names a browser or GL object.** |

`web/pf-glue.js` is load-bearing, not a detail. Rule R2 of the implementation plan says exactly one
seam may know the graphics API, and this is it — video will land on the lower canvas through the
same file, which is what makes the eventual WebGPU swap a change to one file instead of a rewrite.
Nothing in `src/` may name a GL or GPU type; that is a review rule.

## Known gaps

- **`--release` and `cargo test` do not link yet, for the same reason.** Cargo emits every crate
  type a dependency declares, so `punktfunk-core`'s `cdylib` — there for the Swift/Kotlin
  embedders, wanted by nothing on wasm — is linked as an emscripten `SIDE_MODULE`. wasm-ld exports
  its Rust symbols, and emcc rejects the first mangled name that is not a JS identifier:

  ```
  emcc: error: invalid export name: _ZN101_$LT$punktfunk_core..transport..udp..UdpTransport$u20$as…
  ```

  Which symbol survives depends on the profile, which is why the dev build links and the release
  and test builds do not (thin LTO is already on and does not internalise them). The fix is to stop
  emitting that cdylib for wasm — cargo has no per-target `crate-type`, so it means dropping
  `cdylib`/`staticlib` from `punktfunk-core`'s manifest and having `scripts/build-xcframework.sh`
  and the Android Gradle build ask for them with `cargo rustc --crate-type` instead. That touches
  the Apple and Android build contracts, so it wants its own change with those builds in reach.
- No `VideoSurface` yet. The lower canvas is present and empty by design — the seam arrives with
  the decoder in WP2.3, and inventing it before there is a frame to put through it would be
  guessing at the interface.
