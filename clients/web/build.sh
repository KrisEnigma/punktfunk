#!/usr/bin/env bash
# Build the browser client and lay the servable page out in `clients/web/dist/`.
#
#   clients/web/build.sh [--release]
#
# Needs an activated emsdk on PATH (`emcc`) and the `wasm32-unknown-emscripten` Rust target.
# README.md has the emsdk pin and why Skia is built rather than downloaded here.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
profile="debug"
cargo_profile=()
if [ "${1:-}" = "--release" ]; then profile="release"; cargo_profile=(--release); fi

command -v emcc >/dev/null || {
  echo "emcc not on PATH: source your emsdk's emsdk_env.sh first (README.md pins the version)" >&2
  exit 1
}
: "${EMSDK:?set EMSDK to your emsdk root — skia-bindings reads it to find the sysroot}"

# --- Skia -------------------------------------------------------------------------------------
#
# 🛑 This is the ONE target where we build Skia instead of downloading it, and it is not a
# packaging oversight. rust-skia's published wasm archives — 0.99.0 and 0.153.2 alike — are
# compiled for EMSCRIPTEN exception handling, while Rust's `wasm32-unknown-emscripten` std has
# used WASM exception handling since 1.87. The two cannot be linked: with the prebuilt the link
# ends in `undefined symbol: emscripten_longjmp`, and stripping rustc's `-fwasm-exceptions` to
# meet it halfway ends in `undefined symbol: __cpp_exception` from libstd instead. `EMCC_CFLAGS`
# below is what puts Skia on Rust's side of that line.
#
# The cost is paid once per machine: the result is packed into an archive under the cache below,
# and every later build downloads that instead — the same `SKIA_BINARIES_URL` mechanism the webOS
# armv7 client uses for its self-hosted archive. Delete the cache to force a rebuild.
skia_key="a25a0fdb7d90429aa2d1-wasm32-unknown-emscripten-gl-jpegd-jpege-pdf-textlayout"
cache="${PF_SKIA_WASM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/punktfunk/skia-wasm}"
archive="$cache/skia-binaries-$skia_key.tar.gz"
# Match what rustc emits for this target. Applies to every emcc the build runs, which is what we
# want here: Skia's objects and ours must agree about exceptions.
export EMCC_CFLAGS="${EMCC_CFLAGS:-} -fwasm-exceptions"
if [ -f "$archive" ]; then
  export SKIA_BINARIES_URL="file://$cache/skia-binaries-{key}.tar.gz"
else
  echo "==> no cached Skia wasm archive — building it from source (30+ minutes, once)"
  export FORCE_SKIA_BUILD=1
fi

link=(
  # rustc drives `emcc`, not `em++`, so nothing pulls libc++ in — and Skia is C++. Without this
  # the link ends in pages of `undefined symbol: operator new(unsigned long)` from skparagraph.
  -C link-arg=-sDEFAULT_TO_CXX=1
  # `MODULARIZE` so the page starts the module once it has sized the canvas, not on script parse.
  -C link-arg=-sMODULARIZE=1
  -C link-arg=-sEXPORT_NAME=PunktfunkWeb
  # What makes `GL.createContext({majorVersion: 2})` in pf-glue.js legal, and what supplies the
  # GLES3 entry points Skia's WebGL interface assembles itself from (`emscripten_glGetStringi` …).
  -C link-arg=-sMAX_WEBGL_VERSION=2
  -C link-arg=-sMIN_WEBGL_VERSION=2
  # The shell's glyph atlases and poster decodes are not bounded at start-up; plan §5.5 turns the
  # ceiling into a measurement rather than a guess.
  -C link-arg=-sALLOW_MEMORY_GROWTH=1
  # The ring's entry points are called from pf-glue.js's read loop, so they must be exported
  # even though no page names them.
  -C link-arg=-sEXPORTED_FUNCTIONS=_main,_pf_start,_pf_frame,_pf_key,_pf_rx_base,_pf_rx_stride,_pf_rx_claim,_pf_rx_commit,_pf_rx_dropped,_pf_net_blast,_pf_net_drain,_pf_wt_connect,_pf_wt_close,_malloc,_free
  # `stringToNewUTF8` is how the page hands a host address across; `HEAPU8` is emscripten's
  # view of wasm memory, which pf-glue.js writes datagrams into.
  -C link-arg=-sEXPORTED_RUNTIME_METHODS=stringToNewUTF8,HEAPU8
  -C link-arg=--js-library -C "link-arg=$here/web/pf-glue.js"
)

# `cargo rustc`, not `cargo build`: these are link settings for the page's module alone. Passing
# them through `EMCC_CFLAGS` would also reach the `SIDE_MODULE` link of punktfunk-core's cdylib
# (its `[lib] crate-type` carries one for the Swift/Kotlin embedders), which fails on `_main`.
echo "==> cargo rustc (${profile})"
cargo rustc --manifest-path "$here/Cargo.toml" --target wasm32-unknown-emscripten \
  ${cargo_profile[@]+"${cargo_profile[@]}"} -- "${link[@]}"

# Pack what the source build produced, so the next build is a download. skia-bindings renames
# `lib*.wasm.a` to `lib*.a` on import, so the archive has to carry both spellings.
built="$(ls -td "$root"/target/wasm32-unknown-emscripten/"$profile"/build/skia-bindings-*/out/skia 2>/dev/null | head -1)"
if [ ! -f "$archive" ] && [ -n "$built" ]; then
  echo "==> caching the Skia build as $archive"
  mkdir -p "$cache"
  work="$(mktemp -d)"
  mkdir -p "$work/skia-binaries"
  cp "$built"/bindings.rs "$built"/key.txt "$built"/tag.txt "$built"/LICENSE_SKIA "$work/skia-binaries/" 2>/dev/null || true
  for a in "$built"/lib*.a; do
    case "$a" in *.wasm.a) continue;; esac
    cp "$a" "$work/skia-binaries/"
    cp "$a" "$work/skia-binaries/$(basename "${a%.a}").wasm.a"
  done
  tar czf "$archive.tmp" -C "$work" skia-binaries
  mv "$archive.tmp" "$archive"
  rm -rf "$work"
fi

# --- Page -------------------------------------------------------------------------------------
#
# The emitted JS asks for the underscored wasm name rustc gave the linker; cargo only renames the
# `.js`. Copy both under the names the module actually looks for.
dist="$here/dist"
mkdir -p "$dist"
cp "$here/web/index.html" "$dist/"
cp "$root/target/wasm32-unknown-emscripten/$profile/punktfunk-client-web.js" "$dist/"
cp "$root/target/wasm32-unknown-emscripten/$profile/punktfunk_client_web.wasm" "$dist/"
echo "==> $dist"
ls -la "$dist"
