#!/bin/sh
set -eu

version="3.5.0"
archive_sha256="0227b220360a6f819b9ead92343e8112b34733054782561af50cfba1e8afab63"
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
destination="$project_dir/apps/mux/assets/fonts"
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

archive="$temporary/JetBrainsMono.tar.xz"
curl -fsSL \
  "https://github.com/ryanoasis/nerd-fonts/releases/download/v${version}/JetBrainsMono.tar.xz" \
  -o "$archive"

actual_sha256=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
if [ "$actual_sha256" != "$archive_sha256" ]; then
  echo "JetBrains Mono Nerd Font archive checksum mismatch" >&2
  exit 1
fi

mkdir -p "$destination"
tar -xJf "$archive" -C "$destination" \
  JetBrainsMonoNerdFontMono-Regular.ttf \
  JetBrainsMonoNerdFontMono-Bold.ttf \
  JetBrainsMonoNerdFontMono-Italic.ttf \
  JetBrainsMonoNerdFontMono-BoldItalic.ttf \
  OFL.txt
