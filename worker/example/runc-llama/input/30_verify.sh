#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
BUNDLE_ENV="${ARTIFACT_DIR}/bundle.env"
ROOTFS_DIR="${BUNDLE_DIR}/rootfs"
CONFIG_PATH="${BUNDLE_DIR}/config.json"
HANDOFF_ROOTFS="/tmp/vmize-worker/out/rootfs"
HANDOFF_CONFIG="/tmp/vmize-worker/out/config.json"
HANDOFF_MODEL="/tmp/vmize-worker/out/model.gguf"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }

if [[ ! -f "${BUNDLE_ENV}" ]]; then
    echo "[ERROR] bundle.env not found at ${BUNDLE_ENV}"
    exit 1
fi

# shellcheck source=/dev/null
source "${BUNDLE_ENV}"

MODEL_FILE="${MODEL_FILE:?MODEL_FILE missing in bundle.env}"
MODEL_PATH="${MODEL_PATH:?MODEL_PATH missing in bundle.env}"

if [[ ! -f "${CONFIG_PATH}" ]]; then
    echo "[ERROR] config.json not found at ${CONFIG_PATH}"
    exit 1
fi

if [[ ! -d "${ROOTFS_DIR}" ]]; then
    echo "[ERROR] rootfs not found at ${ROOTFS_DIR}"
    exit 1
fi

if [[ ! -f "${MODEL_PATH}" ]]; then
    echo "[ERROR] model file not found at ${MODEL_PATH}"
    exit 1
fi

if [[ ! -x "${ROOTFS_DIR}/opt/llama.cpp/build/bin/llama-cli" ]]; then
    echo "[ERROR] llama-cli binary not found in rootfs at /opt/llama.cpp/build/bin/llama-cli"
    exit 1
fi

if [[ ! -s "/tmp/vmize-worker/out/llama-answer.txt" ]]; then
    echo "[ERROR] llama-answer.txt missing or empty"
    exit 1
fi

echo "[*] Validating config JSON"
jq empty "${CONFIG_PATH}" >/dev/null

echo "[*] Staging artifacts for chained task"
rm -rf "${HANDOFF_ROOTFS}"
mkdir -p "${HANDOFF_ROOTFS}/opt/llama.cpp/build/bin"
cp "${ROOTFS_DIR}/opt/llama.cpp/build/bin/llama-cli" \
   "${HANDOFF_ROOTFS}/opt/llama.cpp/build/bin/llama-cli"
printf '%s\n' "${ROOTFS_DIR}" > "${HANDOFF_ROOTFS}/ROOTFS_SOURCE.txt"

cp "${CONFIG_PATH}" "${HANDOFF_CONFIG}"
cp "${MODEL_PATH}" "${HANDOFF_MODEL}"

if [[ ! -x "${HANDOFF_ROOTFS}/opt/llama.cpp/build/bin/llama-cli" ]]; then
    echo "[ERROR] handoff rootfs is missing /opt/llama.cpp/build/bin/llama-cli"
    exit 1
fi

if [[ ! -s "${HANDOFF_MODEL}" ]]; then
    echo "[ERROR] handoff model is missing or empty at ${HANDOFF_MODEL}"
    exit 1
fi

echo "[+] Verification complete"
