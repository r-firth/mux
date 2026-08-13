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

The pinned engine is exercised end-to-end through the native GPUI renderer:
Unicode fallback, wide-cell placement, styled cells, cursor, colors,
checkpoints, resize, reattachment, selection/copy/paste, mode-aware keyboard and
mouse encoding, and scrollback have automated or native-window coverage. OSC 8
URIs are carried through the adapter per cell, but activation UI is not yet wired.
Ghostty's gesture engine owns cell/word/line selection, repeat-click
thresholds, and direction-aware and rectangular dragging; the GUI translates
native pane geometry. The primary
remaining terminal risks are IME, hyperlink interaction, cursor-blink scheduling,
search, accessibility, and broader compatibility testing. The pinned C API exposes viewport and history cells but not Ghostty's
synchronous screen-search engine, so search is deliberately held behind the
terminal boundary instead of duplicating VT-aware matching with an O(history)
GUI scan.

The C render adapter retains and grows its row, cell, and grapheme buffers
instead of allocating a full viewport on every frame. The Rust render boundary
also refreshes an existing owned frame in place, retaining its viewport vectors
and per-cell grapheme storage across output, scroll, selection, and resize
updates. It still copies cell fields before row shaping; measure that remaining
copy under sustained output before considering a more incremental borrowed or
delta API.

Mux loads Ghostty's primary `font-family` and `font-size` settings at startup
and resolves the requested local face in GPUI's shaping database. The same cell metrics
drive rendering, cursor placement, pane sizing, PTY resize, mouse reporting,
and selection. An unavailable configured face falls back to the
bundled JetBrains Mono Nerd Font, while the shaping stack retains system glyph
fallback. Broader Ghostty font features such as per-style family overrides,
variation axes, synthetic-style controls, and explicit fallback lists remain
outside the supported configuration subset and should be added at this boundary
rather than leaking font assumptions into workspace code.

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
process termination, and live agent-advertised slash commands. Launch recipes
are pinned to versions observed in the official ACP registry so a package
update cannot silently change a released Mux build. Streaming snapshot changes
are coalesced at the native redraw boundary, so token deltas do not repeatedly
reshape the agent surface between display frames or starve terminal rendering.

Authentication remains owned by each agent. Existing Codex credentials are
inherited by its external process. For fresh installs, Mux now persists the
stable methods advertised by `initialize`, surfaces an authentication-required
state, runs the selected agent-owned method from the agent sheet, and retries
`session/new`. It deliberately does not enable ACP's unstable terminal or
environment-variable auth transports. Adapter installation/update UX remains
the largest cross-agent lifecycle risk. Mux now preflights the external runtime
before creating a durable session, explains the Node.js requirement for its
built-in pinned adapters, and leaves a failed launcher immediately retryable;
fully managed adapter installation remains future work.

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
