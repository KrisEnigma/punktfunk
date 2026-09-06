#!/usr/bin/env bash
# punktfunk — the FFmpeg the Steam Deck host links against, built in the pf2 box and carried.
#
# SteamOS is not a fixed ABI target. Its FFmpeg moves independently of any Debian release, and the
# preview channel is already past trixie's — a host linked against the box's FFmpeg then builds
# clean and cannot load, and every Deck breaks the day that lands on stable. So the host carries
# its own: the same pinned LGPL shared build the .deb bundles, behind an absolute rpath.
#
# Absolute, not $ORIGIN: the encode worker is setcap'd, and glibc drops an $ORIGIN rpath for a
# secure binary (packaging/debian/build-deb.sh says the same thing for the .deb).
#
# Idempotent — a stamp file holds the tag it built, so a re-run is free until the pin moves.
# ~4 minutes on a Deck, once per pin. Called by install.sh and update.sh.
set -euo pipefail

log()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m  ok\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

SRC="${PUNKTFUNK_SRC:-$HOME/punktfunk}"
BOX="${PUNKTFUNK_BOX:-pf2}"
PREFIX="${PUNKTFUNK_FFMPEG_PREFIX:-$SRC/target-steamos/ffmpeg}"

# The host links its own carried FFmpeg, so the build box must not also ship libav*-dev: with both
# present the linker resolves -lavcodec to the box's /usr copy before the carried one, leaving the
# host needing that soname while its rpath holds only the carried lib — unloadable. install.sh no
# longer installs it; purge it off a box from an older install, and drop ffmpeg-sys-next's cache so
# the next cargo build relinks against the carried libraries.
CARGO_TD="$(dirname "$PREFIX")"
bust_ffmpeg_sys() {
    for _p in release debug; do
        _d="$CARGO_TD/$_p"
        [ -d "$_d" ] || continue
        rm -rf "$_d"/build/ffmpeg-sys-next-* "$_d"/.fingerprint/ffmpeg-sys-next-*
        rm -f  "$_d"/deps/libffmpeg_sys_next-* "$_d"/deps/ffmpeg_sys_next-*
    done
}
if distrobox enter "$BOX" -- bash -lc 'dpkg -l libavcodec-dev 2>/dev/null | grep -q "^ii"'; then
    log "Purging the box's libav*-dev — the host links its own carried FFmpeg"
    distrobox enter "$BOX" -- bash -lc 'sudo apt-get purge -y libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev libswscale-dev libavdevice-dev >/dev/null 2>&1' || true
    bust_ffmpeg_sys
fi

# One pin for the Deck and the .deb. Read it, never copy it: two numbers that must match and can
# drift apart is how the encode stack ends up behaving differently on one platform.
PINS="$SRC/ci/rust-ci-noble.Dockerfile"
[ -f "$PINS" ] || die "no $PINS to read the FFmpeg pin from"
pin() { sed -n "s/^ARG $1=//p" "$PINS" | head -1; }
FFMPEG_TAG="$(pin FFMPEG_TAG)"
FFMPEG_SHA="$(pin FFMPEG_SHA)"
NVHDR_TAG="$(pin NVHDR_TAG)"
NVHDR_SHA="$(pin NVHDR_SHA)"
for v in FFMPEG_TAG FFMPEG_SHA NVHDR_TAG NVHDR_SHA; do
    [ -n "${!v}" ] || die "$PINS carries no ARG $v"
done

if [ "$(cat "$PREFIX/.pf-tag" 2>/dev/null || true)" = "$FFMPEG_TAG" ]; then
    ok "FFmpeg $FFMPEG_TAG already built ($PREFIX)"
    exit 0
fi

log "Building FFmpeg $FFMPEG_TAG in '$BOX' (~4 min, once per pin)"
# A git tag is mutable and these .so's become part of the host, so assert the commit — the same
# check ci/rust-ci-noble.Dockerfile makes, for the same reason.
distrobox enter "$BOX" -- bash -lc "
set -e
export DEBIAN_FRONTEND=noninteractive
sudo apt-get install -y -qq --no-install-recommends nasm >/dev/null
rm -rf /tmp/pf-ffmpeg /tmp/pf-nvhdr '$PREFIX'
git clone -q --depth 1 --branch '$NVHDR_TAG' https://github.com/FFmpeg/nv-codec-headers.git /tmp/pf-nvhdr
[ \"\$(git -C /tmp/pf-nvhdr rev-parse HEAD)\" = '$NVHDR_SHA' ] || { echo 'nv-codec-headers $NVHDR_TAG moved upstream' >&2; exit 1; }
make -C /tmp/pf-nvhdr install PREFIX='$PREFIX' >/dev/null
git clone -q --depth 1 --branch '$FFMPEG_TAG' https://github.com/FFmpeg/FFmpeg.git /tmp/pf-ffmpeg
[ \"\$(git -C /tmp/pf-ffmpeg rev-parse HEAD)\" = '$FFMPEG_SHA' ] || { echo 'FFmpeg $FFMPEG_TAG moved upstream' >&2; exit 1; }
cd /tmp/pf-ffmpeg
PKG_CONFIG_PATH='$PREFIX/lib/pkgconfig' ./configure --prefix='$PREFIX' \
    --enable-shared --disable-static \
    --disable-doc --disable-programs --disable-debug \
    --enable-nvenc --enable-vaapi \
    --extra-cflags=-I'$PREFIX/include' --extra-ldflags=-L'$PREFIX/lib' >/dev/null
make -j\"\$(nproc)\" >/dev/null
make install >/dev/null
cd /
rm -rf /tmp/pf-ffmpeg /tmp/pf-nvhdr
" < /dev/null || die "the FFmpeg build failed — re-run with PUNKTFUNK_FFMPEG_PREFIX set to keep the tree"

[ -e "$PREFIX/lib/libavcodec.so" ] || die "the FFmpeg build produced no libavcodec in $PREFIX/lib"
printf '%s\n' "$FFMPEG_TAG" > "$PREFIX/.pf-tag"
ok "FFmpeg $FFMPEG_TAG: $PREFIX/lib ($(du -sh "$PREFIX/lib" | cut -f1))"

# The carried FFmpeg's soname just moved under an unchanged prefix; cargo keys ffmpeg-sys-next's
# link on the PKG_CONFIG_PATH string, not the .pc contents, so relink it against the new libraries.
bust_ffmpeg_sys
