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
| `Shift+Command+A` | Open the native ACP agent surface |
| `Shift+Command+S` | Open the native session switcher |

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
| `r` | Rename the active tab |
| `x` | Close the active tab and return to Normal |

The session dialog exposes create, attach, rename, and confirmed kill actions.
Zellij's `Ctrl+o` session prefix intentionally has no Normal-mode binding yet.
This keeps foreground Control-key input untouched except for `Ctrl+p` and
`Ctrl+t`.

The agent surface is an application-level overlay rather than a terminal mode.
Escape or its close button dismisses it without ending the agent session.
Conversation, configuration, authentication, cancellation, end-session, and
permission actions use the native sheet controls. Its local slash commands are
documented in the README.
