#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="basic"
BUNDLE_DIR="/tmp/vm-batch/work/bundle"

command -v runc >/dev/null 2>&1 || { echo "[ERROR] runc not found"; exit 1; }

if [ ! -f "${BUNDLE_DIR}/config.json" ]; then
    echo "[ERROR] config.json not found at ${BUNDLE_DIR}/config.json"
    exit 1
fi

if [ ! -d "${BUNDLE_DIR}/rootfs" ]; then
    echo "[ERROR] rootfs directory not found at ${BUNDLE_DIR}/rootfs"
    exit 1
fi

cleanup() {
    echo "[*] Cleaning up container: ${CONTAINER_NAME}"
    sudo runc delete -f "${CONTAINER_NAME}" 2>/dev/null || true
}
trap cleanup EXIT

echo "[*] Starting container in detached mode"
(
    cd "${BUNDLE_DIR}"
    sudo runc delete -f "${CONTAINER_NAME}" 2>/dev/null || true
    sudo runc run -d "${CONTAINER_NAME}"
)

if sudo runc list | awk 'NR>1 {print $1}' | grep -Fxq "${CONTAINER_NAME}"; then
    echo "[+] Container is running: ${CONTAINER_NAME}"
else
    echo "[ERROR] Container did not start correctly."
    exit 1
fi

# Capture runc list output for vm-batch
sudo runc list > /tmp/vm-batch/out/runc-list.txt
