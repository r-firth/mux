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
| `Shift+Command+A` | Toggle the native ACP agent surface |

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
| `f` | Toggle focused-pane zoom and return to Normal |
| `x` | Close the focused pane and return to Normal |

## Tab mode

| Keys | Action |
| --- | --- |
| `h` / `k` / Left / Up | Previous tab |
| `l` / `j` / Right / Down | Next tab |
| `1` through `9` | Select a numbered tab and return to Normal |
| `n` | Create a tab and return to Normal |
| `x` | Close the active tab and return to Normal |

Resize and session actions exist behind the action boundary but intentionally
have no Normal-mode prefix yet. This keeps the foreground program untouched
except for `Ctrl+p`, `Ctrl+t`, and the explicit Option navigation bindings.

The agent surface is an application-level overlay rather than a terminal mode.
While open, Escape or `Shift+Command+A` closes it, `Ctrl+c` cancels the active
turn, Page Up/Down navigates history, and number keys answer a visible
permission request. Its composer commands are documented in the README.
