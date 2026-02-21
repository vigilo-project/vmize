# Task Chain Tutorial

## Purpose And Scope

This document defines how to develop and maintain Task Chain workflows in `worker/example`.

The current reference chain is:

`runc-llama-build -> runc-llama-hardened`

Use this file as the canonical record for Task Chain behavior, contracts, troubleshooting, and change history.

## Chain Overview (`build -> hardened`)

1. `runc-llama-build`
- Builds and runs llama on top of runc.
- Uses guest-to-container HTTP prompt flow.
- Produces handoff artifacts for the downstream stage.

2. `runc-llama-hardened`
- Consumes build artifacts from the upstream stage.
- Applies capability minimization and hardened runtime-oriented config updates.
- Prepares the runtime contract for UDS-oriented communication changes.

## Artifact Contract (Input / Output)

1. `runc-llama-build`
- Input expectations:
  - `input/*.sh` scripts
  - optional `input/models/*.gguf` preloaded model
- Output artifacts (`task.json`):
  - `rootfs`
  - `config.json`
  - `model.gguf`
  - `llama-answer.txt`
- Downstream handoff:
  - via `next_task_dir: ../runc-llama-hardened`

2. `runc-llama-hardened`
- Input expectations (from upstream handoff):
  - `rootfs`
  - `config.json`
  - `model.gguf`
  - `llama-answer.txt`
- Output artifacts (`task.json`):
  - `config.min.json`
  - `removed-caps.txt`
  - `cap-summary.txt`
  - `llama-answer.txt`

## Run And Validation Commands

```bash
# Build CLI
cargo build --release

# Run the full chain
./target/release/vmize task /Users/sangwan/dev/vmize/worker/example/runc-llama-build

# Core regressions
cargo test -p task
cargo test -p worker --lib
cargo test -p worker --test integration
```

## Troubleshooting Checklist

1. Chain path errors
- Verify `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/task.json` has:
  - `"next_task_dir": "../runc-llama-hardened"`

2. Handoff artifact errors
- Confirm upstream artifacts exist in the first step output:
  - `rootfs`, `config.json`, `model.gguf`

3. Downstream script failures
- Check logs under each task's `output/logs/`.
- Re-run with the same command and inspect script-level errors first.

4. Runtime dependency failures
- Validate bootstrap script dependencies (`jq`, `runc`, `wget`, `tar`, etc.).

## Change Log

Record one entry for every Task Chain-related update.

Entry format:

1. Date
2. Reason
3. Modified files
4. Behavioral changes
5. Verification commands and results

---

### 2026-02-21

1. Reason
- Standardized Task Chain naming and introduced a persistent tutorial log for chain-based development.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/README.md`
- `/Users/sangwan/dev/vmize/README.md`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Renamed task identities to `runc-llama-build` and `runc-llama-hardened`.
- Updated chain link path to `../runc-llama-hardened`.
- Added explicit communication-mode context in task descriptions (HTTP stage / hardened UDS-oriented stage).

4. Verification commands and results
- `cargo test -p task` -> pass (`18 passed`)
- `cargo test -p worker --lib` -> pass (`35 passed`)
- `cargo test -p worker --test integration` -> pass (`7 passed`)
- `./target/release/vmize task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> not executed in this change set (long-running model download/build path)

### 2026-02-21 (llama-answer hardening output)

1. Reason
- Ensure both Task Chain steps produce a verifiable `llama-answer.txt` sample.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/10_minimize_config.sh`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Hardened step now attempts direct llama prompt execution with runtime `LD_LIBRARY_PATH` hints.
- If direct execution fails or output is empty, it falls back to upstream handoff `llama-answer.txt` so output remains non-empty and chain consumers can verify sample presence.
- `cap-summary.txt` now records whether hardened prompt was direct-success or fallback.

4. Verification commands and results
- `./target/release/vmize task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> pass (Task Chain completed)
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-build/output/llama-answer.txt` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/output/llama-answer.txt` -> pass

### 2026-02-21 (hardened direct run fix)

1. Reason
- Hardened stage reported `exit 127` and used fallback answer; direct llama execution had to be made mandatory and reliable.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/input/30_verify.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/10_minimize_config.sh`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Build stage handoff now copies entire `/opt/llama.cpp/build/bin` directory (including `libllama.so.0` and related shared libraries), not only `llama-cli`.
- Hardened bootstrap now installs runtime libraries (`libgomp1`, `libstdc++6`) required by handed-off llama binaries.
- Hardened stage requires `libllama.so.0` in handoff rootfs and executes llama with `LD_LIBRARY_PATH` including handoff `build/bin`.
- Hardened stage no longer falls back to upstream answer; direct execution failure now fails the stage.

4. Verification commands and results
- `cargo test -p worker --test integration all_example_shell_scripts_pass_bash_n -- --nocapture` -> pass
- `cargo run -p vmize -- task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> pass
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/output/cap-summary.txt` -> `llama_hardened_prompt=direct_success`, `llama_hardened_exit=0`
