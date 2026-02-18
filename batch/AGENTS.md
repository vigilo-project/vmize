# Repository Guidelines

## Product Context
`batch` is VMize's workload runner. It turns each task into an isolated VM execution.

## Structure

- `src/bin/batch.rs` — CLI entrypoint
- `src/lib.rs` — public API surface
- `src/runner.rs`, `src/result.rs`, `src/error.rs` — core logic
- `tests/integration.rs` — end-to-end tests (require QEMU)
- `example/` — sample job JSON files and input scripts

## Commands

```bash
cargo build --release
cargo test -- --nocapture
cargo fmt && cargo clippy
```

## Conventions

- Rust defaults: `snake_case` functions, `PascalCase` types, `SCREAMING_SNAKE_CASE` constants
- Explicit `Result` error propagation, small composable functions
- Commit subjects: short imperative, matching existing history
- Keep commits focused — don't mix refactors with behavior changes
