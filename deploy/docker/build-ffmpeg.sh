#!/bin/sh
# Minimal ffmpeg for the media ingest decoder (media-ingest.md D160):
# pipe:0 in, 16 kHz mono s16le out on pipe:1. Only the demuxers and audio
# decoders listed by capture/decoder.rs are built: no network protocol, no
# file protocol, no video, no external library. No x86 assembly (no nasm
# needed): decoding audio costs a fraction of one core either way.
#
# License: built without --enable-gpl / --enable-version3 / --enable-nonfree,
# so the binary is LGPL-2.1-or-later (checked below from config.h). It runs
# as a separate, unmodified program; the notice and the license text are
# installed next to it.
#
#   build-ffmpeg.sh <ffmpeg-X.Y.Z.tar.xz> <sha256> <out-prefix> [aarch64]
set -eu

tarball=$1
sha=$2
prefix=$3
arch=${4:-}

echo "$sha  $tarball" | sha256sum -c -
work=$(mktemp -d)
tar -xJf "$tarball" -C "$work" --strip-components=1
cd "$work"

cross=""
if [ "$arch" = aarch64 ]; then
  cross="--enable-cross-compile --arch=aarch64 --target-os=linux --cross-prefix=aarch64-linux-gnu-"
fi

# shellcheck disable=SC2086
./configure $cross \
  --prefix="$prefix" \
  --disable-everything --disable-autodetect --disable-network \
  --disable-doc --disable-debug --disable-ffplay --disable-ffprobe \
  --disable-x86asm \
  --disable-avdevice --disable-swscale \
  --enable-ffmpeg --enable-avcodec --enable-avformat --enable-avfilter \
  --enable-swresample \
  --enable-protocol=pipe \
  --enable-demuxer=mp3,aac,ogg,flac,wav,mpegts,mov \
  --enable-parser=aac,mpegaudio,opus,vorbis,flac \
  --enable-decoder=mp3float,mp3,mp2float,mp2,aac,aac_fixed,vorbis,opus,flac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8 \
  --enable-encoder=pcm_s16le \
  --enable-muxer=pcm_s16le \
  --enable-filter=aresample,aformat,anull

grep -q '#define FFMPEG_LICENSE "LGPL version 2.1 or later"' config.h \
  || { echo "ffmpeg is not LGPL-2.1-or-later" >&2; exit 1; }

make -j"$(nproc)"
mkdir -p "$prefix/bin" "$prefix/share/licenses/ffmpeg"
cp ffmpeg "$prefix/bin/"
cp COPYING.LGPLv2.1 LICENSE.md "$prefix/share/licenses/ffmpeg/"
cat > "$prefix/share/licenses/ffmpeg/NOTICE" <<EOF
This image ships FFmpeg (https://ffmpeg.org), licensed under the GNU Lesser
General Public License version 2.1 or later (COPYING.LGPLv2.1), unmodified,
as the separate program /usr/local/bin/ffmpeg.

Source: https://ffmpeg.org/releases/$(basename "$tarball")
sha256: $sha
Configuration: $(sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"$/\1/p' config.h)
EOF
cd /
rm -rf "$work"
