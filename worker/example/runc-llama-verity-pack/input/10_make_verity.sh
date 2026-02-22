#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUT_DIR="/tmp/vmize-worker/out"

INPUT_ROOTFS="${WORK_DIR}/rootfs"
INPUT_ROOTFS_TAR="${INPUT_ROOTFS}/rootfs.tar"
INPUT_CONFIG="${WORK_DIR}/config.json"
INPUT_MODEL="${WORK_DIR}/model.gguf"
UNPACKED_ROOTFS="${WORK_DIR}/rootfs.unpacked"
ACTIVE_ROOTFS="${INPUT_ROOTFS}"

OUTPUT_SQUASHFS="${OUT_DIR}/rootfs.squashfs"
OUTPUT_VERITY="${OUT_DIR}/rootfs.verity"
OUTPUT_ROOT_HASH="${OUT_DIR}/rootfs.root_hash"
OUTPUT_MODEL="${OUT_DIR}/model.gguf"
OUTPUT_CONFIG="${OUT_DIR}/config.json"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
command -v mksquashfs >/dev/null 2>&1 || { echo "[ERROR] mksquashfs not found"; exit 1; }
command -v veritysetup >/dev/null 2>&1 || { echo "[ERROR] veritysetup not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

if [[ ! -d "${INPUT_ROOTFS}" ]]; then
    echo "[ERROR] missing rootfs handoff at ${INPUT_ROOTFS}"
    exit 1
fi

if [[ -f "${INPUT_ROOTFS_TAR}" ]]; then
    rm -rf "${UNPACKED_ROOTFS}"
    mkdir -p "${UNPACKED_ROOTFS}"
    ${SUDO} tar -xf "${INPUT_ROOTFS_TAR}" -C "${UNPACKED_ROOTFS}"
    ${SUDO} chown -R "$(id -u):$(id -g)" "${UNPACKED_ROOTFS}"
    ACTIVE_ROOTFS="${UNPACKED_ROOTFS}"
fi

if [[ ! -f "${INPUT_CONFIG}" ]]; then
    echo "[ERROR] missing config handoff at ${INPUT_CONFIG}"
    exit 1
fi

if [[ ! -s "${INPUT_MODEL}" ]]; then
    echo "[ERROR] missing model handoff at ${INPUT_MODEL}"
    exit 1
fi

jq empty "${INPUT_CONFIG}" >/dev/null

rm -f "${OUTPUT_SQUASHFS}" "${OUTPUT_VERITY}" "${OUTPUT_ROOT_HASH}"

echo "[*] Building squashfs from handed-off rootfs"
mksquashfs "${ACTIVE_ROOTFS}" "${OUTPUT_SQUASHFS}" -noappend -comp xz

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

cp -f "${INPUT_CONFIG}" "${OUTPUT_CONFIG}"
cp -f "${INPUT_MODEL}" "${OUTPUT_MODEL}"

echo "[+] dm-verity artifacts ready"
echo "    squashfs: ${OUTPUT_SQUASHFS}"
echo "    verity:   ${OUTPUT_VERITY}"
echo "    root hash:${OUTPUT_ROOT_HASH}"
