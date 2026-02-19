#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vmize-worker/work/bundle"

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

if ! command -v apt-get >/dev/null 2>&1; then
    echo "[ERROR] This bootstrap script currently supports apt-based Ubuntu systems only."
    exit 1
fi

echo "[*] Updating package metadata"
${SUDO} apt-get update

echo "[*] Installing required packages"
${SUDO} apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    tar \
    jq \
    runc \
    xz-utils

mkdir -p "${BUNDLE_DIR}/artifacts"
echo "[+] Bootstrap complete"
