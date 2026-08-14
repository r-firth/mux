# Pinned libghostty-vt build

This Zig project pins the Ghostty revision used by Mux. It has two jobs:

- `zig build run` compiles and runs a small C ABI smoke test covering terminal
  creation, VT ingestion, resize, and snapshot encoding.
- The `mux-terminal-ghostty` build script invokes `zig build install` for the
  `vendored` feature, then compiles the narrow C shim used by the Rust adapter.

Raw Ghostty handles and structs stay inside `mux-terminal-ghostty`; neither the
GUI nor the daemon depends on its unstable C ABI directly.

Both paths require the exact Zig version declared in
`crates/mux-terminal-ghostty/build.rs`. Install it with the project helper:

```sh
MUX_ZIG="$(../../scripts/install-zig-macos.sh)"
"$MUX_ZIG" build run
```

From the repository root, validate the Rust integration with:

```sh
MUX_ZIG="$(scripts/install-zig-macos.sh)" \
  cargo test -p mux-terminal-ghostty --features vendored
```

When updating the Ghostty pin, run both checks and the native product checks in
`CONTRIBUTING.md`. Treat snapshot compatibility and exported C signatures as an
explicit migration; do not expose Ghostty's checkpoint format through the
public IPC contract.
