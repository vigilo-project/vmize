#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUT_DIR="/tmp/vmize-worker/out"

INPUT_SIGNED_TAR="${WORK_DIR}/signed-runtime.tar"
INPUT_CERT="${WORK_DIR}/cert.der"

RUNTIME_DIR="${WORK_DIR}/runtime.ima-verify"
PAYLOAD_DIR="${RUNTIME_DIR}/payload"
VERIFIED_MOUNT="${RUNTIME_DIR}/rootfs.verified"
BUNDLE_DIR="${RUNTIME_DIR}/bundle"
BUNDLE_ROOTFS="${BUNDLE_DIR}/rootfs"
BUNDLE_CONFIG="${BUNDLE_DIR}/config.json"
MODEL_DIR="${RUNTIME_DIR}/models"
MODEL_PATH="${MODEL_DIR}/model.gguf"
SOCKET_DIR="${RUNTIME_DIR}/sockets"
SOCKET_PATH="${SOCKET_DIR}/llama.sock"

EXTRACTED_SQUASHFS="${PAYLOAD_DIR}/rootfs.squashfs"
EXTRACTED_VERITY="${PAYLOAD_DIR}/rootfs.verity"
EXTRACTED_ROOT_HASH="${PAYLOAD_DIR}/rootfs.root_hash"
EXTRACTED_MODEL="${PAYLOAD_DIR}/model.gguf"
EXTRACTED_CONFIG="${PAYLOAD_DIR}/config.json"

OUTPUT_ANSWER="${OUT_DIR}/llama-answer.txt"
OUTPUT_ERROR="${OUT_DIR}/llama-error.txt"
OUTPUT_SERVICE_LOG="${OUT_DIR}/llama-service.log"
OUTPUT_SUMMARY="${OUT_DIR}/runtime-summary.txt"
OUTPUT_RUNC_LIST="${OUT_DIR}/runc-list.txt"
OUTPUT_PROMPT="${OUT_DIR}/prompt.txt"
OUTPUT_IMA_VERIFY_LOG="${OUT_DIR}/ima-verify.log"
OUTPUT_SERVER_LOG="${OUT_DIR}/llama-server.log"

CONTAINER_NAME="llama-ima-verified-runtime"
VERITY_NAME="vmize-verity-ima-$$-$(date +%s)"
VERITY_DEVICE="/dev/mapper/${VERITY_NAME}"
PROMPT_TEXT="${LLAMA_PROMPT:-Say in one short sentence that IMA verified tar runtime stage works.}"
SERVER_TIMEOUT=60

DATA_LOOP=""
HASH_LOOP=""
VERITY_OPENED=0
ROOTFS_MOUNTED=0
BIND_MOUNTED=0
CONTAINER_STARTED=0

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

command -v curl >/dev/null 2>&1 || { echo "[ERROR] curl not found"; exit 1; }
command -v evmctl >/dev/null 2>&1 || { echo "[ERROR] evmctl not found"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
command -v losetup >/dev/null 2>&1 || { echo "[ERROR] losetup not found"; exit 1; }
command -v mount >/dev/null 2>&1 || { echo "[ERROR] mount not found"; exit 1; }
command -v mountpoint >/dev/null 2>&1 || { echo "[ERROR] mountpoint not found"; exit 1; }
command -v runc >/dev/null 2>&1 || { echo "[ERROR] runc not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }
command -v veritysetup >/dev/null 2>&1 || { echo "[ERROR] veritysetup not found"; exit 1; }

cleanup() {
    set +e

    if [[ ${CONTAINER_STARTED} -eq 1 ]]; then
        ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    fi

    if [[ ${BIND_MOUNTED} -eq 1 ]] && mountpoint -q "${BUNDLE_ROOTFS}"; then
        ${SUDO} umount "${BUNDLE_ROOTFS}" >/dev/null 2>&1 || true
    fi

    if [[ ${ROOTFS_MOUNTED} -eq 1 ]] && mountpoint -q "${VERIFIED_MOUNT}"; then
        ${SUDO} umount "${VERIFIED_MOUNT}" >/dev/null 2>&1 || true
    fi

    if [[ ${VERITY_OPENED} -eq 1 ]]; then
        ${SUDO} veritysetup close "${VERITY_NAME}" >/dev/null 2>&1 || true
    fi

    if [[ -n "${HASH_LOOP}" ]]; then
        ${SUDO} losetup -d "${HASH_LOOP}" >/dev/null 2>&1 || true
    fi

    if [[ -n "${DATA_LOOP}" ]]; then
        ${SUDO} losetup -d "${DATA_LOOP}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

if [[ ! -f "${INPUT_SIGNED_TAR}" ]]; then
    echo "[ERROR] missing required input: ${INPUT_SIGNED_TAR}"
    exit 1
fi

if [[ ! -f "${INPUT_CERT}" ]]; then
    echo "[ERROR] missing required input: ${INPUT_CERT}"
    exit 1
fi

if [[ ! -s "${INPUT_SIGNED_TAR}" ]]; then
    echo "[ERROR] signed runtime tar is empty: ${INPUT_SIGNED_TAR}"
    exit 1
fi

if [[ ! -s "${INPUT_CERT}" ]]; then
    echo "[ERROR] verification cert is empty: ${INPUT_CERT}"
    exit 1
fi

rm -rf "${RUNTIME_DIR}"
mkdir -p "${PAYLOAD_DIR}" "${VERIFIED_MOUNT}" "${BUNDLE_ROOTFS}" "${MODEL_DIR}" "${SOCKET_DIR}"
chmod 777 "${SOCKET_DIR}"

if [[ -n "${SUDO}" ]]; then
    ${SUDO} tar --xattrs --xattrs-include='*' -xpf "${INPUT_SIGNED_TAR}" -C "${PAYLOAD_DIR}"
else
    tar --xattrs --xattrs-include='*' -xpf "${INPUT_SIGNED_TAR}" -C "${PAYLOAD_DIR}"
fi

for extracted in "${EXTRACTED_SQUASHFS}" "${EXTRACTED_VERITY}" "${EXTRACTED_ROOT_HASH}" "${EXTRACTED_MODEL}" "${EXTRACTED_CONFIG}"; do
    if [[ ! -f "${extracted}" ]]; then
        echo "[ERROR] missing extracted payload file: ${extracted}"
        exit 1
    fi
done

if [[ ! -s "${EXTRACTED_MODEL}" ]]; then
    echo "[ERROR] extracted model is empty: ${EXTRACTED_MODEL}"
    exit 1
fi

jq empty "${EXTRACTED_CONFIG}" >/dev/null

: > "${OUTPUT_IMA_VERIFY_LOG}"
for verify_target in "${EXTRACTED_SQUASHFS}" "${EXTRACTED_VERITY}" "${EXTRACTED_ROOT_HASH}" "${EXTRACTED_CONFIG}" "${EXTRACTED_MODEL}"; do
    if [[ -n "${SUDO}" ]]; then
        ${SUDO} evmctl -v --key "${INPUT_CERT}" ima_verify "${verify_target}" >> "${OUTPUT_IMA_VERIFY_LOG}" 2>&1
    else
        evmctl -v --key "${INPUT_CERT}" ima_verify "${verify_target}" >> "${OUTPUT_IMA_VERIFY_LOG}" 2>&1
    fi
done

ROOT_HASH="$(tr -d '[:space:]' < "${EXTRACTED_ROOT_HASH}" | tr 'A-F' 'a-f')"
if ! printf '%s\n' "${ROOT_HASH}" | grep -Eq '^[0-9a-f]{64}$'; then
    echo "[ERROR] invalid root hash: '${ROOT_HASH}'"
    exit 1
fi

cp -f "${EXTRACTED_MODEL}" "${MODEL_PATH}"

DATA_LOOP="$(${SUDO} losetup --find --show "${EXTRACTED_SQUASHFS}")"
HASH_LOOP="$(${SUDO} losetup --find --show "${EXTRACTED_VERITY}")"

${SUDO} veritysetup verify "${DATA_LOOP}" "${HASH_LOOP}" "${ROOT_HASH}"
${SUDO} veritysetup open "${DATA_LOOP}" "${VERITY_NAME}" "${HASH_LOOP}" "${ROOT_HASH}"
VERITY_OPENED=1

for _ in $(seq 1 20); do
    if [[ -b "${VERITY_DEVICE}" ]]; then
        break
    fi
    sleep 1
done

if [[ ! -b "${VERITY_DEVICE}" ]]; then
    echo "[ERROR] verity device not available: ${VERITY_DEVICE}"
    exit 1
fi

${SUDO} mount -t squashfs -o ro "${VERITY_DEVICE}" "${VERIFIED_MOUNT}"
ROOTFS_MOUNTED=1

${SUDO} mount --bind "${VERIFIED_MOUNT}" "${BUNDLE_ROOTFS}"
${SUDO} mount -o remount,bind,ro "${BUNDLE_ROOTFS}"
BIND_MOUNTED=1

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

jq \
    --arg model_source "${MODEL_DIR}" \
    --arg socket_source "${SOCKET_DIR}" \
    '.root.path = "rootfs" |
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
    "${EXTRACTED_CONFIG}" > "${BUNDLE_CONFIG}"

(
    cd "${BUNDLE_DIR}"
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    ${SUDO} runc run -d "${CONTAINER_NAME}"
)
CONTAINER_STARTED=1

if ! ${SUDO} runc list | awk 'NR>1 {print $1}' | grep -Fxq "${CONTAINER_NAME}"; then
    echo "[ERROR] container did not start: ${CONTAINER_NAME}"
    exit 1
fi

# Start llama-server inside container
echo "[*] Starting llama-server with UDS socket"
${SUDO} runc exec -d "${CONTAINER_NAME}" /bin/sh -lc \
    "env LD_LIBRARY_PATH=${RUNTIME_LD_LIBRARY_PATH} \
     /opt/llama.cpp/build/bin/llama-server \
     -m /models/model.gguf \
     --host /sockets/llama.sock \
     --port 8080 \
     --ctx-size 2048 \
     --threads 2 \
     > /sockets/llama-server.log 2>&1"

# Wait for socket to be created
echo "[*] Waiting for UDS socket to be ready..."
socket_ready=0
for i in $(seq 1 ${SERVER_TIMEOUT}); do
    if [[ -S "${SOCKET_PATH}" ]]; then
        sleep 2
        socket_ready=1
        echo "[+] Socket file detected (attempt ${i})"
        break
    fi
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
    exit 1
fi
echo "[+] UDS socket is ready: ${SOCKET_PATH}"

# Copy server log
${SUDO} runc exec "${CONTAINER_NAME}" cat /sockets/llama-server.log > "${OUTPUT_SERVER_LOG}" 2>&1 || true

# Send inference request via UDS
echo "[*] Sending inference request via UDS"
printf '%s\n' "${PROMPT_TEXT}" > "${OUTPUT_PROMPT}"

set +e
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

echo "[+] IMA verification and runtime UDS inference completed"

{
    echo "mode=stage5-ima-verified-runtime"
    echo "verified_tar=${INPUT_SIGNED_TAR}"
    echo "cert=${INPUT_CERT}"
    echo "verity_data_loop=${DATA_LOOP}"
    echo "verity_hash_loop=${HASH_LOOP}"
    echo "verity_name=${VERITY_NAME}"
    echo "verity_device=${VERITY_DEVICE}"
    echo "uds_socket=${SOCKET_PATH}"
    echo "prompt=${PROMPT_TEXT}"
} > "${OUTPUT_SUMMARY}"
