#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
architecture=${MUX_ARCHITECTURE:-$(uname -m)}
version=$(cargo pkgid --manifest-path "$project_dir/Cargo.toml" -p mux)
version=${version##*#}
version=${version##*@}
archive_name=${MUX_ARCHIVE_NAME:-Mux-$version-macos-$architecture}
distribution_dir=${MUX_DISTRIBUTION_DIR:-$project_dir/dist}

app_path=$("$project_dir/scripts/bundle-macos.sh")
mkdir -p "$distribution_dir"
archive_path="$distribution_dir/$archive_name.zip"
checksum_path="$archive_path.sha256"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$archive_path"
(cd "$distribution_dir" && shasum -a 256 "$archive_name.zip" > "$archive_name.zip.sha256")

echo "$archive_path"
echo "$checksum_path"
