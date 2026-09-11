#!/bin/sh
# Build PunktfunkCore.xcframework on the Mac runner once per commit.
#
# apple.yml's swift, distribute and screenshots jobs need the same eight-slice bundle for the same
# commit, and each gets a fresh checkout at a new path, which alone makes cargo recompile the
# workspace's crates for every slice. The first job builds (into the persistent target dir, so
# third-party crates stay fresh) and parks a copy under ~/ci/xcframework/<sha>; later jobs copy it.
# Parked copies older than two days are removed.
#
# Usage, from the repo root with GITHUB_SHA set: sh scripts/ci/mac-xcframework.sh
set -eu
sha=${GITHUB_SHA:?GITHUB_SHA is unset}
store="$HOME/ci/xcframework"
bundle=clients/apple/PunktfunkCore.xcframework
mkdir -p "$store"
find "$store" -mindepth 1 -maxdepth 1 -mtime +2 -exec rm -rf {} +

if [ -d "$store/$sha/PunktfunkCore.xcframework" ]; then
    rm -rf "$bundle"
    ditto "$store/$sha/PunktfunkCore.xcframework" "$bundle"
    echo "reused the xcframework built for $sha"
    exit 0
fi

CARGO_TARGET_DIR="$(sh scripts/ci/mac-cargo-target.sh apple)"
export CARGO_TARGET_DIR
BUILD_IOS=1 BUILD_TVOS=1 bash scripts/build-xcframework.sh

# Park it through a temp dir and a rename, so a half-written copy never looks complete.
tmp="$store/.$sha.$$"
mkdir -p "$tmp"
ditto "$bundle" "$tmp/PunktfunkCore.xcframework"
rm -rf "$store/$sha"
mv "$tmp" "$store/$sha"
echo "parked the xcframework for $sha"
