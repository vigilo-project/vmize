#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
ROOTFS_DIR="${BUNDLE_DIR}/rootfs"
CONFIG_PATH="${BUNDLE_DIR}/config.json"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
BUNDLE_ENV="${ARTIFACT_DIR}/bundle.env"
OUT_DIR="/tmp/vmize-worker/out"

OUTPUT_SQUASHFS="${OUT_DIR}/rootfs.squashfs"
OUTPUT_VERITY="${OUT_DIR}/rootfs.verity"
OUTPUT_ROOT_HASH="${OUT_DIR}/rootfs.root_hash"
OUTPUT_CONFIG="${OUT_DIR}/config.json"
OUTPUT_MODEL="${OUT_DIR}/model.gguf"

command -v mksquashfs >/dev/null 2>&1 || { echo "[ERROR] mksquashfs not found"; exit 1; }
command -v veritysetup >/dev/null 2>&1 || { echo "[ERROR] veritysetup not found"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }

if [[ ! -d "${ROOTFS_DIR}" ]]; then
    echo "[ERROR] rootfs not found at ${ROOTFS_DIR}"
    exit 1
fi

if [[ ! -f "${CONFIG_PATH}" ]]; then
    echo "[ERROR] config.json not found at ${CONFIG_PATH}"
    exit 1
fi

if [[ ! -f "${BUNDLE_ENV}" ]]; then
    echo "[ERROR] bundle.env not found at ${BUNDLE_ENV}"
    exit 1
fi

# shellcheck source=/dev/null
source "${BUNDLE_ENV}"

MODEL_FILE="${MODEL_FILE:?MODEL_FILE missing in bundle.env}"
MODEL_PATH="${MODEL_PATH:?MODEL_PATH missing in bundle.env}"

if [[ ! -s "${MODEL_PATH}" ]]; then
    echo "[ERROR] model file not found or empty at ${MODEL_PATH}"
    exit 1
fi

jq empty "${CONFIG_PATH}" >/dev/null

rm -f "${OUTPUT_SQUASHFS}" "${OUTPUT_VERITY}" "${OUTPUT_ROOT_HASH}"

echo "[*] Building squashfs from rootfs"
mksquashfs "${ROOTFS_DIR}" "${OUTPUT_SQUASHFS}" -noappend -comp xz

echo "[*] Generating dm-verity metadata"
format_output="$(veritysetup format "${OUTPUT_SQUASHFS}" "${OUTPUT_VERITY}" 2>&1)"
printf '%s\n' "${format_output}"

root_hash="$(
    printf '%s\n' "${format_output}" \
    | awk -F': ' '/Root hash:/ {print $2}' \
    | tail -n 1 \
    | tr 'A-F' 'a-f' \
    | tr -d '[:space:]'
)"

if ! printf '%s\n' "${root_hash}" | grep -Eq '^[0-9a-f]{64}$'; then
    echo "[ERROR] invalid root hash extracted from veritysetup output: '${root_hash}'"
    exit 1
fi

printf '%s\n' "${root_hash}" > "${OUTPUT_ROOT_HASH}"

echo "[*] Verifying dm-verity metadata"
veritysetup verify "${OUTPUT_SQUASHFS}" "${OUTPUT_VERITY}" "${root_hash}"

cp -f "${CONFIG_PATH}" "${OUTPUT_CONFIG}"
cp -f "${MODEL_PATH}" "${OUTPUT_MODEL}"

echo "[+] dm-verity artifacts ready"
echo "    squashfs:  ${OUTPUT_SQUASHFS}"
echo "    verity:    ${OUTPUT_VERITY}"
echo "    root hash: ${OUTPUT_ROOT_HASH}"
echo "    config:    ${OUTPUT_CONFIG}"
echo "    model:     ${OUTPUT_MODEL}"
