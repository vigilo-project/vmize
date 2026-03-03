# worker

`worker` is VMize's task execution engine.
It runs task directories in ephemeral VMs so each workload is isolated.
`worker` is a library crate; use the workspace `vmize` CLI to execute tasks.

Built on top of [`vm`](https://github.com/vigilo-project/vm), `worker` defines a task as:
- `task.json` — task definition with explicit `commands` list
- `input/` — scripts and assets to copy into the VM
- `output/` — collected output files and per-command logs

Shared curated tasks in this workspace live in `./example/`.
For chain-oriented development notes and change history, see [`TASK_CHAIN_TUTORIAL.md`](./TASK_CHAIN_TUTORIAL.md).

## Minimum Goal
`worker` is considered healthy when it can:
1. Load and run valid task directories.
2. Collect output artifacts back to host `output/`.
3. Exit non-zero on task/script failures.
4. Enforce `--batch` max of 4 tasks.

## Acceptance Checklist
- Single-task run works end-to-end.
- Multi-task run works sequentially.
- `--batch` accepts up to 4 tasks and rejects the 5th.
- Output files are present after successful runs.
- Per-command logs appear in `output/logs/`.

## Quick Start

```bash
cargo build --release

# Single task
./target/release/vmize task worker/example/task1

# Multiple tasks (sequential)
./target/release/vmize task worker/example/task1 worker/example/task2

# Concurrent (max 4)
./target/release/vmize task --batch \
  worker/example/split-task1 \
  worker/example/split-task2 \
  worker/example/split-task3 \
  worker/example/split-task4

# Shared tasks
./target/release/vmize task example/runc
./target/release/vmize task example/runc-llama-build
./target/release/vmize task worker/example/ima-sign
./target/release/vmize task worker/example/kernel-build
./target/release/vmize task worker/example/rootfs-build

# runc-llama prerequisites (required before running the chain)
#   1) build rootfs image
./target/release/vmize task worker/example/rootfs-build
#   2) build kernel image
./target/release/vmize task worker/example/kernel-build
#
# The runc-llama tasks use custom boot files expected at:
#   image/rootfs.qcow2, image/bzImage, and image/kernel.config
#
# kernel-build enables required kernel options for this chain, including:
#   USER_NS, CGROUP_BPF, CGROUP_DEVICE, BLK_DEV_DM, DM_VERITY, BLK_DEV_LOOP, CRYPTO_SHA256
# and exports kernel.config for task preflight checks.
#
# runc-llama stage tasks are configured with disk_size=20G to avoid qcow2 resize
# failures when handing off large rootfs artifacts.
#
# runc-llama Task Chain:
#   step1: runc-llama-build (llama-server with UDS inference)
#   step2: runc-llama-hardened (hardened config + llama-server UDS)
#   step3: runc-llama-verity-pack (squashfs + dm-verity artifact packaging)
#   step4: runc-llama-verity-run (runtime UDS check + IMA-sign + xattr tar packaging)
#   step5: runc-llama-ima-verify-run (verify tar+cert with IMA, then run UDS inference)
./target/release/vmize task worker/example/runc-llama-build
```

Per-task kernel config requirements for this chain are documented in
[`TASK_CHAIN_TUTORIAL.md`](./TASK_CHAIN_TUTORIAL.md) under
`Per-Task Kernel Config Requirements`.

`worker/example/runc-llama-hardened` expects `rootfs`, `config.json`, and `model.gguf`
from `runc-llama-build`, hands off to `runc-llama-verity-pack`, and then into
`runc-llama-verity-run`, and finally to `runc-llama-ima-verify-run` for IMA-verified replay.

`worker/example/ima-sign` is an independent debug-verify task that checks
`ima_sign` + `ima_verify`, plus tar+HTTP roundtrip preservation of `security.ima`
without enabling kernel appraise policy.

## Task Directory Structure

```text
<task-dir>/
├── task.json
├── input/          # Scripts and binary assets
└── output/
    └── logs/       # Per-command stdout/stderr (auto-created)
```

Example `task.json`:

```json
{
  "name": "my-task",
  "description": "Build and test in an ephemeral VM",
  "disk_size": "20G",
  "vm": {
    "boot": "ubuntu"
  },
  "commands": ["00_setup.sh", "10_run.sh"],
  "artifacts": ["result.txt"]
}
```

`commands` — ordered list of files in `input/` to execute inside the VM.
`artifacts` — expected output files; if omitted, all of `/tmp/vmize-worker/out/` is copied back.
`vm.boot` — VM boot mode:
- `ubuntu` (default): use VMize host-profile Ubuntu cloud image flow.
- `custom`: use explicit `vm.kernel` + `vm.rootfs`.
- `cloud` is accepted as an alias of `ubuntu` for backward compatibility.

For custom boot tasks:

```json
{
  "disk_size": "12G",
  "vm": {
    "boot": "custom",
    "kernel": "../../../image/bzImage",
    "rootfs": "../../../image/rootfs.qcow2",
    "kernel_config": "../../../image/kernel.config",
    "required_kernel_config": [
      "CONFIG_DM_VERITY=y",
      "CONFIG_CRYPTO_SHA256=y"
    ],
    "clone_rootfs": true
  }
}
```

`vm.kernel`/`vm.rootfs` paths are resolved relative to the task directory (absolute paths are also allowed).
`vm.kernel_config` points to a kernel `.config`-style text file used for static preflight checks.
`vm.required_kernel_config` declares required options in `CONFIG_FOO=y|m|n` format.
When `clone_rootfs` is `true` (default), worker runs each task step on a temporary rootfs copy.
With custom boot, `disk_size` is applied by resizing that temporary copy.
When `vm.required_kernel_config` is set, worker checks both:
- static preflight on host (`vm.kernel_config`) before VM boot.
- runtime probe in guest (`/proc/config.gz` or `/boot/config-$(uname -r)`) before running scripts.

## Verification Commands

```bash
cargo test -p worker --lib     # unit tests only (no QEMU)
cargo test -p worker           # all tests (integration requires QEMU)

# Optional integration paths
BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p worker -- --nocapture
```
