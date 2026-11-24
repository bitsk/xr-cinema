#!/bin/sh
set -eu
export LC_ALL=C

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: sh package.sh OUTPUT.deb [XR_CINEMA_BINARY]" >&2
    exit 2
fi
output=$1
shift
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
chmod 0755 "$stage"
DESTDIR=$stage PREFIX=/usr sh "$root/packaging/install.sh" "$@"
binary=$stage/usr/bin/xr-cinema
header=$(readelf -hW "$binary")
case "$header" in
    *ELF64*AArch64*) arch=arm64 ;;
    *ELF64*X86-64*) arch=amd64 ;;
    *ELF64*RISC-V*) arch=riscv64 ;;
    *) echo "unsupported ELF architecture" >&2; exit 1 ;;
esac
glibc=$(readelf --version-info "$binary" | sed -n 's/.*Name: GLIBC_\([0-9.]*\) .*/\1/p' | sort -V | tail -1)
test -n "$glibc"
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -1)
test -n "$version"
install -d "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<EOF
Package: xr-cinema
Version: $version
Architecture: $arch
Maintainer: XR Cinema contributors
Section: video
Priority: optional
Depends: python3 (>= 3.9), libc6 (>= $glibc), libgcc-s1, libvulkan1, libopenxr-loader1, libcairo2, libpango-1.0-0, libpangocairo-1.0-0, libglib2.0-0t64 | libglib2.0-0, libgstreamer1.0-0, libgstreamer-plugins-base1.0-0, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-libav
Recommends: gstreamer1.0-plugins-bad, gstreamer1.0-pulseaudio | gstreamer1.0-alsa, fonts-noto-cjk
Description: Spatial video player for openKylin XR
 Play mono, stereo and equirectangular videos in a virtual cinema using
 Vulkan and an active OpenXR runtime. Control playback with tracked
 controllers or the local control socket.
EOF
dpkg-deb --root-owner-group --build "$stage" "$output"
