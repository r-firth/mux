#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
profile=${MUX_BUILD_PROFILE:-release}
zig=${MUX_ZIG:-zig}
codesign_identity=${MUX_CODESIGN_IDENTITY:--}
build_number=${MUX_BUILD_NUMBER:-1}
bundle_identifier=${MUX_BUNDLE_IDENTIFIER:-io.mux.Mux}
bundle_name=${MUX_BUNDLE_NAME:-Mux}
default_state_application=${MUX_DEFAULT_STATE_APPLICATION:-}
if [ -z "$default_state_application" ] && [ "$bundle_identifier" != "io.mux.Mux" ]; then
  default_state_application=${bundle_identifier##*.}
fi

case "$profile" in
  debug) cargo_profile_args="" ;;
  release) cargo_profile_args="--release" ;;
  *) echo "unsupported MUX_BUILD_PROFILE: $profile" >&2; exit 2 ;;
esac

cd "$project_dir"
if [ -n "$default_state_application" ]; then
  MUX_DEFAULT_STATE_APPLICATION="$default_state_application" \
    MUX_ZIG="$zig" MACOSX_DEPLOYMENT_TARGET=13.0 \
    cargo build -p mux --features product $cargo_profile_args
else
  MUX_ZIG="$zig" MACOSX_DEPLOYMENT_TARGET=13.0 \
    cargo build -p mux --features product $cargo_profile_args
fi

target_profile_dir="$project_dir/target/$profile"
if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
  target_profile_dir="$project_dir/target/$CARGO_BUILD_TARGET/$profile"
fi

app_dir=${MUX_APP_PATH:-$project_dir/target/Mux.app}
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
frameworks_dir="$contents_dir/Frameworks"
mkdir -p "$macos_dir" "$frameworks_dir"
cp "$project_dir/packaging/macos/Info.plist" "$contents_dir/Info.plist"
cp "$target_profile_dir/mux" "$macos_dir/mux"

package_id=$(cargo pkgid -p mux)
version=${package_id##*#}
version=${version##*@}
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" \
  "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" \
  "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $bundle_identifier" \
  "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $bundle_name" \
  "$contents_dir/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $bundle_name" \
  "$contents_dir/Info.plist"

ghostty_library=$(find "$target_profile_dir/build" \
  -path '*/out/ghostty/lib/libghostty-vt.dylib' -type f -print | head -n 1)
if [ -z "$ghostty_library" ]; then
  echo "vendored libghostty-vt was not produced" >&2
  exit 1
fi
cp "$ghostty_library" "$frameworks_dir/libghostty-vt.dylib"
install_name_tool -add_rpath '@executable_path/../Frameworks' "$macos_dir/mux" 2>/dev/null || true
codesign --force --deep --options runtime --sign "$codesign_identity" "$app_dir"

echo "$app_dir"
