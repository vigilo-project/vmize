# Tasks

This directory contains task definitions for vmize.

## runc-llama

Run Ubuntu minimal OCI via runc, build and run llama.cpp with mounted GGUF model, and export replay bundle.

### Prerequisites

The task requires a GGUF model file. If you're behind a corporate firewall, download the model externally first.

### Quick Start (with local model)

```bash
# If model already exists locally
LOCAL_MODEL_PATH=/path/to/model.gguf cargo run --release -p vmize -- task tasks/runc-llama
```

### Firewall-Restricted Environment

For corporate networks that block Hugging Face:

**Step 1: Download model externally (home/hotspot/VPN)**

```bash
wget -O qwen2.5-0.5b-instruct-q4_0.gguf \
  "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_0.gguf"
```

Transfer to company network and place at:
```
~/models/qwen2.5-0.5b-instruct-q4_0.gguf
```

**Step 2: Run task**

```bash
LOCAL_MODEL_PATH=~/models/qwen2.5-0.5b-instruct-q4_0.gguf \
  cargo run --release -p vmize -- task tasks/runc-llama
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LOCAL_MODEL_PATH` | - | Local .gguf file path (skips download) |
| `LLAMA_PROMPT` | "Say in one short sentence..." | Inference prompt |

### Expected Duration

- VM startup: ~1 min
- Ubuntu rootfs download: ~2 min
- llama.cpp build: ~5 min (inside VM)
- Inference: ~30 sec
- **Total: ~10 min**

### Output Files

```
tasks/runc-llama/output/
├── llama-answer.txt         # Model response
├── llama-version.txt        # llama.cpp version info
├── config.json              # OCI bundle config
├── runc-llama-replay.tar.xz # Replayable bundle (~800MB)
└── *.log                    # Detailed execution logs
```
