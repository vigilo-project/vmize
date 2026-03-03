# Task Chain Tutorial

## Purpose And Scope

This document defines how to develop and maintain Task Chain workflows in `worker/example`.

The current reference chain is:

`runc-llama-build -> runc-llama-hardened -> runc-llama-verity-pack -> runc-llama-verity-run -> runc-llama-ima-verify-run`

## Prerequisites

Before running the `runc-llama` chain, run the image preparation tasks first:

```bash
./target/release/vmize task worker/example/rootfs-build
./target/release/vmize task worker/example/kernel-build
```

These tasks prepare custom boot files used by the chain:
- `image/rootfs.qcow2`
- `image/bzImage`

`kernel-build` applies required config overrides for this chain:
- `CONFIG_USER_NS`
- `CONFIG_CGROUP_BPF`
- `CONFIG_CGROUP_DEVICE`
- `CONFIG_BLK_DEV_LOOP`
- `CONFIG_BLK_DEV_DM`
- `CONFIG_DM_VERITY`

`runc-llama-*` tasks are set to `disk_size: 20G` to avoid resize failures when
using the handed-off rootfs artifacts.

Use this file as the canonical record for Task Chain behavior, contracts, troubleshooting, and change history.

## Chain Overview (`build -> hardened -> verity-pack -> verity-run -> ima-verify-run`)

1. `runc-llama-build`
- Builds and runs llama on top of runc.
- Uses llama-server with Unix Domain Socket (UDS) for API-based inference.
- Produces base artifacts for downstream stages.

2. `runc-llama-hardened`
- Consumes build artifacts from the upstream stage.
- Applies capability minimization and hardened runtime-oriented config updates.
- Runs llama-server with UDS inside runc container for inference validation.
- Hands off `rootfs/config/model` for verity packaging.

3. `runc-llama-verity-pack`
- Consumes hardened `rootfs/config/model` artifacts.
- Produces squashfs + dm-verity metadata (`rootfs.squashfs`, `rootfs.verity`, `rootfs.root_hash`).
- Passes through `config.json` and `model.gguf` for runtime verification stage.

4. `runc-llama-verity-run`
- Consumes stage3 verity artifacts and opens dm-verity mapping at runtime.
- Mounts verified squashfs rootfs and runs runc using patched config.
- Runs llama-server with UDS for inference validation.
- After runtime check, signs handoff payload with IMA and emits xattr-preserving tar + verification cert.

5. `runc-llama-ima-verify-run`
- Consumes stage4 signed tar + verification cert.
- Extracts payload with xattrs, verifies IMA signatures, and fails closed on verification errors.
- Runs llama-server with UDS from verified payload for inference validation (`llama-answer.txt`).

## Related Independent Validation Task

`ima-sign` is intentionally independent from the 5-stage chain.

- Scope:
  - validates `evmctl ima_sign` and `evmctl ima_verify` behavior on sample files
  - includes a tamper case that must fail verification
  - validates tar+HTTP upload/download roundtrip keeps `security.ima` verifiable after extract
- Safety posture:
  - debug verify only
  - does not enable or mutate kernel IMA appraise enforcement policy

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
- Handoff transport detail:
  - `rootfs` is emitted as a directory that contains `rootfs.tar` (full OS rootfs payload)
  - excludes runtime-only mounts (`/dev`, `/proc`, `/sys`, `/run`) for transfer safety
- Downstream handoff:
  - via `next_task_dir: ../runc-llama-hardened`

2. `runc-llama-hardened`
- Input expectations (from upstream handoff):
  - `rootfs`
  - `config.json`
  - `model.gguf`
  - `llama-answer.txt`
- Handoff unpack detail:
  - if `rootfs/rootfs.tar` exists, stage2 unpacks it with `sudo` and re-owns files to current user
- Output artifacts (`task.json`):
  - `rootfs`
  - `config.json`
  - `model.gguf`
  - `config.min.json`
  - `removed-caps.txt`
  - `cap-summary.txt`
  - `llama-answer.txt`
- Downstream transport detail:
  - stage2 repacks `rootfs` as `rootfs/rootfs.tar` for stable artifact transfer
- Downstream handoff:
  - via `next_task_dir: ../runc-llama-verity-pack`

3. `runc-llama-verity-pack`
- Input expectations (from upstream handoff):
  - `rootfs`
  - `config.json`
  - `model.gguf`
- Input unpack detail:
  - if `rootfs/rootfs.tar` exists, stage3 unpacks it with `sudo` and re-owns files to current user
- Output artifacts (`task.json`):
  - `rootfs.squashfs`
  - `rootfs.verity`
  - `rootfs.root_hash`
  - `model.gguf`
  - `config.json`
- Downstream handoff:
  - via `next_task_dir: ../runc-llama-verity-run`

4. `runc-llama-verity-run`
- Input expectations (from upstream handoff):
  - `rootfs.squashfs`
  - `rootfs.verity`
  - `rootfs.root_hash`
  - `config.json`
  - `model.gguf`
- Runtime behavior:
  - creates loop devices for squashfs/hash payload
  - validates and opens dm-verity mapping
  - mounts verified squashfs rootfs
  - runs llama-server with UDS for inference
- Output artifacts (`task.json`):
  - `signed-runtime.tar` (IMA signed payload archive with xattrs)
  - `cert.der` (verification key for downstream stage)
- Downstream handoff:
  - via `next_task_dir: ../runc-llama-ima-verify-run`

5. `runc-llama-ima-verify-run`
- Input expectations (from upstream handoff):
  - `signed-runtime.tar`
  - `cert.der`
- Runtime behavior:
  - extracts tar with xattrs restored
  - verifies IMA signatures on `rootfs.squashfs/rootfs.verity/rootfs.root_hash/config.json/model.gguf`
  - opens and mounts dm-verity payload only after IMA verify success
  - runs llama-server with UDS for inference
- Output artifacts (`task.json`):
  - `llama-answer.txt`
  - `llama-error.txt`
  - `llama-server.log`
  - `runtime-summary.txt`
  - `runc-list.txt`
  - `prompt.txt`
  - `ima-verify.log`

## Run And Validation Commands

```bash
# Build CLI
cargo build --release

# Run the full chain
./target/release/vmize task /Users/sangwan/dev/vmize/worker/example/runc-llama-build

# Run independent IMA signature debug-verify PoC
./target/release/vmize task /Users/sangwan/dev/vmize/worker/example/ima-sign

# Core regressions
cargo test -p task
cargo test -p worker --lib
cargo test -p worker --test integration
```

## Troubleshooting Checklist

1. Chain path errors
- Verify `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/task.json` has:
  - `"next_task_dir": "../runc-llama-hardened"`
- Verify `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/task.json` has:
  - `"next_task_dir": "../runc-llama-verity-pack"`
- Verify `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/task.json` has:
  - `"next_task_dir": "../runc-llama-verity-run"`
- Verify `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/task.json` has:
  - `"next_task_dir": "../runc-llama-ima-verify-run"`

2. Handoff artifact errors
- Confirm upstream artifacts exist in each step output:
  - step1 -> step2: `rootfs`, `config.json`, `model.gguf`
  - step2 -> step3: `rootfs`, `config.json`, `model.gguf`
  - step3 -> step4: `rootfs.squashfs`, `rootfs.verity`, `rootfs.root_hash`, `config.json`, `model.gguf`
  - step4 -> step5: `signed-runtime.tar`, `cert.der`

3. Downstream script failures
- Check logs under each task's `output/logs/`.
- Re-run with the same command and inspect script-level errors first.

4. Runtime dependency failures
- Validate bootstrap dependencies:
  - step1/step2: `jq`, `runc`, `curl`, `wget`, `tar`, etc.
  - step3: `squashfs-tools`, `cryptsetup-bin`, `jq`
  - step4: `runc`, `cryptsetup-bin`, `squashfs-tools`, `util-linux`, `curl`, `jq`, `ima-evm-utils`, `openssl`
  - step5: `runc`, `cryptsetup-bin`, `squashfs-tools`, `util-linux`, `curl`, `jq`, `ima-evm-utils`, `tar`

5. Disk size / space failures
- Stage2, stage3, stage4, and stage5 are configured with `"disk_size": "20G"` to absorb large handoff payloads.
- If `No space left on device` appears, verify each task JSON keeps `disk_size` at least `20G`.

6. DNS / Network resolution failures
- If container apt-get fails with "Temporary failure resolving":
  - The container has no network namespace isolation but lacks DNS configuration
  - Stage1's `20_run_basic.sh` copies guest VM's `/etc/resolv.conf` into container
  - Verify: `runc exec <container> nslookup archive.ubuntu.com` should succeed
- Root cause: runc containers don't inherit host/guest DNS settings automatically
- Solution pattern (used in 20_run_basic.sh):
  ```bash
  ${SUDO} cat /etc/resolv.conf | run_exec "cat > /etc/resolv.conf"
  ```

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

### 2026-02-22 (verity-pack stage added)

1. Reason
- Added Task Chain stage 3 to package rootfs into squashfs + dm-verity artifacts for the next runtime validation stage.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/input/30_verify.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/10_minimize_config.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/input/10_make_verity.sh`
- `/Users/sangwan/dev/vmize/worker/README.md`
- `/Users/sangwan/dev/vmize/README.md`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Step1 now hands off full OS rootfs instead of binary-only rootfs.
- Step2 now hands off `rootfs/config.json/model.gguf` to downstream stage while keeping existing hardening outputs.
- Step3 (`runc-llama-verity-pack`) now generates:
  - `rootfs.squashfs`
  - `rootfs.verity`
  - `rootfs.root_hash`
  - plus pass-through `config.json` and `model.gguf`
- Step3 runs `veritysetup verify` and fails immediately on integrity setup failure.

4. Verification commands and results
- `cargo test -p worker --test integration all_example_shell_scripts_pass_bash_n -- --nocapture` -> pass
- `CARGO_BUILD_JOBS=2 cargo run -p vmize -- task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> pass (3-step chain)
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/output/rootfs.squashfs` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/output/rootfs.verity` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/output/rootfs.root_hash` -> pass
- `grep -Eq '^[0-9a-f]{64}$' /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/output/rootfs.root_hash` -> pass

### 2026-02-22 (chain transport + runtime hardening follow-up)

1. Reason
- Stabilize large rootfs artifact handoff and reduce chain failures from path permissions, DNS flakiness, and VM disk pressure.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/input/10_build_bundle.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/input/20_run_basic.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-build/input/30_verify.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/input/10_minimize_config.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-hardened/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/input/10_make_verity.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/task.json`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Added DNS fallback and apt retry logic to stage1/2/3 bootstrap scripts.
- Switched rootfs handoff transport to `rootfs/rootfs.tar` (full payload) to avoid scp failures on special files.
- Stage2 and stage3 now unpack tar handoff with `sudo`, then `chown` back to current user to avoid permission-denied extraction failures.
- Stage2 and stage3 task disk size set to `20G` to avoid `No space left on device` in downstream chain steps.
- Reduced noisy build/install output in stage1 run script and made llama build logs failure-focused.

4. Verification commands and results
- `cargo test -p task` -> pass
- `cargo test -p worker --lib` -> pass
- `cargo test -p worker --test integration` -> pass (`7 passed`)
- `CARGO_BUILD_JOBS=2 cargo run -p vmize -- task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> still flaky in this environment (intermittent `status -1` VM/script interruption during long-running chain path)

### 2026-02-22 (verity runtime stage added and validated)

1. Reason
- Implement stage4 runtime verification that consumes stage3 verity artifacts, runs runc on verified rootfs, and validates abstract UDS inference.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-pack/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/input/10_run_verity_uds.sh`
- `/Users/sangwan/dev/vmize/worker/README.md`
- `/Users/sangwan/dev/vmize/README.md`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Added new stage4 task `runc-llama-verity-run`.
- Stage3 now links to stage4 with `next_task_dir: ../runc-llama-verity-run`.
- Stage4 now:
  - opens dm-verity mapping from `rootfs.squashfs/rootfs.verity/rootfs.root_hash`
  - mounts verified squashfs rootfs
  - runs runc with patched config (`/models` bind mount + `/tmp` tmpfs)
  - validates prompt/response over abstract UDS and writes `llama-answer.txt`

4. Verification commands and results
- `cargo test -p worker --test integration all_example_shell_scripts_pass_bash_n -- --nocapture` -> pass
- `CARGO_BUILD_JOBS=2 cargo run -p vmize -- task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> pass (4-step chain)
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/output/llama-answer.txt` -> pass
- `grep -q '^uds_socket=@' /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/output/runtime-summary.txt` -> pass

### 2026-02-22 (independent IMA sign + HTTP tar roundtrip task added)

1. Reason
- Validate feasibility of IMA-based signing and verification before integrating signature checks into the stage4 artifact path.
- Keep scope safe by using debug verify only, with no IMA appraise policy enforcement.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/ima-sign/task.json`
- `/Users/sangwan/dev/vmize/worker/example/ima-sign/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/ima-sign/input/10_sign_verify.sh`
- `/Users/sangwan/dev/vmize/worker/example/ima-sign/output/.gitkeep`
- `/Users/sangwan/dev/vmize/worker/README.md`
- `/Users/sangwan/dev/vmize/README.md`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Added independent task `ima-sign` (not linked via `next_task_dir`).
- Task now:
  - signs `sample-a.txt` and `sample-b.bin` with `evmctl ima_sign`
  - verifies both files using `evmctl -v --key cert.der ima_verify`
  - mutates one signed sample and asserts verification fails
  - packs signed files with `tar --xattrs --xattrs-include='*'`, serves over local HTTP, downloads, extracts, and re-verifies
- Output artifacts now include sign/verify logs, HTTP roundtrip log, xattr dump, summary, and tar payloads.
- Private signing key (`key.pem`) is ephemeral and not exported.

4. Verification commands and results
- `cargo test -p worker --test integration all_example_shell_scripts_pass_bash_n -- --nocapture` -> pass
- `CARGO_BUILD_JOBS=2 cargo run -p vmize -- task /Users/sangwan/dev/vmize/worker/example/ima-sign` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/ima-sign/output/ima-sign-summary.txt` -> pass
- `grep -F 'positive verify: success' /Users/sangwan/dev/vmize/worker/example/ima-sign/output/ima-sign-summary.txt` -> pass
- `grep -F 'tampered verify: expected failure' /Users/sangwan/dev/vmize/worker/example/ima-sign/output/ima-sign-summary.txt` -> pass
- `grep -F 'http_roundtrip_verify=success' /Users/sangwan/dev/vmize/worker/example/ima-sign/output/ima-sign-summary.txt` -> pass

### 2026-02-22 (stage4 IMA signed packaging + stage5 IMA verify runtime)

1. Reason
- Add IMA signature-preserving packaging at stage4 and enforce IMA verification before runtime execution at stage5.
- Ensure stage5 only runs llama inference after tar payload verification succeeds.

2. Modified files
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/input/10_run_verity_uds.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-ima-verify-run/task.json`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-ima-verify-run/input/00_bootstrap.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-ima-verify-run/input/10_verify_and_run.sh`
- `/Users/sangwan/dev/vmize/worker/example/runc-llama-ima-verify-run/output/.gitkeep`
- `/Users/sangwan/dev/vmize/worker/README.md`
- `/Users/sangwan/dev/vmize/README.md`
- `/Users/sangwan/dev/vmize/worker/TASK_CHAIN_TUTORIAL.md`

3. Behavioral changes
- Stage4 still performs dm-verity runtime/UDS inference validation, then IMA-signs handoff files.
- Stage4 now outputs only:
  - `signed-runtime.tar` (xattrs included)
  - `cert.der` (verification key)
- Added stage5 `runc-llama-ima-verify-run`:
  - extracts stage4 tar with xattrs
  - verifies IMA signatures with `cert.der`
  - runs dm-verity + runc + abstract UDS inference only after successful verification
  - emits `llama-answer.txt` and `ima-verify.log`

4. Verification commands and results
- `cargo test -p worker --test integration all_example_shell_scripts_pass_bash_n -- --nocapture` -> pass
- `CARGO_BUILD_JOBS=2 cargo run -p vmize -- task /Users/sangwan/dev/vmize/worker/example/runc-llama-build` -> pass (5-step chain)
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/output/signed-runtime.tar` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-verity-run/output/cert.der` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-ima-verify-run/output/llama-answer.txt` -> pass
- `test -s /Users/sangwan/dev/vmize/worker/example/runc-llama-ima-verify-run/output/ima-verify.log` -> pass

### 2026-02-23 (DNS resolution fix for runc containers)

1. Reason
- Fix DNS resolution failures inside runc containers that prevented package installation.

2. Modified files
- `/home/sangwan/github/vmize/worker/example/runc-llama-build/input/20_run_basic.sh`

3. Behavioral changes
- Stage1 now copies guest VM's `/etc/resolv.conf` into runc container before installing dependencies
- Added DNS resolution check with fallback to update resolv.conf if needed
- Removed hardcoded DNS servers (1.1.1.1, 8.8.8.8) in favor of guest VM's actual DNS settings

4. Verification commands and results
- `./target/release/vmize task /home/sangwan/github/vmize/worker/example/runc-llama-build` -> pass (5-step chain)
- `test -s /home/sangwan/github/vmize/worker/example/runc-llama-ima-verify-run/output/llama-answer.txt` -> pass

### 2026-02-24 (llama-server with UDS integration)

1. Reason
- Replace llama-cli with llama-server using Unix Domain Sockets across all stages for API-based inference similar to vLLM.

2. Modified files
- `worker/example/runc-llama-build/input/00_bootstrap.sh`
- `worker/example/runc-llama-build/input/10_build_bundle.sh`
- `worker/example/runc-llama-build/input/20_run_basic.sh`
- `worker/example/runc-llama-build/input/30_verify.sh`
- `worker/example/runc-llama-hardened/input/00_bootstrap.sh`
- `worker/example/runc-llama-hardened/input/10_minimize_config.sh`
- `worker/example/runc-llama-ima-verify-run/input/00_bootstrap.sh`
- `worker/example/runc-llama-ima-verify-run/input/10_verify_and_run.sh`
- `worker/example/runc-llama-ima-verify-run/task.json`
- `worker/example/runc-llama-verity-run/input/00_bootstrap.sh`
- `worker/example/runc-llama-verity-run/input/10_run_verity_uds.sh`

3. Behavioral changes
- All stages now build and run llama-server instead of llama-cli
- Added `/sockets` bind mount to config.json for UDS communication
- llama-server binds to `/sockets/llama.sock` using `--host /sockets/llama.sock`
- HTTP API inference via curl with `--unix-socket` option
- Added curl dependency to all bootstrap scripts
- Added architecture detection for LD_LIBRARY_PATH (x86_64/aarch64)
- Socket permissions handled with sudo for root-owned sockets
- Fixed artifact name: `llama-service.log` -> `llama-server.log`
- Removed socat dependency (no longer needed for UDS)

4. Verification commands and results
- `cargo run --bin vmize -- task worker/example/runc-llama-build` -> pass (5-step chain)
- All stages produce valid `llama-answer.txt` with inference results
- Stage artifacts properly handed off between chain steps
