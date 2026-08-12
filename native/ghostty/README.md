# libghostty-vt integration spike

This Zig project pins the exact Ghostty commit validated by the architecture
spike. It compiles and runs a small C consumer against the static
`ghostty-vt` artifact, exercising terminal creation, VT ingestion, resize, and
snapshot encoding.

It requires Zig 0.16.0. It is deliberately separate from the default Cargo
build while the C API is unversioned and changing.

```sh
cd native/ghostty
zig build run
```

The next step is to generate a narrow Rust binding for the same pin and make it
an optional `mux-terminal` backend. Do not expose raw Ghostty handles or structs
outside that adapter.

