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

OUTPUT_ROOTFS="${OUT_DIR}/rootfs"
OUTPUT_CHAIN_CONFIG="${OUT_DIR}/config.json"
OUTPUT_CHAIN_MODEL="${OUT_DIR}/model.gguf"
OUTPUT_CONFIG="${OUT_DIR}/config.min.json"
OUTPUT_REMOVED="${OUT_DIR}/removed-caps.txt"
OUTPUT_SUMMARY="${OUT_DIR}/cap-summary.txt"
OUTPUT_ANSWER="${OUT_DIR}/llama-answer.txt"
OUTPUT_ERROR="${OUT_DIR}/llama-error.txt"
PROMPT_TEXT="${LLAMA_PROMPT:-Say in one short sentence that hardened runc llama stage works.}"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
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

if [[ ! -x "${ACTIVE_ROOTFS}/opt/llama.cpp/build/bin/llama-cli" ]]; then
    echo "[ERROR] rootfs handoff is missing llama-cli binary"
    exit 1
fi

if [[ ! -e "${ACTIVE_ROOTFS}/opt/llama.cpp/build/bin/libllama.so.0" ]]; then
    echo "[ERROR] rootfs handoff is missing libllama.so.0"
    exit 1
fi

if [[ ! -f "${INPUT_CONFIG}" ]]; then
    echo "[ERROR] missing config handoff at ${INPUT_CONFIG}"
    exit 1
fi

if [[ ! -s "${INPUT_MODEL}" ]]; then
    echo "[ERROR] missing model handoff at ${INPUT_MODEL}"
    exit 1
fi

DROP_CAPS=(
    "CAP_AUDIT_WRITE"
    "CAP_KILL"
    "CAP_MKNOD"
    "CAP_NET_RAW"
    "CAP_SETFCAP"
)

drop_caps_json="$(printf '%s\n' "${DROP_CAPS[@]}" | jq -R . | jq -s .)"

jq \
    --argjson drop "${drop_caps_json}" \
    'def trim_caps($drop):
        (. // [])
        | map(select(($drop | index(.)) | not))
        | unique;
     .process.capabilities.bounding = trim_caps($drop) |
     .process.capabilities.effective = trim_caps($drop) |
     .process.capabilities.permitted = trim_caps($drop)' \
    "${INPUT_CONFIG}" > "${OUTPUT_CONFIG}"

for cap in "${DROP_CAPS[@]}"; do
    if jq -e --arg cap "${cap}" '
        (
            .process.capabilities.bounding +
            .process.capabilities.effective +
            .process.capabilities.permitted
        ) | index($cap) != null
    ' "${OUTPUT_CONFIG}" >/dev/null; then
        echo "[ERROR] failed to remove ${cap} from minimized config"
        exit 1
    fi
done

printf '%s\n' "${DROP_CAPS[@]}" > "${OUTPUT_REMOVED}"

{
    echo "input_rootfs=${ACTIVE_ROOTFS}"
    echo "input_model=${INPUT_MODEL}"
    echo "caps_removed=$(wc -l < "${OUTPUT_REMOVED}")"
    echo "bounding_before=$(jq '.process.capabilities.bounding | length' "${INPUT_CONFIG}")"
    echo "bounding_after=$(jq '.process.capabilities.bounding | length' "${OUTPUT_CONFIG}")"
    echo "effective_after=$(jq '.process.capabilities.effective | length' "${OUTPUT_CONFIG}")"
    echo "permitted_after=$(jq '.process.capabilities.permitted | length' "${OUTPUT_CONFIG}")"
} > "${OUTPUT_SUMMARY}"

RUNTIME_LD_LIBRARY_PATH="${ACTIVE_ROOTFS}/opt/llama.cpp/build/bin:${ACTIVE_ROOTFS}/usr/lib/aarch64-linux-gnu:${ACTIVE_ROOTFS}/lib/aarch64-linux-gnu:${ACTIVE_ROOTFS}/usr/lib:${ACTIVE_ROOTFS}/lib"
if [[ -n "${LD_LIBRARY_PATH:-}" ]]; then
    RUNTIME_LD_LIBRARY_PATH="${RUNTIME_LD_LIBRARY_PATH}:${LD_LIBRARY_PATH}"
fi

set +e
env LD_LIBRARY_PATH="${RUNTIME_LD_LIBRARY_PATH}" \
    "${ACTIVE_ROOTFS}/opt/llama.cpp/build/bin/llama-cli" \
    -m "${INPUT_MODEL}" \
    -p "${PROMPT_TEXT}" \
    -n 48 \
    --temp 0.2 \
    --seed 42 \
    --single-turn \
    --simple-io \
    > "${OUTPUT_ANSWER}" \
    2> "${OUTPUT_ERROR}"
llama_status=$?
set -e

if [[ ${llama_status} -ne 0 ]] || ! grep -q '[[:alnum:]]' "${OUTPUT_ANSWER}"; then
    echo "[ERROR] hardened direct llama run failed (exit=${llama_status})"
    if [[ -s "${OUTPUT_ERROR}" ]]; then
        cat "${OUTPUT_ERROR}" >&2
    fi
    exit 1
fi

echo "llama_hardened_prompt=direct_success" >> "${OUTPUT_SUMMARY}"
echo "llama_hardened_exit=${llama_status}" >> "${OUTPUT_SUMMARY}"

rm -rf "${OUTPUT_ROOTFS}"
mkdir -p "${OUTPUT_ROOTFS}"
${SUDO} tar \
    --exclude='./dev/*' \
    --exclude='./proc/*' \
    --exclude='./sys/*' \
    --exclude='./run/*' \
    -C "${ACTIVE_ROOTFS}" \
    -cf "${OUTPUT_ROOTFS}/rootfs.tar" \
    .
${SUDO} chown "$(id -u):$(id -g)" "${OUTPUT_ROOTFS}/rootfs.tar"
cp -f "${OUTPUT_CONFIG}" "${OUTPUT_CHAIN_CONFIG}"
cp -f "${INPUT_MODEL}" "${OUTPUT_CHAIN_MODEL}"

echo "[+] Minimized OCI config written to ${OUTPUT_CONFIG}"
