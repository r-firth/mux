#!/bin/sh
set -eu

version=0.16.0
architecture=${1:-$(uname -m)}
destination=${2:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/mux-zig-$version}

case "$architecture" in
  arm64 | aarch64)
    archive_arch=aarch64
    checksum=b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489
    ;;
  x86_64 | amd64)
    archive_arch=x86_64
    checksum=0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7
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
