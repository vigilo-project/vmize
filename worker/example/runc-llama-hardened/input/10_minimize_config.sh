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

BUNDLE_DIR="${WORK_DIR}/bundle"
BUNDLE_ROOTFS="${BUNDLE_DIR}/rootfs"
BUNDLE_CONFIG="${BUNDLE_DIR}/config.json"
MODEL_DIR="${BUNDLE_DIR}/models"
SOCKET_DIR="${BUNDLE_DIR}/sockets"

OUTPUT_ROOTFS="${OUT_DIR}/rootfs"
OUTPUT_CHAIN_CONFIG="${OUT_DIR}/config.json"
OUTPUT_CHAIN_MODEL="${OUT_DIR}/model.gguf"
OUTPUT_CONFIG="${OUT_DIR}/config.min.json"
OUTPUT_REMOVED="${OUT_DIR}/removed-caps.txt"
OUTPUT_SUMMARY="${OUT_DIR}/cap-summary.txt"
OUTPUT_ANSWER="${OUT_DIR}/llama-answer.txt"
OUTPUT_ERROR="${OUT_DIR}/llama-error.txt"
OUTPUT_RUNC_LIST="${OUT_DIR}/runc-list.txt"
OUTPUT_SERVER_LOG="${OUT_DIR}/llama-server.log"
PROMPT_TEXT="${LLAMA_PROMPT:-Say in one short sentence that hardened runc llama stage works.}"

CONTAINER_NAME="llama-hardened"
SOCKET_PATH="${SOCKET_DIR}/llama.sock"
SERVER_TIMEOUT=60
BIND_MOUNTED=0

command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }
command -v runc >/dev/null 2>&1 || { echo "[ERROR] runc not found"; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "[ERROR] curl not found"; exit 1; }

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

# Create bundle directory structure
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_ROOTFS}" "${MODEL_DIR}" "${SOCKET_DIR}"

# Ensure socket directory is writable (container runs as root)
chmod 777 "${SOCKET_DIR}"

# Copy model to bundle
cp -f "${INPUT_MODEL}" "${MODEL_DIR}/model.gguf"

# Create bind mount from ACTIVE_ROOTFS to BUNDLE_ROOTFS
${SUDO} mount --bind "${ACTIVE_ROOTFS}" "${BUNDLE_ROOTFS}"
${SUDO} mount -o remount,bind,ro "${BUNDLE_ROOTFS}"
BIND_MOUNTED=1

# Generate minimized config with UDS socket mount
jq \
    --argjson drop "${drop_caps_json}" \
    --arg model_source "${MODEL_DIR}" \
    --arg socket_source "${SOCKET_DIR}" \
    'def trim_caps($drop):
        (. // [])
        | map(select(($drop | index(.)) | not))
        | unique;
     .process.capabilities.bounding = trim_caps($drop) |
     .process.capabilities.effective = trim_caps($drop) |
     .process.capabilities.permitted = trim_caps($drop) |
     .root.path = "rootfs" |
     .process.args = ["/bin/sh", "-lc", "sleep infinity"] |
     .mounts = (
        (.mounts // [])
        | map(select(.destination != "/models" and .destination != "/sockets"))
        + [{
            "destination": "/models",
            "type": "bind",
            "source": $model_source,
            "options": ["rbind", "ro"]
        }, {
            "destination": "/sockets",
            "type": "bind",
            "source": $socket_source,
            "options": ["rbind", "rw"]
        }]
     )' \
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

# Copy config to bundle
cp -f "${OUTPUT_CONFIG}" "${BUNDLE_CONFIG}"

cleanup() {
    set +e
    echo "[*] Cleaning up..."
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    if [[ ${BIND_MOUNTED:-0} -eq 1 ]] && mountpoint -q "${BUNDLE_ROOTFS}"; then
        ${SUDO} umount "${BUNDLE_ROOTFS}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# Start runc container
echo "[*] Starting hardened runc container: ${CONTAINER_NAME}"
(
    cd "${BUNDLE_DIR}"
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    ${SUDO} runc run -d "${CONTAINER_NAME}"
)

if ! ${SUDO} runc list | awk 'NR>1 {print $1}' | grep -Fxq "${CONTAINER_NAME}"; then
    echo "[ERROR] container did not start: ${CONTAINER_NAME}"
    exit 1
fi
echo "[+] Container is running: ${CONTAINER_NAME}"

# Start llama-server inside container
echo "[*] Starting llama-server with UDS socket"
# Detect architecture for LD_LIBRARY_PATH
ARCH="$(uname -m)"
case "${ARCH}" in
    aarch64|arm64)
        RUNTIME_LD_LIBRARY_PATH="/opt/llama.cpp/build/bin:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu:/usr/lib:/lib"
        ;;
    x86_64|amd64)
        RUNTIME_LD_LIBRARY_PATH="/opt/llama.cpp/build/bin:/usr/lib/x86_64-linux-gnu:/lib/x86_64-linux-gnu:/usr/lib:/lib"
        ;;
    *)
        RUNTIME_LD_LIBRARY_PATH="/opt/llama.cpp/build/bin:/usr/lib:/lib"
        ;;
esac

${SUDO} runc exec -d "${CONTAINER_NAME}" /bin/sh -lc \
    "env LD_LIBRARY_PATH=${RUNTIME_LD_LIBRARY_PATH} \
     /opt/llama.cpp/build/bin/llama-server \
     -m /models/model.gguf \
     --host /sockets/llama.sock \
     --port 8080 \
     --ctx-size 2048 \
     --threads 2 \
     > /sockets/llama-server.log 2>&1"

# Wait for socket to be created and server to be ready
echo "[*] Waiting for UDS socket to be ready..."
socket_ready=0
for i in $(seq 1 ${SERVER_TIMEOUT}); do
    if [[ -S "${SOCKET_PATH}" ]]; then
        # Give server a moment to fully initialize after socket creation
        sleep 2
        socket_ready=1
        echo "[+] Socket file detected (attempt ${i})"
        break
    fi
    # Show progress every 10 seconds
    if (( i % 10 == 0 )); then
        echo "[*] Still waiting... (${i}s)"
        ${SUDO} runc exec "${CONTAINER_NAME}" tail -5 /sockets/llama-server.log 2>/dev/null || true
    fi
    sleep 1
done

if [[ ${socket_ready} -ne 1 ]]; then
    echo "[ERROR] llama-server socket not ready after ${SERVER_TIMEOUT}s"
    ${SUDO} runc exec "${CONTAINER_NAME}" cat /sockets/llama-server.log > "${OUTPUT_SERVER_LOG}" 2>&1 || true
    cat "${OUTPUT_SERVER_LOG}" >&2
    echo ""
    echo "[DEBUG] Socket file status:"
    ls -la "${SOCKET_PATH}" 2>&1 || true
    echo ""
    echo "[DEBUG] Container processes:"
    ${SUDO} runc exec "${CONTAINER_NAME}" ps aux 2>&1 || true
    exit 1
fi
echo "[+] UDS socket is ready: ${SOCKET_PATH}"

# Copy server log
${SUDO} runc exec "${CONTAINER_NAME}" cat /sockets/llama-server.log > "${OUTPUT_SERVER_LOG}" 2>&1 || true

# Show server log for debugging
echo "[DEBUG] llama-server log:"
${SUDO} runc exec "${CONTAINER_NAME}" tail -20 /sockets/llama-server.log || true

# Send inference request via UDS
echo "[*] Sending inference request via UDS"

set +e
# Use sudo for curl if socket is owned by root and we're not root
if [[ -S "${SOCKET_PATH}" ]] && [[ "$(stat -c %U "${SOCKET_PATH}" 2>/dev/null)" == "root" ]] && [[ "$(id -u)" -ne 0 ]]; then
    echo "[DEBUG] Running curl with sudo (socket owned by root)"
    http_code=$(${SUDO} curl --unix-socket "${SOCKET_PATH}" \
        -X POST http://localhost/v1/chat/completions \
        -H "Content-Type: application/json" \
        -d "{
            \"messages\": [{\"role\": \"user\", \"content\": \"${PROMPT_TEXT}\"}],
            \"max_tokens\": 48,
            \"temperature\": 0.2,
            \"seed\": 42
        }" \
        -s -o "${OUTPUT_ANSWER}" \
        -w "%{http_code}" \
        --max-time 30 \
        2> "${OUTPUT_ERROR}")
    curl_status=$?
else
    http_code=$(curl --unix-socket "${SOCKET_PATH}" \
        -X POST http://localhost/v1/chat/completions \
        -H "Content-Type: application/json" \
        -d "{
            \"messages\": [{\"role\": \"user\", \"content\": \"${PROMPT_TEXT}\"}],
            \"max_tokens\": 48,
            \"temperature\": 0.2,
            \"seed\": 42
        }" \
        -s -o "${OUTPUT_ANSWER}" \
        -w "%{http_code}" \
        --max-time 30 \
        2> "${OUTPUT_ERROR}")
    curl_status=$?
fi
set -e

echo "[DEBUG] curl_status=${curl_status}, http_code=${http_code}"
echo "[DEBUG] llama-answer.txt content:"
cat "${OUTPUT_ANSWER}" 2>&1 || true
echo ""
echo "[DEBUG] llama-error.txt content:"
cat "${OUTPUT_ERROR}" 2>&1 || true

${SUDO} runc list > "${OUTPUT_RUNC_LIST}" 2>&1 || true

if [[ ${curl_status} -ne 0 ]] || [[ "${http_code}" != "200" ]]; then
    echo "[ERROR] UDS inference failed (curl_status=${curl_status}, http_code=${http_code})"
    [[ -s "${OUTPUT_ERROR}" ]] && cat "${OUTPUT_ERROR}" >&2
    [[ -s "${OUTPUT_ANSWER}" ]] && cat "${OUTPUT_ANSWER}" >&2
    exit 1
fi

if ! grep -q '[[:alnum:]]' "${OUTPUT_ANSWER}"; then
    echo "[ERROR] llama-server produced empty output"
    cat "${OUTPUT_ERROR}" >&2
    exit 1
fi

echo "[+] llama-server inference succeeded via UDS"
echo "llama_hardened_prompt=uds_success" >> "${OUTPUT_SUMMARY}"
echo "llama_hardened_exit=0" >> "${OUTPUT_SUMMARY}"
echo "llama_server_mode=uds" >> "${OUTPUT_SUMMARY}"
echo "llama_socket_path=${SOCKET_PATH}" >> "${OUTPUT_SUMMARY}"

# Prepare chain handoff artifacts
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
