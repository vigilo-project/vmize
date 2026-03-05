#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUT_DIR="/tmp/vmize-worker/out"

VMIZE_TAR="${OUT_DIR}/vmize-package.tar"
ANSWER_FILE="${OUT_DIR}/llama-answer.txt"
ERROR_FILE="${OUT_DIR}/llama-error.txt"

CONTAINER_NAME="llama-pkg"
SOCKET_DIR="/var/lib/package-manager/containers/${CONTAINER_NAME}/runtime/sockets"
SOCKET_PATH="${SOCKET_DIR}/llama.sock"

PROMPT_TEXT="${LLAMA_PROMPT:-Say in one short sentence that the package-manager loader successfully ran the vmize package.}"
SERVER_TIMEOUT=60

CONTAINER_STARTED=0

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

cleanup() {
    set +e
    if [[ ${CONTAINER_STARTED} -eq 1 ]]; then
        ${SUDO} loader kill "${CONTAINER_NAME}" >/dev/null 2>&1 || true
        ${SUDO} loader delete "${CONTAINER_NAME}" --force >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# Verify all 5 input artifacts exist
for artifact in rootfs.squashfs rootfs.verity rootfs.root_hash config.json model.gguf; do
    if [[ ! -f "${WORK_DIR}/${artifact}" ]]; then
        echo "[ERROR] Missing input artifact: ${WORK_DIR}/${artifact}"
        exit 1
    fi
done

echo "[*] Creating vmize package tar"
tar cf "${VMIZE_TAR}" \
    -C "${WORK_DIR}" \
    rootfs.squashfs rootfs.verity rootfs.root_hash config.json model.gguf

ls -lh "${VMIZE_TAR}"

echo "[*] Running loader run"
${SUDO} loader run "${CONTAINER_NAME}" --pkg "${VMIZE_TAR}" --detach
CONTAINER_STARTED=1

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

echo "[*] Starting llama-server inside container"
${SUDO} runc exec -d "${CONTAINER_NAME}" /bin/sh -lc \
    "env LD_LIBRARY_PATH=${RUNTIME_LD_LIBRARY_PATH} \
     /opt/llama.cpp/build/bin/llama-server \
     -m /models/model.gguf \
     --host /sockets/llama.sock \
     --port 8080 \
     --ctx-size 2048 \
     --threads 2 \
     > /sockets/llama-server.log 2>&1"

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
    fi
    sleep 1
done

if [[ ${socket_ready} -ne 1 ]]; then
    echo "[ERROR] UDS socket not ready after ${SERVER_TIMEOUT}s"
    echo "[DEBUG] Expected socket: ${SOCKET_PATH}"
    ls -la "${SOCKET_DIR}" 2>&1 || echo "[DEBUG] Socket dir does not exist"
    echo "[DEBUG] Server log:"
    ${SUDO} runc exec "${CONTAINER_NAME}" cat /sockets/llama-server.log 2>&1 || true
    echo "[DEBUG] Container processes:"
    ${SUDO} runc exec "${CONTAINER_NAME}" ps aux 2>&1 || true
    exit 1
fi
echo "[+] UDS socket is ready: ${SOCKET_PATH}"

echo "[*] Sending inference request via UDS"
set +e
http_code=$(${SUDO} curl --unix-socket "${SOCKET_PATH}" \
    -X POST http://localhost/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d "{
        \"messages\": [{\"role\": \"user\", \"content\": \"${PROMPT_TEXT}\"}],
        \"max_tokens\": 48,
        \"temperature\": 0.2,
        \"seed\": 42
    }" \
    -s -o "${ANSWER_FILE}" \
    -w "%{http_code}" \
    --max-time 30 \
    2> "${ERROR_FILE}")
curl_status=$?
set -e

if [[ ${curl_status} -ne 0 ]] || [[ "${http_code}" != "200" ]]; then
    echo "[ERROR] UDS inference failed (curl_status=${curl_status}, http_code=${http_code})"
    [[ -s "${ERROR_FILE}" ]] && cat "${ERROR_FILE}" >&2
    [[ -s "${ANSWER_FILE}" ]] && cat "${ANSWER_FILE}" >&2
    exit 1
fi

if ! grep -q '[[:alnum:]]' "${ANSWER_FILE}"; then
    echo "[ERROR] inference produced empty output"
    [[ -s "${ERROR_FILE}" ]] && cat "${ERROR_FILE}" >&2
    exit 1
fi

echo "[+] Inference succeeded via package-manager loader"
echo "[*] Response:"
cat "${ANSWER_FILE}"
