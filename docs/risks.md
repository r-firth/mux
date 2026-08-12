# Validated risks and decisions

Research and spikes were performed on 2026-08-12.

## libghostty

Ghostty's current repository describes `libghostty-vt` as usable but with API
signatures still in flux. Ghostty 1.3 also says the standalone module is not yet
versioned independently and the C API is a work in progress:

- <https://github.com/ghostty-org/ghostty#cross-platform-libghostty-for-embeddable-terminals>
- <https://ghostty.org/docs/install/release-notes/1-3-0#libghostty>

The released `v1.3.1` C surface does not yet expose the complete terminal,
render-state, and snapshot APIs required by this architecture. Ghostty commit
`b2fa2931b6599f7e32a7c547b3f5520ac3333881` does expose them. We built that
commit successfully on Apple Silicon with Zig 0.16.0 and verified these exported
symbols:

```text
ghostty_terminal_new
ghostty_terminal_vt_write
ghostty_terminal_resize
ghostty_render_state_update
ghostty_snapshot_encode_alloc
```

Decision: pin a known Ghostty commit, keep all C ABI access in one adapter, and
make its upgrade a deliberate compatibility task. Do not port Ghostty's VT
implementation to Rust. Do not make Ghostty's opaque snapshot format part of
our public IPC version; identify it by engine build and checkpoint format.

The pinned engine is now exercised end-to-end through the native GPU renderer:
Unicode fallback, wide-cell placement, styled cells, cursor, colors, damage,
checkpoints, resize, reattachment, selection/copy/paste, mode-aware keyboard and
mouse encoding, scrollback, and sustained output all have automated or
native-window coverage. OSC 8 URIs are carried through the adapter per cell and
activated only by a deliberate modifier-click, with no regex reconstruction in
the GUI. Ghostty's gesture engine now owns cell/word/line selection, repeat-click
thresholds, direction-aware and rectangular dragging, and viewport autoscroll;
the GUI only translates native pane geometry and runs its timer. The primary
remaining terminal risks are search, cursor/blink timing, accessibility, and
broader compatibility testing. Native IME is enabled, preedit is rendered, and
candidate windows follow the active terminal caret or agent composer input area.

The C render adapter retains and grows its row, cell, and grapheme buffers
instead of allocating a full viewport on every frame. The Rust render boundary
still materializes an owned cell frame before row shaping. Measure that copy
and its allocation profile under sustained output before replacing it with a
more incremental borrowed/delta API.

## ACP

ACP v1 is the current stable protocol; v2 is draft. The maintained Rust SDK is
the `agent-client-protocol` crate, currently at crate version 2.0.0. The crate
major is not the ACP wire version. The application must initialize agents with
stable ACP protocol v1 until v2 is finalized:

- <https://agentclientprotocol.com/libraries/rust>
- <https://agentclientprotocol.com/rfds/rust-sdk-v1>

Zed runs external agents as separate processes and leaves runtime, auth, model,
and native configuration with the agent. We will preserve the same boundary:

- <https://github.com/zed-industries/zed/blob/main/docs/src/ai/external-agents.md>

The maintained Codex adapter has moved from `zed-industries/codex-acp` to
`agentclientprotocol/codex-acp`; it is a stdio ACP agent that translates to the
Codex App Server. We should spawn that adapter like any other ACP agent rather
than add Codex-specific UI plumbing:

- <https://github.com/agentclientprotocol/codex-acp>

Decision: keep stable product events such as message deltas, tool activity, and
permission prompts in `mux-acp`, with raw ACP schema objects at the adapter
edge. That boundary now runs real Codex sessions through the maintained adapter
and has been exercised for prompt streaming, tool calls, mode and reasoning
configuration, permission rejection, explicit context, GUI detach/reattach,
and process termination. Launch recipes are pinned to versions observed in the
official ACP registry so a package update cannot silently change a released
Mux build.

Authentication remains owned by each agent. Existing Codex credentials are
inherited by its external process; richer ACP authentication-method discovery
and installation/update UI remain the largest cross-agent lifecycle risk.

## PTY and IPC

`portable-pty` owns Unix/macOS processes and keeps ConPTY possible later. Its
read interface is blocking, so each pane uses a dedicated drain thread plus a
bounded coalescing stage. The latter applies up to 64 KiB of output to the
canonical Ghostty state per batch before publishing one ordered event. This
prevents a slow GUI from stalling a busy process without adding noticeable
interactive-output latency.

Before calling this production-ready, measure thread cost at high pane counts,
terminal throughput, attach latency, and resize behavior. If the blocking model
is material, replace only the PTY backend with a kqueue/epoll adapter; the pane
runtime and IPC contract do not change.
