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
daemon_pid=
cleanup() {
  if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
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
app_icon="$app/Contents/Resources/AppIcon.icns"
asset_catalog="$app/Contents/Resources/Assets.car"

[ -x "$executable" ] || fail "bundle executable is missing"
[ -f "$ghostty" ] || fail "bundled libghostty-vt is missing"
[ -f "$plist" ] || fail "Info.plist is missing"
[ -f "$app_icon" ] || fail "app icon is missing"
[ -f "$asset_catalog" ] || fail "asset catalog is missing"

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
bundle_icon_file=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$plist")
bundle_icon_name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconName' "$plist")
[ "$bundle_identifier" = "io.mux.Mux" ] || fail "unexpected bundle identifier: $bundle_identifier"
[ "$bundle_version" = "$expected_version" ] || fail "expected version $expected_version, found $bundle_version"
[ "$minimum_macos" = "13.0" ] || fail "unexpected minimum macOS version: $minimum_macos"
[ "$bundle_icon_file" = "AppIcon" ] || fail "unexpected icon file: $bundle_icon_file"
[ "$bundle_icon_name" = "AppIcon" ] || fail "unexpected icon name: $bundle_icon_name"
case "$build_number" in
  '' | *[!0-9]*) fail "bundle build number is not numeric: $build_number" ;;
esac

otool -L "$executable" | grep -Fq '@rpath/libghostty-vt.dylib' ||
  fail "Mux does not link the bundled libghostty through @rpath"
otool -l "$executable" | grep -Fq '@executable_path/../Frameworks' ||
  fail "Mux does not carry the app Frameworks runtime search path"
codesign --verify --deep --strict --verbose=2 "$app"

if [ -n "${MUXCTL:-}" ]; then
  [ -x "$MUXCTL" ] || fail "MUXCTL is not executable: $MUXCTL"
  state_directory="$verification_directory/state"
  daemon_log="$verification_directory/daemon.log"
  "$executable" --daemon --state-dir "$state_directory" >"$daemon_log" 2>&1 &
  daemon_pid=$!

  ready=false
  attempts=0
  while [ "$attempts" -lt 50 ]; do
    if "$MUXCTL" --state-dir "$state_directory" health >/dev/null 2>&1; then
      ready=true
      break
    fi
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
      cat "$daemon_log" >&2
      fail "packaged daemon exited during startup"
    fi
    attempts=$((attempts + 1))
    sleep 0.1
  done
  [ "$ready" = true ] || {
    cat "$daemon_log" >&2
    fail "packaged daemon did not become healthy"
  }

  session=$(
    "$MUXCTL" --state-dir "$state_directory" new \
      --name package-smoke --panes 2 --program /bin/sh
  )
  session_id=$(printf '%s\n' "$session" | awk 'NR == 1 { print $1 }')
  [ -n "$session_id" ] || fail "runtime smoke did not return a session id"
  {
    printf '%s\n' "printf 'MUX_PACKAGE_SMOKE\\n'"
    sleep 0.5
  } |
    "$MUXCTL" --state-dir "$state_directory" attach "$session_id" \
      >"$verification_directory/attach.log"
  grep -Fq 'MUX_PACKAGE_SMOKE' "$verification_directory/attach.log" || {
    cat "$verification_directory/attach.log" >&2
    fail "packaged PTY did not return expected output"
  }
  "$MUXCTL" --state-dir "$state_directory" inspect "$session_id" >/dev/null
  "$MUXCTL" --state-dir "$state_directory" kill "$session_id" >/dev/null
fi

echo "verified $archive_name ($expected_architecture, Mux $expected_version, build $build_number)"
