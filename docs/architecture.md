# Architecture

The product is a thin native GUI attached to a durable local workspace daemon.
The daemon is authoritative for processes, workspace topology, and terminal
state. A GUI is a replaceable view and input source; closing it never implies
closing a shell.

```text
native GPUI GUI
  GPUI window + input / libghostty canvas / gpui-component overlays
         | local framed IPC
         v
workspace daemon
  sessions -> tabs -> pane layout
  pane -> PTY + child + canonical terminal engine
  agent session -> external ACP process + durable UI snapshot
  fleet -> the coding agent in each terminal pane, and what it is doing
```

## Boundaries

- `mux-workspace` owns stable product concepts: sessions, tabs, split trees,
  focus, actions, modes, and keymaps. It has no windowing or process code.
- `mux-protocol` owns the versioned GUI/daemon wire contract. Terminal payloads
  are binary rather than JSON/base64. Every output event is sequenced. Its epoch
  changes with incompatible serialized types so a long-lived older daemon is
  rejected at the hello exchange instead of corrupting a new GUI's decoder.
- `mux-terminal` owns the terminal-engine contract. The product implementation
  wraps a pinned `libghostty-vt` ABI and produces opaque checkpoints plus replay
  data. A small replay engine remains available for protocol tests.
- `mux-daemon` owns PTYs and children. A pane reader feeds the canonical
  terminal engine before publishing output, so a slow or absent GUI cannot
  backpressure the shell.
- `mux-client` is the transport SDK shared by the native GUI and diagnostic
  tools. It does not own workspace state.
- `mux-acp` is the product-facing agent boundary. ACP transport types stay
  behind it so protocol or SDK upgrades do not leak into the UI model. It runs
  stable ACP v1 through the maintained Rust SDK, owns external agent processes,
  normalizes streaming messages/tools/plans/permissions, and exposes modes and
  configuration without agent-specific UI code.

- `mux-fleet` owns the coding agents that run in terminal panes: which
  agent a process is, what its hooks and its output say it is doing, the
  project it works in, and putting Mux's hooks into an agent's settings and
  taking them out. It keeps no clock and starts no process; the daemon feeds
  it and says when things happened.

## Attach consistency

The client subscribes to live events before the daemon captures an attachment.
The attachment records `next_sequence`; `mux-client` discards any already-queued
event below that cursor as a duplicate. A sequence gap or a lagged live
broadcast produces `ResyncRequired` instead of silently losing terminal output.

With Ghostty, the attachment is an opaque terminal snapshot at sequence `N`
plus any output after `N`. The GUI restores its local render replica from
that snapshot and continues feeding ordered PTY bytes. The daemon remains the
canonical emulator because it must process terminal effects and answer PTY
queries even when no GUI exists.

This deliberately accepts a second terminal replica in each attached GUI. VT
parsing is inexpensive relative to rendering, and the design avoids serializing
a bespoke cell-diff protocol across an unstable C API. Measurements will decide
whether that trade remains correct.

## Rendering direction

The native shell uses GPUI for the window, input dispatch, shaping, and GPU
painting. A custom GPUI canvas consumes libghostty render-state snapshots
through the terminal boundary, paints exact cell backgrounds and cursor shapes,
and anchors shaped runs to Ghostty columns so fallback and wide glyphs do not
move later cells. GPUI Component supplies text inputs and the window root;
the chrome around them (the strip, slabs, sheets, tooltips and messages) is
drawn with plain GPUI elements in the mono face, without IDE chrome.

Ghostty owns selection formatting, bracketed paste, mode-aware keyboard and
mouse encoding, and the scrollback viewport. The GUI feeds its reusable
selection-gesture engine native pointer geometry. Accessibility, IME preedit,
hyperlink activation, search, cursor-blink scheduling, damage-aware row caches,
and broader compatibility testing remain base-terminal work and should be
completed before claiming parity with mature native terminals.

No rendering dependency is pulled into the daemon, and no GUI type crosses the
wire. This keeps an AppKit-specific shell or a future renderer experiment from
changing session ownership.

## Lifecycle

The native app discovers or starts its executable in detached daemon mode; its
local socket has mode 0600. The separate `muxd` binary exists for diagnostics
and integration tests, but is not required beside the application bundle.
Closing the GUI does not terminate the daemon or its shells. A daemon crash
remains distinct from closing the GUI and currently ends the live processes it
owns.

The daemon writes its workspace to `workspace.json` in the state directory
(mode 0600): each session's layout, tab names and inks, spawn command, and
the live working directory of every pane. It writes on each workspace change
and every five seconds if anything, such as a pane's directory, has moved.
A daemon that starts finds the file before it accepts clients and spawns a
fresh shell for every saved pane, keeping the pane, tab and session ids, so a
restart or a protocol upgrade loses the running programs but not the shape of
the workspace. A file it cannot read, or one from another format version, is
left alone.

What each pane showed is kept beside it, as plain text, in
`pane-text/<pane id>.txt` (mode 0600): the last thousand lines of its history
and screen, written every five seconds when they have changed, and removed
with the pane. A restored pane shows that text above a faint rule before its
new shell's prompt. A pane with only its prompt on it keeps nothing, and one
untouched since it was itself restored keeps what it came back with, so an
idle pane does not gather a rule at every restart.

The window keeps its own frame beside it, in `window.json`: where it was
last left and at what size, and whether it was zoomed. The app opens there
when a display still shows that frame, and centred otherwise.

Workspace mutations and focused terminal input share one ordered backend
queue. This prevents keystrokes immediately following a pane, tab, or session
change from being routed to the formerly focused PTY. A GUI can also select an
explicit state directory for isolated profiles and native integration tests.

The pane runtime also observes the PTY foreground process group. If a child
process enabled Kitty key-release reporting and exits without restoring it,
the daemon clears that orphaned mode when the shell regains the terminal and
publishes the reset in the same ordered output stream consumed by GUI replicas.
This keeps durable checkpoints and every attached client in agreement while
preserving enhanced keyboard input for live TUIs.

Panes find the shell's prompts by the OSC 133 marks it prints. The daemon
writes a `.zshenv` to `shell-integration/zsh` in the state directory and
starts zsh with `ZDOTDIR` there; it restores the user's `ZDOTDIR` (passed in
`MUX_USER_ZDOTDIR`), sources their own `.zshenv`, and adds precmd and preexec
hooks that print the marks, so the rest of their startup files load as
usual. bash is given a `PROMPT_COMMAND` that prints the prompt mark, and how the
last command ended, ahead of any the user exported, keeping `$?` for it,
and a `PS0` that marks where each command starts (bash 4.4 and later). The
GUI keeps how each prompt's command ended and how long it ran, from the
start and end marks, and lines them up with the prompts from the newest.

The daemon also owns ACP processes. Closing the native agent surface only
detaches that view; explicit `/end` closes the selected process and the GUI
hides its closed snapshot from the session picker. A new agent is started for a
pane ID rather than a GUI-supplied guessed path, so the daemon can resolve the
foreground shell's current directory at the moment of launch.

## Fleet

An agent run from a shell, `claude` or `codex` in a terminal pane, is a fact
about that pane, not a session Mux owns. The daemon learns of it from four
signals, taking each as it comes: the hooks Mux installs in the agent, OSC 7501
in the pane's output, a notification or a bell, and the process in front of
the pane's shell, which says an agent is there and when it has gone. Mux
never reads an agent's screen to guess its state.

Every pane's environment carries `MUX_PANE_ID` and `MUX_CTL`, the `muxctl`
beside the Mux that owns the pane. An installed hook is one shell line,
`[ -z "$MUX_PANE_ID" ] || "$MUX_CTL" hook claude Stop`, so it does nothing
outside a Mux pane, names no path an update could leave stale, prints
nothing and always succeeds. `muxctl hook` reads the agent's event and
reports it to the daemon for that pane. Installing touches only Mux's own
entries in `~/.claude/settings.json`, `~/.codex/hooks.json` and
`~/.copilot/hooks/mux.json`, and copies a file beside itself the first time
it changes it. pi has no hooks to list: Mux's is an extension of its own,
`~/.pi/agent/extensions/mux-fleet.ts`, that runs `muxctl hook pi` at pi's
events.

The daemon keeps one fleet for all its sessions, tells every client of each
change, and writes it to `fleet.json` in the state directory, so an agent
that has ended, or was running when the daemon stopped, is still listed
with the command that picks its session up again. The GUI holds a copy and
draws the fleet view from it: the roster is its own, and the chosen agent's
pane is that pane's ordinary terminal, shown alone, so stepping into it is
the same input path as typing in its tab. A pane of another session has no
terminal in this window, so the view asks the daemon for the end of its
text instead.

## Agent boundary

Mux behaves as an ACP client analogous to Zed. Codex, Claude Agent, and Gemini
are launch profiles, not bespoke integrations. After initialization, the
adapter stores an application-facing snapshot and translates ACP notifications
into durable timeline events. Model, reasoning effort, and modes come from the
agent's advertised configuration, so the GUI never assumes Codex-specific IDs.
Agent-native slash commands likewise come from ACP's live
`available_commands_update` snapshot and are combined with Mux's small local
lifecycle command set in the composer's keyboard completion overlay. Custom
agents are ordinary `AgentSpec` launch recipes loaded from Zed-compatible
`agent_servers` settings; they use the same ACP adapter and UI as built-ins.

Authentication remains agent-owned. Mux stores the stable auth methods from
`initialize`; when `session/new` returns `auth_required`, it presents those
methods in the agent sheet, sends ACP `authenticate` with the selected method ID, and retries
`session/new`. Credentials never enter Mux IPC or durable agent snapshots.

Tab terminal context is on by default. The GUI adds selected text or visible
viewport text from the other panes as separate, size-bounded ACP content blocks
marked as untrusted terminal data. `@` completion indexes the live working
directory off the render thread; chosen files are read asynchronously under
per-file and total size limits and sent as distinct, explicitly untrusted ACP
content blocks. The client advertises only capabilities it actually implements;
agents cannot silently acquire a filesystem or terminal proxy.

See [validated risks and decisions](risks.md) for the pinned Ghostty and ACP
assumptions that need deliberate revalidation during dependency upgrades.
