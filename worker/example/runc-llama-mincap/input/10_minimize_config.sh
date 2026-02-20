#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUT_DIR="/tmp/vmize-worker/out"

INPUT_ROOTFS="${WORK_DIR}/rootfs"
INPUT_CONFIG="${WORK_DIR}/config.json"
INPUT_MODEL="${WORK_DIR}/model.gguf"

OUTPUT_CONFIG="${OUT_DIR}/config.min.json"
OUTPUT_REMOVED="${OUT_DIR}/removed-caps.txt"
OUTPUT_SUMMARY="${OUT_DIR}/cap-summary.txt"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }

if [[ ! -d "${INPUT_ROOTFS}" ]]; then
    echo "[ERROR] missing rootfs handoff at ${INPUT_ROOTFS}"
    exit 1
fi

if [[ ! -x "${INPUT_ROOTFS}/opt/llama.cpp/build/bin/llama-cli" ]]; then
    echo "[ERROR] rootfs handoff is missing llama-cli binary"
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
    echo "input_rootfs=${INPUT_ROOTFS}"
    echo "input_model=${INPUT_MODEL}"
    echo "caps_removed=$(wc -l < "${OUTPUT_REMOVED}")"
    echo "bounding_before=$(jq '.process.capabilities.bounding | length' "${INPUT_CONFIG}")"
    echo "bounding_after=$(jq '.process.capabilities.bounding | length' "${OUTPUT_CONFIG}")"
    echo "effective_after=$(jq '.process.capabilities.effective | length' "${OUTPUT_CONFIG}")"
    echo "permitted_after=$(jq '.process.capabilities.permitted | length' "${OUTPUT_CONFIG}")"
} > "${OUTPUT_SUMMARY}"

echo "[+] Minimized OCI config written to ${OUTPUT_CONFIG}"
