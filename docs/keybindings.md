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

Option composes characters, `Option+e` an accent and `Option+b` a `∫`, unless
your Ghostty config sets `macos-option-as-alt`: then Option is Meta, so
`Option+b` and `Option+f` move a shell back and forward a word, as in
Ghostty. Mux can't tell the two Option keys apart, so `left` and `right` both
turn it on for either key.

## Mac shortcuts

The keys a Mac terminal answers to work beside Zellij's, from any mode and
from an agent pane too. They are in the menu bar's Shell and Window menus.

| Keys | Action |
| --- | --- |
| `Cmd+T` | Create a tab |
| `Cmd+Shift+T` | Reopen the tab closed last, its splits and names, with a fresh shell in each pane's directory |
| `Cmd+D` | Split to the right |
| `Cmd+Shift+D` | Split downward |
| `Cmd+Shift+Return` | Toggle focused-pane zoom |
| `Cmd+W` | Close the focused pane, or its tab when it is the tab's only pane |
| `Cmd+A` | Select everything in the focused pane, its history too |
| `Cmd+Left` / `Cmd+Right` | Go to the start or end of the line being typed (sends `Ctrl+a` / `Ctrl+e`, as Ghostty does) |
| `Cmd+Backspace` | Delete back to the start of the line (sends `Ctrl+u`) |
| `Cmd+K` | Clear the focused pane: its screen and history at the prompt, only its history with a program in front |
| `Cmd+1` through `Cmd+8` | Select a numbered tab |
| `Cmd+9` | Select the last tab |
| `Cmd+Shift+]` / `Ctrl+Tab` | Next tab |
| `Cmd+Shift+[` / `Ctrl+Shift+Tab` | Previous tab |
| `Cmd+]` / `Cmd+[` | Focus the next or previous pane, coming round at the ends |
| `Cmd+Option+arrows` | Focus the pane in that direction |
| `Cmd+Control+arrows` | Move the focused pane's edge in that direction |
| `Cmd+Control+=` | Share the tab's room evenly between its panes |
| `Cmd+M` | Minimize the window |
| `Cmd+Control+F` | Enter or leave full screen |
| `Cmd+H` / `Cmd+Option+H` | Hide Mux, or every other app |

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
| `c` | Name the focused pane in its head; Return keeps it, Escape puts it back |

A named pane's head shows its name, `server` or `logs`, with what its shell
says beside it, and a tab without a name of its own goes by its focused
pane's name. The name stays with the pane when it moves to another tab and
after a restart. Clearing it hands the head back to the shell. Double-click
a pane's head to name it with the mouse.

## Resize mode

Enter with `Ctrl+p`, then `Ctrl+n`. Use `h/j/k/l` or the arrow keys to move
the nearest boundary in that direction, and `=` to share the tab's room
evenly between its panes, as `Cmd+Control+=` does from anywhere, so three
panes side by side get a third each. Enter or Escape returns to Normal.
This nested prefix preserves Zellij's resize muscle memory without stealing
Normal-mode `Ctrl+n` from Vim and other terminal applications.

The gaps between panes resize them too: drag one, and each pane's head shows
its size as it goes. The middle holds a gap for a moment on the way past, and
a double click evens the split out.

## Tab mode

| Keys | Action |
| --- | --- |
| `h` / `k` / Left / Up | Previous tab |
| `l` / `j` / Right / Down | Next tab |
| `1` through `9` | Select a numbered tab and return to Normal |
| `n` | Create a tab and return to Normal |
| `r` | Rename the active tab in its chip; Return keeps it, Escape puts it back |
| `c` | Give the active tab the next ink; it keeps it when moved and after a restart |
| `Shift+h` / `Shift+Left` | Move the active tab one place left |
| `Shift+l` / `Shift+Right` | Move the active tab one place right |
| `b` | Move the focused pane to a tab of its own, next to this one |
| `s` | Type into every pane of the tab at once, or stop, and return to Normal |
| `x` | Close the active tab and return to Normal |

While a tab is synced, each key and paste goes to every shell in it, each
pane's head says `sync`, and their edges take the tab's ink. An agent pane,
or one whose shell has exited, is left out.

Clearing a tab's name hands it back to whatever its shell shows. A moved
tab keeps its name; a numbered tab takes the number of its new place.
A tab without a name of its own is named for the program running in its
focused pane, `htop` or `npm`, while one runs, even under a shell that
leaves the title alone, and the pane's head says it beside the directory.
With the mouse, drag a tab's chip along the strip to move it, and
double-click the chip of the tab on screen to rename it. A chip's dot turns
into a close mark under the pointer; click it, or middle-click the chip, to
close that tab without leaving the one on screen. A tab with something
running asks first. Right-click a chip for a menu of what Tab mode does to it,
with close other tabs besides.

Zellij's `Ctrl+o` session prefix intentionally has no Normal-mode binding yet.
This keeps foreground Control-key input untouched except for `Ctrl+p` and
`Ctrl+t`.

## Cursor

A pane shows the cursor your Ghostty does: `cursor-style` in its config
(`block`, `bar`, `underline` or `block_hollow`), a block by default. A program
that picks its own cursor, as Vim does in insert mode, still gets it, and the
pane goes back to yours when the program resets it.

With `focus-follows-mouse = true` in your Ghostty config, the pane under
the pointer takes the keys as the pointer moves onto it.

A split without the keys fades back, as in Ghostty, by
`unfocused-split-opacity` in its config (0.7 by default; 1 turns it off).
The panes of a synced tab all take the keys, so none fades.

## Scrollback

The wheel or trackpad moves through a pane's history; a thin thumb on the
pane's right edge shows where while it is scrolled back. Drag the thumb to
move through the history, or click above or below it to jump there. A pill
at the bottom of the pane says how many lines are below, counting up while
the pane carries on printing; click it to go back down. Typing also returns
the pane to its latest output.

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

`Cmd+E` finds what is selected, as in any Mac app: its first line goes
into the field and the search starts from the newest output.

## Quick select

`Cmd+Shift+Space` puts a key or two, solid in the tab's ink, on every URL,
file path, commit hash, IP address and UUID on the focused pane's screen,
and fades the rest of it. Type a target's keys to copy it, or type them
with Shift to put it at the prompt. The targets nearest the bottom get the
easiest keys, the same text gets the same keys wherever it appears, and the
keys stay put while output moves underneath them.

| Keys | Action |
| --- | --- |
| A target's letters | Copy it |
| Shift and its letters | Type it at the prompt |
| Backspace | Take back the last letter |
| Escape or `Cmd+Shift+Space` | Close quick select |

## Right click

A right click on a terminal pane opens a menu by the pointer: copy, paste
and select all, split right or down, zoom, clear, find and close, each
beside its keys. On a link it starts with opening or copying the link (for
a file, showing it in Finder or copying its path). The arrows and Return
work it too; Escape or a click elsewhere puts it away. A program that takes
the mouse, such as an editor, gets the click instead, unless Shift is held.

## Pasting

`Cmd+V` pastes as the program in front asks for pastes. A paste that would
run something, because it breaks a line where the program hasn't asked for
bracketed pastes (`cat`, an old shell, a remote prompt), waits: the strip
says how many lines it would run, and `Cmd+V` again within a few seconds
pastes it. A shell that brackets its pastes, as bash, zsh and fish do,
gets the paste at once, since it holds it as text until Return. Setting
`clipboard-paste-protection = false` in your Ghostty config turns this off.

A program can copy to the clipboard itself with OSC 52, as Neovim, tmux
and programs over ssh do; the strip says how many lines it copied.
`clipboard-write = deny` in your Ghostty config stops it. Reading the
clipboard that way is not allowed.

## Selecting

Drag to select, double-click for a word and triple-click for a line;
Option-drag selects a rectangle. Typing into the pane lets go of the
selection, as in Ghostty, so a highlight never sits over text that has
since changed.

## Shift-click

Shift-click extends the selection to where you click, from its far end, as
in any Mac text view: past its end grows it forward, before its start grows
it back.

## Option-click

Option-click in a pane to move the cursor to where you clicked, as in
Terminal: Mux presses the arrows that walk it there, so it works at a
shell's prompt, in an editor, anywhere the arrows move the cursor.
Option-drag still selects a rectangle.

## Dropping files

Drop files from the Finder on a pane to type their paths at its cursor,
each escaped for the shell and followed by a space, as Terminal does. The
pane lights in the tab's ink while files are held over it. The paths go in
as a paste, so nothing runs until Return.

## Links

Hold `Cmd` over a link in a pane's output, an OSC 8 hyperlink or a URL
written out in the text (even one wrapped across rows), to underline it and
see where it goes in the strip. `Cmd`-click opens it. File links are shown in
Finder rather than opened.

A path written in the output, such as a compiler's `src/main.rs:12:5` or
`~/notes.md`, is a link too: `Cmd`-click shows it in Finder, found from the
directory the pane's shell is in now (a folder opens). The strip says when
there is nothing there.

## Panes that call

A pane out of sight that rings the bell, or sends a desktop notification
(OSC 9 or OSC 777, as Claude Code and Codex do when they want you), is marked
until it is looked at. Its tab's dot turns peach with what it said beside the
name, and in the tab on screen the pane's head says it. A program that ran for
ten seconds or more and finishes out of sight, such as a build or a test run,
calls the same way, with how long it took: `cargo done in 3m`. While Mux is in the
background, the first call also bounces its Dock icon. A bell in the pane you
are looking at flashes it for a moment in the tab's ink instead.

A program that reports its progress with OSC 9;4, as Ghostty shows, gets a
thin bar along the top of its pane: the tab's ink while it goes, peach when
it fails, muted while it waits, and a faint band while it is busy without
saying how far. A tab out of sight carries the bar under its chip. A report
not renewed for 15 seconds goes away, as in Ghostty.

## Closing what is running

Closing a pane, or a tab, whose shell is running something in front of it,
such as a build or an editor, doesn't end it straight away: the strip names
what would end, and closing it again within a few seconds goes ahead. A pane
at its prompt closes at once.

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
