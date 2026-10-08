#!/usr/bin/env bash
# Films the README demo on a Mac. Launches a separate Mux on a throwaway state
# dir, films its window while scripts/demo/storyboard.sh drives it, and leaves
# the footage with its cue sheet for scripts/demo/edit.py.
#
# Usage: scripts/demo/film-macos.sh [OUT_DIR]     (default: target/demo)
#
# Writes OUT_DIR/footage.mov (the window at full Retina resolution, no
# pointer), OUT_DIR/cues.tsv (when each key chord was pressed) and
# OUT_DIR/timing.tsv (when filming and the storyboard started).
#
# Needs Screen Recording permission for the terminal that runs it, a built
# Mux.app (MUX_APP, default /Applications/Mux.app) whose protocol matches this
# checkout, Xcode's command line tools, and nvim, git and cargo on the shell's
# PATH. With Accessibility permission too (macOS offers it on the first run),
# the second tab becomes a Claude Code agent pane and answers a question (set
# DEMO_NO_AGENT=1 to skip it); without it, the second tab runs htop. Your own Mux and its daemon are never
# touched: the filmed Mux runs its own daemon in the throwaway state dir, and
# both stop when filming ends. Keep other windows off the middle of the main
# display while it films.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
out=${1:-$repo/target/demo}
app=${MUX_APP:-/Applications/Mux.app}
tests=${DEMO_TESTS:-cargo test --lib -p mux-workspace -p mux-acp -p mux-terminal}
mkdir -p "$out"
out=$(cd "$out" && pwd)
rm -f "$out/footage.mov" "$out/cues.tsv" "$out/timing.tsv"

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
wait_for() { # SECONDS COMMAND...
    local deadline=$(($(date +%s) + $1))
    shift
    until "$@" >/dev/null 2>&1; do
        (($(date +%s) < deadline)) || return 1
        sleep 0.3
    done
}

echo "building muxctl"
cargo build --quiet --release -p muxctl --manifest-path "$repo/Cargo.toml"
muxctl=$repo/target/release/muxctl

state=$(mktemp -d /tmp/mux-demo.XXXXXX)
ctl() { "$muxctl" --state-dir "$state" "$@"; }

keys=
if [[ -z ${DEMO_NO_AGENT:-} ]]; then
    if ! swiftc -O "$repo/scripts/demo/keys.swift" -o "$state/keys" 2>/dev/null; then
        echo "swiftc could not build the key presser, so no agent pane in this take"
    elif "$state/keys" trusted --prompt || {
        echo "waiting up to three minutes for Accessibility permission (System Settings, Privacy & Security)"
        wait_for 180 "$state/keys" trusted
    }; then
        keys=$state/keys
    else
        echo "no Accessibility permission for this terminal, so no agent pane in this take"
    fi
fi

MUX_STATE_DIR=$state "$app/Contents/MacOS/mux" --state-dir "$state" >"$out/app.log" 2>&1 &
gui=$!
capture=
cleanup() {
    if [[ -n $capture ]]; then kill -INT "$capture" 2>/dev/null || true; fi
    kill "$gui" 2>/dev/null || true
    # The filmed Mux's daemon, found by its own state dir; nothing else.
    pkill -f -- "--state-dir $state" 2>/dev/null || true
    rm -rf "$state"
}
trap cleanup EXIT

wait_for 30 ctl list || { echo "the demo Mux never started its daemon (see $out/app.log)" >&2; exit 1; }

# The window's bounds in points, and the pointer parked just below it.
cat >"$state/window.swift" <<'SWIFT'
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
for window in windows {
    guard (window[kCGWindowOwnerPID as String] as? Int32) == pid,
          (window[kCGWindowLayer as String] as? Int) == 0,
          let bounds = window[kCGWindowBounds as String] as? NSDictionary,
          let rect = CGRect(dictionaryRepresentation: bounds),
          rect.width > 200 else { continue }
    CGWarpMouseCursorPosition(CGPoint(x: rect.midX, y: rect.maxY + 24))
    print(Int(rect.minX), Int(rect.minY), Int(rect.width), Int(rect.height))
    exit(0)
}
exit(1)
SWIFT
wait_for 30 swift "$state/window.swift" "$gui" || { echo "the demo window never appeared" >&2; exit 1; }

# Warm the test build in the pane's own shell, so the filmed run is quick.
echo "warming the test build"
ctl type main --enter "cd '$repo' && $tests >/dev/null 2>&1; touch '$state/warm'; clear"
wait_for 900 test -e "$state/warm" || { echo "the warm-up run never finished" >&2; exit 1; }

# The agent's tab: start Claude Code for its pane now, so the take shows the
# answer rather than the agent starting up.
agent=
if [[ -n $keys ]]; then
    echo "starting the agent"
    ctl do main new-tab
    ctl rename-tab main claude
    agent=$(ctl agent-pane main --cwd "$repo")
    if ! ctl agent-wait "$agent" --timeout-secs 180; then
        echo "the agent never became ready, so no agent pane in this take"
        agent=
    fi
    ctl do main previous-tab
    "$keys" activate "$gui" || true
fi
sleep 1.5

read -r x y w h < <(swift "$state/window.swift" "$gui")
echo "filming the window at $x,$y ${w}x$h"
record=$(now)
screencapture -v -x -V 90 -R "$x,$y,$w,$h" "$out/footage.mov" &
capture=$!
sleep 2
story=$(now)
DEMO_CUES=$out/cues.tsv MUXCTL=$muxctl MUX_STATE_DIR=$state DEMO_TESTS=$tests \
    DEMO_AGENT=$agent DEMO_KEYS=$keys "$repo/scripts/demo/storyboard.sh"
finish=$(now)
printf 'record\t%s\nstory\t%s\nfinish\t%s\n' "$record" "$story" "$finish" >"$out/timing.tsv"
sleep 1
# Control-C ends a screen recording and saves it.
kill -INT "$capture"
wait "$capture" || true
capture=
[[ -s $out/footage.mov ]] || { echo "screencapture saved no footage" >&2; exit 1; }
echo "footage: $out/footage.mov ($(du -h "$out/footage.mov" | cut -f1))"
