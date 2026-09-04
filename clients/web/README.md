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
| `src/transport.rs` | The datagram ring and `punktfunk_core`'s `Transport` over it. |
| `web/pf-glue.js` | Emscripten `--js-library`. **The only file that names a browser or GL object.** |

`web/pf-glue.js` is load-bearing, not a detail. Rule R2 of the implementation plan says exactly one
seam may know the graphics API, and this is it — video will land on the lower canvas through the
same file, which is what makes the eventual WebGPU swap a change to one file instead of a rewrite.
Nothing in `src/` may name a GL or GPU type; that is a review rule.

## Tests

```sh
RUSTFLAGS="-C link-arg=-sDEFAULT_TO_CXX=1 -C link-arg=-sALLOW_MEMORY_GROWTH=1 \
  -C link-arg=-sMAX_WEBGL_VERSION=2 -C link-arg=-sNODERAWFS=1" \
EMCC_CFLAGS="-fwasm-exceptions" \
SKIA_BINARIES_URL="file://$HOME/.cache/punktfunk/skia-wasm/skia-binaries-{key}.tar.gz" \
CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUNNER=node \
cargo test -p pf-console-ui --target wasm32-unknown-emscripten --no-default-features
```

231 pass, including the `clients/shared/console-vectors.json` parity vectors. The client's own
tests (the datagram ring) need the glue linked as well, since the ring calls into it:

```sh
RUSTFLAGS="... -C link-arg=--js-library -C link-arg=$PWD/clients/web/web/pf-glue.js" \
CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUNNER=node \
cargo test -p punktfunk-client-web --target wasm32-unknown-emscripten
```
 `NODERAWFS` is
what lets the source-scanning lint read the crate's own `src`; without it that one test fails on a
missing directory.

The wasm harness is **single-threaded**, which makes it stricter than the desktop one: libtest
normally gives every test its own thread, and `theme::reduce_motion` is a thread-local, so tests
that disagree about motion only stay isolated by accident there. State what a test needs rather
than inherit it.

## Known gaps

- No `VideoSurface` yet. The lower canvas is present and empty by design — the seam arrives with
  the decoder in WP2.3, and inventing it before there is a frame to put through it would be
  guessing at the interface.
- The release module is **8.0 MB of wasm plus 114 KB of JS**, unstripped and un-`wasm-opt`ed. Plan
  §5.5 wants a measured heap ceiling; this is the payload half of it.
- The ring is wired but nothing decodes yet. `pf_net_blast` / `pf_net_drain` exist to measure the
  seam (plan §5.4) and to keep it exercised until the session pump lands on it in WP2.2.

## Measured, Safari 27 → a Linux host over the LAN

5000 datagrams of 1200 B, echoed back by the host's browser plane:

| | |
|---|---|
| Send crossing, Rust → JS → the wire | **1.8 µs** per datagram (5000 queued in 9 ms) |
| Round trip, including LAN and the host echo | **97 µs** per datagram |
| Sustained | **10,267 datagrams/s**, zero ring drops |

Plan §3 sizes the hot path for 4–5k/s at 50 Mbps, so the ring has about twice the headroom it
needs and is not the thing to optimise. One caveat worth keeping: drain on `requestAnimationFrame`,
not `setTimeout`. A background tab throttles timers to 1 Hz, and the same run that drops nothing on
rAF dropped 688 of 5000 on a 25 ms timer — the ring holds ~50 ms at this rate.
