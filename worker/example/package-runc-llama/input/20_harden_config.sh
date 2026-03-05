#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
CONFIG_PATH="${BUNDLE_DIR}/config.json"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }

if [[ ! -f "${CONFIG_PATH}" ]]; then
    echo "[ERROR] config.json not found at ${CONFIG_PATH}"
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

echo "[*] Dropping capabilities from OCI config"
TMP_CONFIG="${CONFIG_PATH}.tmp"
jq \
    --argjson drop "${drop_caps_json}" \
    'def trim_caps($drop):
        (. // [])
        | map(select(($drop | index(.)) | not))
        | unique;
     .process.capabilities.bounding |= trim_caps($drop) |
     .process.capabilities.effective |= trim_caps($drop) |
     .process.capabilities.permitted |= trim_caps($drop)' \
    "${CONFIG_PATH}" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${CONFIG_PATH}"

# Validate caps are actually removed
for cap in "${DROP_CAPS[@]}"; do
    if jq -e --arg cap "${cap}" '
        (
            .process.capabilities.bounding +
            .process.capabilities.effective +
            .process.capabilities.permitted
        ) | index($cap) != null
    ' "${CONFIG_PATH}" >/dev/null; then
        echo "[ERROR] failed to remove ${cap} from config"
        exit 1
    fi
done

echo "[+] Capabilities hardened"
echo "    Removed: ${DROP_CAPS[*]}"
echo "    Bounding count: $(jq '.process.capabilities.bounding | length' "${CONFIG_PATH}")"
