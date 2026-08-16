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
resources_dir="$contents_dir/Resources"
mkdir -p "$macos_dir" "$frameworks_dir" "$resources_dir"
cp "$project_dir/packaging/macos/Info.plist" "$contents_dir/Info.plist"
cp "$target_profile_dir/mux" "$macos_dir/mux"

asset_catalog_info="$resources_dir/AssetCatalogInfo.plist"
xcrun actool \
  --compile "$resources_dir" \
  --platform macosx \
  --minimum-deployment-target 13.0 \
  --target-device mac \
  --app-icon AppIcon \
  --standalone-icon-behavior all \
  --output-partial-info-plist "$asset_catalog_info" \
  "$project_dir/packaging/macos/Assets.xcassets" >/dev/null
rm -f "$asset_catalog_info"

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

# Generated bundles should not inherit quarantine, Finder metadata, or resource
# forks from a checkout or cached native dependency. Those attributes make
# strict code-signature validation fail on some machines.
xattr -cr "$app_dir"

sign_code() {
  code_path=$1
  if [ "$codesign_identity" = - ]; then
    # Apple Silicon requires code to carry a signature, but local builds do
    # not need a certificate or Hardened Runtime. Keeping this signature
    # purely ad hoc avoids imposing distribution-only library validation on
    # an app someone has just built from source on their own Mac.
    codesign --force --sign - "$code_path"
  else
    # A real distribution identity should use Apple's notarization-compatible
    # signing shape: Hardened Runtime plus a trusted timestamp.
    codesign --force --options runtime --timestamp --sign "$codesign_identity" "$code_path"
  fi
}

# Sign nested code first, then seal the outer bundle. Using --deep while
# signing can accidentally apply the app's options to every nested code item.
sign_code "$frameworks_dir/libghostty-vt.dylib"
sign_code "$app_dir"

echo "$app_dir"
