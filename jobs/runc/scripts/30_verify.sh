#!/usr/bin/env bash
set -euo pipefail

ARTIFACT_DIR="/tmp/vm-batch/work/bundle/artifacts"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }

CONFIG_PATH="${ARTIFACT_DIR}/config.json"
ROOTFS_TAR="${ARTIFACT_DIR}/rootfs.tar.xz"

if [ ! -f "${CONFIG_PATH}" ]; then
    echo "[ERROR] config.json not found at ${CONFIG_PATH}"
    exit 1
fi

if [ ! -f "${ROOTFS_TAR}" ]; then
    echo "[ERROR] rootfs.tar.xz not found at ${ROOTFS_TAR}"
    exit 1
fi

echo "[*] Verifying JSON format: ${CONFIG_PATH}"
jq empty "${CONFIG_PATH}" >/dev/null

echo "[*] Verifying tar.xz archive: ${ROOTFS_TAR}"
tar -tJf "${ROOTFS_TAR}" >/dev/null

if [ ! -s "${ROOTFS_TAR}" ]; then
    echo "[ERROR] rootfs.tar.xz is empty"
    exit 1
fi

echo "[+] Bundle verification passed"
echo "    ${CONFIG_PATH}"
echo "    ${ROOTFS_TAR}"

# Write verification result to vm-batch output
{
    echo "[+] Bundle verification passed"
    echo "    config.json: OK"
    echo "    rootfs.tar.xz: OK"
} > /tmp/vm-batch/out/verify.log
