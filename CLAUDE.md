# nobs

Run these six checks from the project folder, each as written, before you call a change done.

1. `cargo build --workspace`
2. `cargo nextest run --workspace`
3. `cargo test --workspace --doc`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. `cargo fmt --all -- --check`
6. `cargo audit`

Write no code comments by default.

A paragraph in Markdown is one line, never hard-wrapped.

`reference/` is read-only.

Once per machine, run `rustup target add aarch64-unknown-linux-musl`.

Once per machine, run `rustup component add llvm-tools`.

Build the static Linux arm64 binary with `cargo build --release --target aarch64-unknown-linux-musl -p nobs-cli`. The file lands at `target/aarch64-unknown-linux-musl/release/nobs`.

`.cargo/config.toml` sets the linker `rust-lld` for that target, and `clang` with `tools/llvm-ar` for the C code of the SSH library, because the Mac's own tools cannot make Linux files.
