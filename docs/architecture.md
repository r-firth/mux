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
move later cells. GPUI Component supplies sheets, dialogs, inputs, switches,
buttons, notifications, icons, and animation without imposing IDE chrome.

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

The daemon also owns ACP processes. Closing the native agent surface only
detaches that view; explicit `/end` closes the selected process and the GUI
hides its closed snapshot from the session picker. A new agent is started for a
pane ID rather than a GUI-supplied guessed path, so the daemon can resolve the
foreground shell's current directory at the moment of launch.

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
