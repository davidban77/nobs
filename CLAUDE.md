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
