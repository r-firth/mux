# Contributing

Mux uses the Rust toolchain pinned in `rust-toolchain.toml`. Before opening a
pull request, run:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Native product checks also require Zig 0.16.0:

```sh
MUX_ZIG=/path/to/zig cargo test -p mux --features product --all-targets
MUX_ZIG=/path/to/zig cargo clippy --workspace --all-targets --features mux/product -- -D warnings
```

Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `perf:`, and so on). Release Please turns those commits into a versioned
release PR and changelog; merging that PR and passing CI creates the GitHub
release and its macOS artifacts.
