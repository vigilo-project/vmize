# Repository Guidelines

## Product Context
`batch` is VMize's task execution engine. It runs task directories inside ephemeral VMs.

## Structure
- `src/lib.rs` — public API surface
- `src/runner.rs`, `src/result.rs`, `src/error.rs` — core logic
- `src/task.rs` — task definition loading (`task.json`)
- `tests/integration.rs` — end-to-end tests (requires QEMU)
- `example/` — sample task directories
- `../cli/src/main.rs` — workspace CLI entrypoint (`vmize task`)

## Minimum Goal (MVP)
A `batch` change is acceptable only if all of these remain true:
1. A valid task directory (`task.json` + `scripts/`) runs successfully.
2. Task scripts execute in deterministic filename order.
3. Output is collected back to host `output/`.
4. Failure is surfaced as non-zero exit and clear error output.
5. `--concurrent` rejects more than 4 tasks.

## Acceptance Checklist
- Task loading errors are clear (`Failed to load task ...`).
- Successful runs produce expected output artifacts.
- `--concurrent` limit enforcement is unchanged.
- `vmize task` usage/help behavior is unchanged for invalid args.

## Verification Commands
```bash
cargo build --release -p batch
cargo test -p batch
cargo run -p vmize -- task --help

# Optional integration path
BATCH_OLLAMA_IT=1 cargo test run_task_with_options_ollama_prompt_collects_answer --test integration -p batch -- --nocapture
```

## Conventions
- Rust defaults: `snake_case`, `PascalCase`, `SCREAMING_SNAKE_CASE`
- Prefer explicit `Result` propagation and small composable functions
- Keep commits focused; avoid mixing refactor and behavior changes
