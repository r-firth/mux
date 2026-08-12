# mux

`mux` is a native Rust terminal workspace built around Ghostty terminal
emulation, persistent Zellij-style panes and tabs, and a minimal GPU-rendered
interface. External coding agents are first-class, persistent workspace
sessions through the same stable Agent Client Protocol used by Zed.

## What works now

- A persistent daemon owns sessions and real PTY-backed processes.
- The native macOS GUI is built with GPUI and `gpui-component`; a focused GPUI
  canvas renders pinned libghostty state without a second windowing stack.
- A session contains tabs, split layouts, and independent terminal panes.
- Clients attach through a versioned, length-delimited local protocol.
- Closing the GUI leaves shells alive; reopening restores Ghostty checkpoints
  and resumes the ordered output stream.
- Pane input and resize operations continue without coupling process lifetime
  to a client.
- Workspace actions and the supported Zellij default bindings are
  UI-independent. `Ctrl+p d/r` splits down/right, Option+arrows move between
  panes and across tabs at horizontal edges. Normal terminal input reserves no
  other application shortcuts, so Vim keybindings pass through unchanged.
- The primary `font-family` and `font-size` follow the user's Ghostty config,
  with a bundled JetBrains Mono Nerd Font as the portable default. Text runs,
  cursor, PTY, mouse, and selection geometry share one exact cell grid; wide
  and fallback glyphs are anchored so they cannot shift later columns.
- Ghostty owns mode-aware key, paste, scrollback, mouse-protocol, and selection
  gesture semantics—including word/line clicks, directional dragging, block
  selection, and autoscroll. The GUI supplies native input and scheduling; Shift
  temporarily releases mouse-reporting applications for local selection/scroll.
- Ghostty-compatible background, foreground, cursor, palette, primary font,
  and font size are loaded from the current user configuration. Terminal RGB,
  cell attributes, selection, and cursor style come from libghostty render
  frames rather than a second VT implementation.
- Terminal and ACP integrations have explicit internal boundaries.
- New panes and tabs start in the focused shell's live working directory; a
  brand-new workspace starts in the user's home directory.
- `Shift+Command+S` opens a lightweight session surface for creating,
  attaching, renaming, and explicitly killing daemon-owned sessions.
- A native, animated agent sheet launches Codex, Claude Agent, Gemini CLI, or
  GitHub Copilot CLI as external ACP processes without turning the terminal
  into an IDE. Integrations can be enabled independently in Settings.
- Agent sessions survive closing the GUI, stream conversation/tool/plan state,
  expose agent-provided model, effort, and mode controls, and present permission
  decisions in the application.
- Pane context is explicit: the focused terminal viewport can be attached to a
  prompt or disabled, with untrusted-context boundaries at the ACP adapter.

See [the architecture](docs/architecture.md) and
[validated risks](docs/risks.md).

## Run the native app

Building the vendored Ghostty adapter currently requires Zig 0.16. On macOS:

```sh
MUX_ZIG=/path/to/zig scripts/bundle-macos.sh
open target/Mux.app
```

The app discovers or starts its per-user daemon automatically. The daemon keeps
running when the window closes, including live ACP agent sessions.

For an isolated profile or development session, pass `--state-dir PATH` to the
GUI binary or set `MUX_STATE_DIR`. See
[the exact supported keybindings](docs/keybindings.md).

## Agents

Press `Shift+Command+A` or the agent button to open the agent sheet. With no
existing sessions, choose an agent. Working directory defaults to the focused
pane's live directory and can be overridden before launch. Built-in adapters are exact-version ACP
packages downloaded and cached by `npx` on first use, so Node.js must be
available in the daemon's `PATH`. If it is not, the launcher stays retryable
and shows the required fix instead of leaving a half-created agent session.

The composer accepts normal ACP prompts and a small local command layer:

| Command | Action |
| --- | --- |
| `/new [agent]` | Start a new ACP session (Codex by default) |
| `/context none\|pane` | Choose explicit terminal context |
| `/model [value]` | Inspect or change the ACP model option |
| `/effort [value]` | Inspect or change reasoning effort |
| `/mode [value]` | Inspect or change the ACP session mode |
| `/cancel` | Cancel the active turn |
| `/end` | End the selected session |
| `/help` | Show the local command summary |

Unknown slash commands are sent to the agent, preserving its native command
vocabulary. Conversation history scrolls independently. Permission requests,
agent modes, model, effort, authentication methods, cancellation, and session
end controls are presented with native components.

If an agent rejects session creation with ACP `auth_required`, the sheet keeps
the process alive and presents the stable agent-owned sign-in methods advertised
during initialization. Mux never asks for or stores the credential in this
flow; the external agent owns its browser or other sign-in interaction.

## Diagnostic CLI

Start the daemon:

```sh
cargo run -p mux-daemon --bin muxd -- --state-dir .mux-state
```

In another shell, create a two-pane session and inspect it:

```sh
cargo run -p muxctl -- --state-dir .mux-state new --name daily --panes 2
cargo run -p muxctl -- --state-dir .mux-state list
cargo run -p muxctl -- --state-dir .mux-state inspect daily
```

Attach the diagnostic stream, type into the focused pane, exit the client, and
run the same attach command again. The shell remains owned by `muxd`:

```sh
cargo run -p muxctl -- --state-dir .mux-state attach daily
```

## Verify

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
MUX_ZIG=/path/to/zig cargo test -p mux --features product --all-targets
MUX_ZIG=/path/to/zig cargo test -p mux-daemon --features ghostty-vendored --all-targets
```

## Releases

CI runs formatting, tests, Clippy, the vendored Ghostty product build, and a
macOS package check covering checksums, Mach-O architecture, bundle metadata,
libghostty runtime linkage, the complete code signature, and a packaged-binary
smoke test that starts the daemon, creates two real PTYs, exchanges terminal
output, reattaches, and cleans up. Conventional Commits feed Release Please;
merging its release PR and passing CI creates a GitHub release, then Apple
Silicon and Intel app archives plus portable SHA-256 checksums are built,
verified independently, and attached automatically. Local archives can be
created with:

```sh
MUX_ZIG=/path/to/zig scripts/package-macos.sh
```

Verify an archive with:

```sh
scripts/verify-macos-package.sh dist/Mux-0.2.0-macos-arm64.zip arm64 0.2.0
```
