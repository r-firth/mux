# Zellij default keybindings

Mux implements the Zellij default bindings for the pane, resize, tab, and
session features it currently exposes. Bindings resolve to product actions;
the GUI and daemon do not hardcode key behavior into workspace mutations.

## Shared

| Keys | Action |
| --- | --- |
| `Ctrl+p` | Enter Pane mode; press again to return to Normal |
| `Ctrl+t` | Enter Tab mode; press again to return to Normal |
| `Option+h` / `Option+Left` | Focus left pane, or previous tab at the edge |
| `Option+l` / `Option+Right` | Focus right pane, or next tab at the edge |
| `Option+j` / `Option+Down` | Focus pane below |
| `Option+k` / `Option+Up` | Focus pane above |
| `Ctrl+p`, `a` | Turn the focused pane into its native ACP agent surface |

`Alt` is the same modifier as `Option` on macOS. Shared bindings remain active
inside the supported modes, matching Zellij. Enter or Escape returns from a
mode to normal terminal input. All other Normal-mode keys, including
`Ctrl+n`, `Ctrl+o`, and `:`, pass through to the foreground program.

## Pane mode

| Keys | Action |
| --- | --- |
| `h/j/k/l` or arrows | Focus a neighboring pane |
| `n` | Create a pane and return to Normal |
| `d` | Split downward and return to Normal |
| `r` | Split to the right and return to Normal |
| `Ctrl+n` | Enter Resize mode (Normal-mode `Ctrl+n` still reaches the terminal) |
| `f` | Toggle focused-pane zoom and return to Normal |
| `x` | Close the focused pane and return to Normal |
| `a` | Open the agent pane and return to Normal |

## Resize mode

Enter with `Ctrl+p`, then `Ctrl+n`. Use `h/j/k/l` or the arrow keys to move
the nearest boundary in that direction. Enter or Escape returns to Normal.
This nested prefix preserves Zellij's resize muscle memory without stealing
Normal-mode `Ctrl+n` from Vim and other terminal applications.

## Tab mode

| Keys | Action |
| --- | --- |
| `h` / `k` / Left / Up | Previous tab |
| `l` / `j` / Right / Down | Next tab |
| `1` through `9` | Select a numbered tab and return to Normal |
| `n` | Create a tab and return to Normal |
| `r` | Rename the active tab in its chip; Return keeps it, Escape puts it back |
| `c` | Give the active tab the next ink |
| `x` | Close the active tab and return to Normal |

Clearing a tab's name hands it back to whatever its shell shows.

Zellij's `Ctrl+o` session prefix intentionally has no Normal-mode binding yet.
This keeps foreground Control-key input untouched except for `Ctrl+p` and
`Ctrl+t`.

## Scrollback

The wheel or trackpad moves through a pane's history; a thin thumb on the
pane's right edge shows where while it is scrolled back. Typing returns the
pane to its latest output.

A pane keeps as much history as your Ghostty does: the `scrollback-limit` in
its config, or Ghostty's own 10 MB, which is several thousand rows (more in a
narrow pane). Tab switches, splits and closes leave it alone, even while the
pane is printing. The daemon keeps only the last thousand rows or so, so that
is what a pane starts with when Mux opens or moves to another session.

| Keys | Action |
| --- | --- |
| `Cmd+Home` / `Cmd+End` | Go to the top or bottom of the history |
| `Cmd+PageUp` / `Cmd+PageDown` | Move a page through the history |
| `Shift+PageUp` / `Shift+PageDown` | The same, unless the program has no history (a full-screen program keeps them) |

## Find

`Cmd+F` opens find in the focused pane's head, so it takes no rows from the
terminal. Every place the text appears in the pane, its history included, is
marked: the current one solid in the tab's ink, the rest washed in it, with a
tick for each on the pane's right edge to show where in the history it is.
The search starts from the latest output and works up. Lowercase text finds
either case; a capital makes the search exact. New output is searched as it
arrives.

| Keys | Action |
| --- | --- |
| Return, Up or `Cmd+G` | The next place up, in older output |
| Shift+Return, Down or `Cmd+Shift+G` | The next place down, in newer output |
| Escape | Close find, leaving the pane where it is |
| `Cmd+F` | Back to the field with its text selected, to type over it |

## Links

Hold `Cmd` over a link in a pane's output, an OSC 8 hyperlink or a URL
written out in the text (even one wrapped across rows), to underline it and
see where it goes in the strip. `Cmd`-click opens it. File links are shown in
Finder rather than opened.

## Panes that call

A pane out of sight that rings the bell, or sends a desktop notification
(OSC 9 or OSC 777, as Claude Code and Codex do when they want you), is marked
until it is looked at. Its tab's dot turns peach with what it said beside the
name, and in the tab on screen the pane's head says it. While Mux is in the
background, the first call also bounces its Dock icon.

## Exited panes

A shell that exits cleanly in the pane being typed in closes that pane, or its
tab when it was the tab's only pane. Any other exit, such as a failure or a
pane that ended while another had the keys, stays on screen with a rule that
says how it ended. While it has the focus, these keys act on it:

| Keys | Action |
| --- | --- |
| Return | Close the pane (its tab, when it is the tab's only pane) |
| `n` | Start a new shell in its place, keeping the tab and the region |

The session's last pane is never closed; Return starts a new shell there.

## Go to

`Cmd+P` (or `Cmd+Shift+P`) opens a palette over the workspace. With nothing
typed it is a map of the session: each tab with its number, the panes of a
split tab under it with what each is running, the other sessions, then each
command beside the keys that do it. It opens on the tab you were in before
this one, so `Cmd+P` then Return goes back.

Typing narrows the list to rows with those letters in order: `sr` finds split
right and `cw` finds the pane running `cargo watch`. A word also finds a pane
by its tab's name, and a tab by what its panes are running. A number goes to
that tab. Agents are listed once you type one's name: `codex` opens Codex in
the pane with the keys, or shows the Codex session already in the tab.

| Keys | Action |
| --- | --- |
| Up / Down, `Ctrl+p` / `Ctrl+n`, Tab / Shift+Tab | Choose a row |
| Return | Go there |
| Escape or `Cmd+P` | Close the palette |

## Sessions sheet

Click the session's name at the right of the strip, or press `Cmd+Shift+S`, to
drop the sessions sheet from it.

| Keys | Action |
| --- | --- |
| `j` / `k` or Up / Down | Choose a session |
| Return | Open the chosen session |
| `n` | Start a session in the focused pane's directory |
| `r` | Rename the chosen session in its row; Return saves, Escape cancels |
| `x` | Ask to end the chosen session; `y` or Return ends it, `n` or Escape keeps it |
| Escape or `Cmd+Shift+S` | Close the sheet |

## Settings sheet

`Cmd+,` opens settings: the agents `Ctrl+a` offers, each on or off.

| Keys | Action |
| --- | --- |
| `j` / `k` or Up / Down | Choose an agent |
| Space or Return | Turn it on or off; the change is saved at once |
| `o` | Show `settings.json` in Finder |
| Escape or `Cmd+,` | Close the sheet |

The sheet reads `settings.json` each time it opens, so agents added by hand
under `agent_servers` appear without a restart. While the file doesn't parse,
the sheet says so and saves nothing.

## Agent surface

The focused pane becomes a native, tab-local agent surface; it is not a fixed
sidebar and the PTY behind it remains alive. The same pane-navigation model is
used throughout.

| Keys | Action |
| --- | --- |
| `Ctrl+a` | Return this pane to its terminal without ending the agent |
| Return | Send the draft or accept the selected completion |
| Shift+Return | Insert a newline |
| `/` | Complete Mux commands and live commands advertised by the ACP agent |
| `@` | Complete a file from the focused pane's working directory |
| Up / Down | Move through an open completion menu |
| Tab | Accept the selected completion |
| Escape | Close completion first; otherwise cancel the active agent turn |
| Option+Left / Right | Move through tab-local agent sessions, then panes/tabs at the edge |
| Option+Up / Down | Focus the terminal pane above or below |

`/new [agent] [cwd]` starts another session, `/end` ends the selected session
and removes it from the session picker, and `/help` renders the full local and
agent-advertised command reference in the conversation. Native controls remain
an optional mouse path.
