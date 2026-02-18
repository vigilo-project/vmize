#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/batch/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
MODELS_DIR="${BUNDLE_DIR}/models"

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

if ! command -v apt-get >/dev/null 2>&1; then
    echo "[ERROR] This script currently supports apt-based Ubuntu systems only."
    exit 1
fi

echo "[*] Updating package metadata"
${SUDO} apt-get update

echo "[*] Installing host dependencies for runc workflow"
${SUDO} apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    jq \
    runc \
    tar \
    wget \
    xz-utils

mkdir -p "${ARTIFACT_DIR}" "${MODELS_DIR}"
echo "[+] Bootstrap complete"
