#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vm-batch/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
BUNDLE_ENV="${ARTIFACT_DIR}/bundle.env"
ROOTFS_DIR="${BUNDLE_DIR}/rootfs"
CONFIG_PATH="${BUNDLE_DIR}/config.json"
REPLAY_DIR="${ARTIFACT_DIR}/replay"
REPLAY_TAR="${ARTIFACT_DIR}/runc-llama-replay.tar.xz"
ROOTFS_TAR="${ARTIFACT_DIR}/rootfs.tar.xz"

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }

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

if [[ ! -s "/tmp/vm-batch/out/llama-answer.txt" ]]; then
    echo "[ERROR] llama-answer.txt missing or empty"
    exit 1
fi

echo "[*] Validating config JSON"
jq empty "${CONFIG_PATH}" >/dev/null

echo "[*] Packing finalized rootfs (includes built llama.cpp)"
rm -f "${ROOTFS_TAR}"
(
    cd "${ROOTFS_DIR}"
    tar -cJf "${ROOTFS_TAR}" . --ignore-failed-read
)

if [[ ! -s "${ROOTFS_TAR}" ]]; then
    echo "[ERROR] rootfs.tar.xz is empty"
    exit 1
fi

rm -rf "${REPLAY_DIR}"
mkdir -p "${REPLAY_DIR}/models"

jq '.mounts = (.mounts | map(if .destination == "/models" then .source = "__MODELS_DIR__" else . end))' \
    "${CONFIG_PATH}" > "${REPLAY_DIR}/config.template.json"

cp "${ROOTFS_TAR}" "${REPLAY_DIR}/rootfs.tar.xz"
cp "${MODEL_PATH}" "${REPLAY_DIR}/models/${MODEL_FILE}"

cat > "${REPLAY_DIR}/run-from-output.sh" <<'RUNEOF'
#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${BUNDLE_DIR}/work"
CONTAINER_NAME="${CONTAINER_NAME:-llama-replay}"
PROMPT="${PROMPT:-Say in one short sentence that this replay bundle works.}"

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

MODEL_PATH="$(find "${BUNDLE_DIR}/models" -maxdepth 1 -type f -name '*.gguf' | head -n 1)"
if [[ -z "${MODEL_PATH}" ]]; then
    echo "[ERROR] No GGUF model found under ${BUNDLE_DIR}/models"
    exit 1
fi
MODEL_FILE="$(basename "${MODEL_PATH}")"

mkdir -p "${WORK_DIR}/rootfs"
if [[ ! -f "${WORK_DIR}/.rootfs_ready" ]]; then
    tar -xJf "${BUNDLE_DIR}/rootfs.tar.xz" -C "${WORK_DIR}/rootfs"
    touch "${WORK_DIR}/.rootfs_ready"
fi

jq --arg models "${BUNDLE_DIR}/models" \
   '.root.path = "rootfs" |
    .mounts = (.mounts | map(if .destination == "/models" then .source = $models else . end))' \
   "${BUNDLE_DIR}/config.template.json" > "${WORK_DIR}/config.json"

(
  cd "${WORK_DIR}"
  ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
  ${SUDO} runc run -d "${CONTAINER_NAME}"
)

prompt_escaped="$(printf '%s' "${PROMPT}" | sed "s/'/'\"'\"'/g")"
${SUDO} runc exec "${CONTAINER_NAME}" /bin/sh -lc \
  "/opt/llama.cpp/build/bin/llama-cli -m /models/${MODEL_FILE} -p '${prompt_escaped}' -n 48 --temp 0.2 --seed 42"

${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
RUNEOF
chmod +x "${REPLAY_DIR}/run-from-output.sh"

cat > "${REPLAY_DIR}/README.txt" <<READEOF
Replay bundle contents:
- rootfs.tar.xz: Ubuntu minimal rootfs with llama.cpp already built in /opt/llama.cpp
- config.template.json: OCI config with model mount source placeholder
- models/${MODEL_FILE}: GGUF model used for prompt test
- run-from-output.sh: script that extracts rootfs, patches config, and runs runc

Usage:
  cd \$(dirname run-from-output.sh)
  ./run-from-output.sh
READEOF

rm -f "${REPLAY_TAR}"
(
    cd "${REPLAY_DIR}"
    tar -cJf "${REPLAY_TAR}" .
)

if [[ ! -s "${REPLAY_TAR}" ]]; then
    echo "[ERROR] replay tar was not created"
    exit 1
fi

cp "${REPLAY_TAR}" /tmp/vm-batch/out/runc-llama-replay.tar.xz

{
    echo "[+] runc llama replay bundle generated"
    echo "    replay tar: /tmp/vm-batch/out/runc-llama-replay.tar.xz"
    echo "    model file: ${MODEL_FILE}"
    echo "    prompt test output: /tmp/vm-batch/out/llama-answer.txt"
    echo ""
    echo "To replay later (from extracted output):"
    echo "  tar -xJf runc-llama-replay.tar.xz"
    echo "  ./run-from-output.sh"
} > /tmp/vm-batch/out/bundle-manifest.txt

echo "[+] Verification complete"
