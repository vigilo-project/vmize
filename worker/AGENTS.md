# Repository Guidelines

## Product Context
`worker` is VMize's task execution engine. It runs task directories inside ephemeral VMs.

## Structure
- `src/lib.rs` — public API surface
- `src/runner.rs`, `src/result.rs`, `src/error.rs` — core logic
- `src/vm_ops.rs` — `VmOps` trait, `RealVmOps`, `MockVmOps`
- `../task/src/lib.rs` — task definition loading (`task.json`)
- `tests/integration.rs` — end-to-end tests (requires QEMU)
- `example/` — sample task directories
- `../cli/src/main.rs` — workspace CLI entrypoint (`vmize task`)

## Minimum Goal (MVP)
A `worker` change is acceptable only if all of these remain true:
1. A valid task directory (`task.json` + `input/`) runs successfully.
2. Task commands execute in the order declared in `task.json`.
3. Output is collected back to host `output/`; logs land in `output/logs/`.
4. Failure is surfaced as non-zero exit and clear error output.
5. `--batch` rejects more than 4 tasks.

## Acceptance Checklist
- Task loading errors are clear (`Failed to load task ...`).
- Missing `input/` directory is caught at load time, not at runtime.
- Missing command files in `input/` are caught at load time.
- Successful runs produce expected output artifacts.
- `--batch` limit enforcement is unchanged.
- `vmize task` usage/help behavior is unchanged for invalid args.

## Verification Commands
```bash
cargo build --release -p worker
cargo test -p worker --lib      # unit tests only (no QEMU)
cargo test -p worker            # all tests (integration requires QEMU)
cargo run -p vmize -- task --help

# Optional integration path
BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p worker -- --nocapture
```

## Conventions
- Rust defaults: `snake_case`, `PascalCase`, `SCREAMING_SNAKE_CASE`
- Prefer explicit `Result` propagation and small composable functions
- Keep commits focused; avoid mixing refactor and behavior changes
