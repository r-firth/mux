#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: $0 ARCHIVE EXPECTED_ARCH EXPECTED_VERSION" >&2
  exit 2
fi

archive=$1
expected_architecture=$2
expected_version=$3
checksum_file="$archive.sha256"

fail() {
  echo "package verification failed: $*" >&2
  exit 1
}

[ -f "$archive" ] || fail "archive does not exist: $archive"
[ -f "$checksum_file" ] || fail "checksum does not exist: $checksum_file"

archive_directory=$(CDPATH= cd -- "$(dirname -- "$archive")" && pwd)
archive_name=$(basename -- "$archive")
(cd "$archive_directory" && shasum -a 256 -c "$(basename -- "$checksum_file")")

verification_directory=$(mktemp -d "${TMPDIR:-/tmp}/mux-package-verify.XXXXXX")
cleanup() {
  if [ -d "$verification_directory" ]; then
    rm -rf -- "$verification_directory"
  fi
}
trap cleanup EXIT HUP INT TERM

ditto -x -k "$archive_directory/$archive_name" "$verification_directory"
app="$verification_directory/Mux.app"
executable="$app/Contents/MacOS/mux"
ghostty="$app/Contents/Frameworks/libghostty-vt.dylib"
plist="$app/Contents/Info.plist"

[ -x "$executable" ] || fail "bundle executable is missing"
[ -f "$ghostty" ] || fail "bundled libghostty-vt is missing"
[ -f "$plist" ] || fail "Info.plist is missing"

executable_description=$(file "$executable")
ghostty_description=$(file "$ghostty")
case "$executable_description" in
  *"Mach-O 64-bit executable $expected_architecture"*) ;;
  *) fail "Mux architecture mismatch: $executable_description" ;;
esac
case "$ghostty_description" in
  *"Mach-O 64-bit dynamically linked shared library $expected_architecture"*) ;;
  *) fail "libghostty architecture mismatch: $ghostty_description" ;;
esac

bundle_identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")
bundle_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")
build_number=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$plist")
minimum_macos=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$plist")
[ "$bundle_identifier" = "io.mux.Mux" ] || fail "unexpected bundle identifier: $bundle_identifier"
[ "$bundle_version" = "$expected_version" ] || fail "expected version $expected_version, found $bundle_version"
[ "$minimum_macos" = "13.0" ] || fail "unexpected minimum macOS version: $minimum_macos"
case "$build_number" in
  '' | *[!0-9]*) fail "bundle build number is not numeric: $build_number" ;;
esac

otool -L "$executable" | grep -Fq '@rpath/libghostty-vt.dylib' ||
  fail "Mux does not link the bundled libghostty through @rpath"
otool -l "$executable" | grep -Fq '@executable_path/../Frameworks' ||
  fail "Mux does not carry the app Frameworks runtime search path"
codesign --verify --deep --strict --verbose=2 "$app"

echo "verified $archive_name ($expected_architecture, Mux $expected_version, build $build_number)"
