#!/bin/sh
set -eu

version=0.17.0
architecture=${1:-$(uname -m)}
destination=${2:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/mux-zig-$version}

case "$architecture" in
  arm64 | aarch64)
    archive_arch=aarch64
    checksum=b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a
    ;;
  x86_64 | amd64)
    archive_arch=x86_64
    checksum=4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6
    ;;
  *)
    echo "unsupported macOS architecture: $architecture" >&2
    exit 2
    ;;
esac

zig_dir="$destination/zig-$archive_arch-macos-$version"
if [ -x "$zig_dir/zig" ]; then
  echo "$zig_dir/zig"
  exit 0
fi

mkdir -p "$destination"
archive="$destination/zig-$archive_arch-macos-$version.tar.xz"
url="https://ziglang.org/download/$version/zig-$archive_arch-macos-$version.tar.xz"
curl --fail --location --silent --show-error "$url" --output "$archive"
actual=$(shasum -a 256 "$archive" | awk '{print $1}')
if [ "$actual" != "$checksum" ]; then
  echo "Zig archive checksum mismatch" >&2
  exit 1
fi
tar -C "$destination" -xf "$archive"
echo "$zig_dir/zig"
