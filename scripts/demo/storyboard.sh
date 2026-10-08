#!/usr/bin/env bash
# Plays the README demo against a running Mux window through muxctl: it types
# the commands and splits, zooms and switches tabs as the keys would, with the
# pauses the film needs. scripts/demo/film-macos.sh launches a separate Mux and
# films its window while this runs.
#
# Environment:
#   MUXCTL          muxctl binary (default: muxctl on PATH)
#   MUX_STATE_DIR   state dir of the Mux being filmed (never your own)
#   DEMO_SESSION    session to drive (default: main)
#   DEMO_EDITOR     editor for the first pane (default: nvim)
#   DEMO_TESTS      test command for the busy pane; a bell follows it
#   DEMO_TOP        what the second tab runs when there is no agent (default: htop)
#   DEMO_AGENT      an idle agent already started for the second tab's pane
#   DEMO_KEYS       a command that presses keys in the window: `chord ctrl+p a`
#                   presses each chord in turn and `type TEXT MS` types text.
#                   With it and DEMO_AGENT, the second tab opens its agent pane
#                   and asks it a question; the agent pane has no muxctl verb.
#   DEMO_CUES       file to append "<seconds>\t<label>" to for each key chord,
#                   timed from the start, for the key captions in the edit;
#                   labels starting with # are marks, not captions
#   DEMO_TYPE_MS    pause between typed characters (default: 34)
set -euo pipefail

muxctl=${MUXCTL:-muxctl}
session=${DEMO_SESSION:-main}
editor=${DEMO_EDITOR:-nvim}
tests=${DEMO_TESTS:-cargo test --lib -p mux-workspace -p mux-acp -p mux-terminal}
top=${DEMO_TOP:-htop}
type_ms=${DEMO_TYPE_MS:-34}
question='What is crates/mux-workspace/src/lib.rs for? One sentence.'

# macOS ships bash 3.2 and a date without %N, so time with perl.
now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }
start=$(now)

ctl() { "$muxctl" ${MUX_STATE_DIR:+--state-dir "$MUX_STATE_DIR"} "$@"; }
run() { ctl type "$session" --delay-ms "$type_ms" --enter "$1"; }
keys() { ctl type "$session" --delay-ms "${2:-90}" "$1"; }
cue() {
    if [[ -n ${DEMO_CUES:-} ]]; then
        perl -e 'printf "%.3f\t%s\n", $ARGV[0] - $ARGV[1], $ARGV[2]' "$(now)" "$start" "$1" >>"$DEMO_CUES"
    fi
}
# A chord: its caption shows a moment before the workspace changes.
chord() {
    cue "$1"
    sleep 0.25
    ctl do "$session" "$2"
}

# One pane on the ground: open the workspace model in the editor and page
# through it. The editor filling the pane is the edit's sync mark.
sleep 1.0
run "$editor crates/mux-workspace/src/lib.rs"
cue '#editor'
sleep 1.3
keys '}}}}}}' 130
sleep 0.5

# Split right: the history, in colour.
chord '⌃P R' split-right
sleep 0.7
run 'git log --oneline --graph --decorate -14'
sleep 1.4

# Split that pane down and set the tests going with a bell at the end, then
# go back to the editor: the ground stirs while the pane works, and the pane
# rings when it is done.
chord '⌃P D' split-down
sleep 0.7
run "$tests; tput bel"
sleep 0.3
chord '⌥ ←' focus-left
sleep 0.4
keys '}}}}' 130
sleep 2.2

if [[ -n ${DEMO_AGENT:-} && -n ${DEMO_KEYS:-} ]]; then
    # The second tab: turn its pane into the agent and ask it something.
    chord '⌃T 2' next-tab
    sleep 0.8
    cue '⌃P A'
    sleep 0.25
    "$DEMO_KEYS" chord ctrl+p a
    sleep 1.0
    "$DEMO_KEYS" type "$question" "$type_ms"
    sleep 0.4
    "$DEMO_KEYS" chord return
    ctl agent-wait "$DEMO_AGENT" --turns 1 --timeout-secs 20 || true
    sleep 2.0
    chord '⌃T 1' previous-tab
else
    # A new tab, its own ink, and back.
    chord '⌃T N' new-tab
    sleep 0.8
    run "$top"
    sleep 2.4
    chord '⌃T 1' previous-tab
fi
sleep 1.0

# Zoom the editor to fill the window, and back.
chord '⌃P F' zoom
sleep 1.4
chord '⌃P F' zoom
sleep 1.4
