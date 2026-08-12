# mux

`mux` is a native Rust terminal workspace built around Ghostty terminal
emulation, persistent Zellij-style panes and tabs, and a minimal GPU-rendered
interface. External coding agents are first-class, persistent workspace
sessions through the same stable Agent Client Protocol used by Zed.

## What works now

- A persistent daemon owns sessions and real PTY-backed processes.
- The native macOS GUI renders pinned libghostty state with `wgpu` and `glyphon`.
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
- A bundled JetBrains Mono Nerd Font and system fallback cover shell glyphs,
  Unicode, CJK, emoji, bold, italic, faint, and terminal decorations.
- Ghostty owns mode-aware key, paste, scrollback, mouse-protocol, and selection
  gesture semantics—including word/line clicks, directional dragging, block
  selection, and autoscroll. The GUI supplies native input and scheduling; Shift
  temporarily releases mouse-reporting applications for local selection/scroll.
- OSC 8 hyperlinks come directly from Ghostty cell state. Hold Command to reveal
  a link with native pointer feedback and Command-click it without interfering
  with ordinary terminal mouse input.
- Ghostty-compatible background, foreground, cursor, and palette values are
  loaded from the current user theme; terminal RGB is rendered in the correct
  sRGB colour space. Cursor visibility and DECSCUSR/DEC blink policy come from
  Ghostty, with its 600 ms cadence and activity/focus resets scheduled natively.
- Terminal and ACP integrations have explicit internal boundaries.
- New panes and tabs start in the focused shell's live working directory; a
  brand-new workspace starts in the user's home directory.
- `Shift+Command+S` opens a lightweight session surface for creating,
  attaching, renaming, and explicitly killing daemon-owned sessions.
- A native, animated agent surface launches Codex, Claude Agent, or Gemini as
  external ACP processes without turning the terminal into an IDE.
- Agent sessions survive closing the GUI, stream conversation/tool/plan state,
  expose agent-provided model, effort, and mode controls, and present permission
  decisions in the application.
- Pane context is explicit and opt-in: attach selected text or the focused
  terminal viewport to a prompt, with untrusted-context boundaries at the ACP
  adapter.

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

Press `Shift+Command+A` to open the agent surface. With no existing sessions,
choose an agent and press Enter or click Start. Working directory defaults to
the focused pane's live directory. Built-in adapters are exact-version ACP
packages downloaded and cached by `npx` on first use, so Node.js must be
available in the daemon's `PATH`. If it is not, the launcher stays retryable
and shows the required fix instead of leaving a half-created agent session.

The composer accepts normal ACP prompts and a small local command layer:

| Command | Action |
| --- | --- |
| `/new [agent]` | Start a new ACP session (Codex by default) |
| `/agents` | Return to existing sessions |
| `/cwd [path]` | Inspect or override the next session's directory |
| `/context none\|selection\|pane` | Choose explicit terminal context |
| `/login [method]` | Run an agent-advertised ACP sign-in method |
| `/model [value]` | Inspect or change the ACP model option |
| `/effort [value]` | Inspect or change reasoning effort |
| `/mode [value]` | Inspect or change the ACP session mode |
| `/config <id> <value>` | Change any other agent-provided option |
| `/end` | End the selected session after a second confirmation |

Typing `/` opens a compact command palette that merges Mux lifecycle commands
with the selected agent's live ACP `available_commands_update` catalog. Arrow
keys choose and Tab completes; unknown slash commands are still sent to the
agent, preserving its native command vocabulary. Page Up/Down or the mouse
wheel navigates conversation history. Permission choices work with number keys
or the mouse. `Shift+Enter` inserts a newline, `Command+V` pastes, and
`Ctrl+U` or `Command+Backspace` clears the composer.

If an agent rejects session creation with ACP `auth_required`, the surface
keeps the process alive, presents a sign-in-required state, and lets `/login`
run one of the stable agent-owned methods advertised during initialization.
Mux never asks for or stores the credential in this flow; the external agent
owns its browser or other sign-in interaction.

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
macOS bundle smoke check. Conventional Commits feed Release Please; merging its
release PR and passing CI creates a GitHub release, then Apple Silicon and Intel
app archives plus portable SHA-256 checksums are built and attached
automatically. Local archives can be created with:

```sh
MUX_ZIG=/path/to/zig scripts/package-macos.sh
```
