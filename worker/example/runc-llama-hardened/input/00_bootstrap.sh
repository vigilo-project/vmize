#!/usr/bin/env bash
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

if ! command -v apt-get >/dev/null 2>&1; then
    echo "[ERROR] This script currently supports apt-based Ubuntu systems only."
    exit 1
fi

${SUDO} apt-get update
${SUDO} apt-get install -y --no-install-recommends jq libgomp1 libstdc++6

echo "[+] runc-llama-hardened bootstrap complete"
