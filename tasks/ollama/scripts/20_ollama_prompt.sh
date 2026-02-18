#!/usr/bin/env bash
set -euo pipefail

MODEL="${OLLAMA_MODEL:-qwen2.5:0.5b}"
PROMPT="${OLLAMA_PROMPT:-Tell me a single concise sentence in English describing this VM run.}"

# Detect ollama binary path without `command -v`.
OLLAMA_BIN=""
if [ -x /usr/local/bin/ollama ]; then
    OLLAMA_BIN=/usr/local/bin/ollama
elif [ -x /usr/bin/ollama ]; then
    OLLAMA_BIN=/usr/bin/ollama
else
    echo "ollama not found in VM, installing..." >&2
    # zstd is required by the ollama installer for extraction
    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get update -qq && sudo apt-get install -y -qq zstd >&2
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y zstd >&2
    fi
    if [ -x /usr/bin/curl ]; then
        curl -fsSL https://ollama.com/install.sh | sh
    elif [ -x /usr/bin/wget ]; then
        wget -qO- https://ollama.com/install.sh | sh
    else
        echo "Neither curl nor wget is available to install ollama." >&2
        exit 1
    fi

    if [ -x /usr/local/bin/ollama ]; then
        OLLAMA_BIN=/usr/local/bin/ollama
    elif [ -x /usr/bin/ollama ]; then
        OLLAMA_BIN=/usr/bin/ollama
    else
        echo "ollama install finished but binary not found." >&2
        exit 2
    fi
fi

if ! "$OLLAMA_BIN" list | awk 'NR > 1 {print $1}' | grep -Fxq "$MODEL"; then
    echo "ollama model not found, pulling: $MODEL" >&2
    if ! "$OLLAMA_BIN" pull "$MODEL"; then
        echo "failed to pull model: $MODEL" >&2
        exit 2
    fi
fi

if ! "$OLLAMA_BIN" list | awk 'NR > 1 {print $1}' | grep -Fxq "$MODEL"; then
    echo "ollama model still not installed after pull: $MODEL" >&2
    exit 2
fi

if ! timeout 120 "$OLLAMA_BIN" run "$MODEL" "$PROMPT" > /tmp/batch/out/ollama-answer.txt 2> /tmp/batch/out/ollama-error.txt; then
    echo "ollama run failed for model: $MODEL" >&2
    if [ -s /tmp/batch/out/ollama-error.txt ]; then
        cat /tmp/batch/out/ollama-error.txt >&2
    fi
    exit 3
fi
