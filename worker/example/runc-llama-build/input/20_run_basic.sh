#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
BUNDLE_ENV="${ARTIFACT_DIR}/bundle.env"
SOCKET_DIR="${BUNDLE_DIR}/sockets"

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

if [[ ! -f "${BUNDLE_ENV}" ]]; then
    echo "[ERROR] bundle.env not found at ${BUNDLE_ENV}"
    exit 1
fi

# shellcheck source=/dev/null
source "${BUNDLE_ENV}"

CONTAINER_NAME="${CONTAINER_NAME:-llama-basic}"
MODEL_FILE="${MODEL_FILE:?MODEL_FILE missing in bundle.env}"
MODEL_PATH="${MODEL_PATH:?MODEL_PATH missing in bundle.env}"
PROMPT_TEXT="${LLAMA_PROMPT:-${PROMPT_TEXT:-Say in one short sentence that batch runc llama demo works.}}"

SOCKET_PATH="${SOCKET_DIR}/llama.sock"
SERVER_TIMEOUT=60

if [[ ! -f "${BUNDLE_DIR}/config.json" ]]; then
    echo "[ERROR] config.json not found at ${BUNDLE_DIR}/config.json"
    exit 1
fi

if [[ ! -d "${BUNDLE_DIR}/rootfs" ]]; then
    echo "[ERROR] rootfs directory not found at ${BUNDLE_DIR}/rootfs"
    exit 1
fi

if [[ ! -f "${MODEL_PATH}" ]]; then
    echo "[ERROR] GGUF model not found at ${MODEL_PATH}"
    exit 1
fi

run_exec() {
    local cmd="$1"
    ${SUDO} runc exec "${CONTAINER_NAME}" /bin/sh -lc "${cmd}"
}

run_exec_with_retry() {
    local description="$1"
    local cmd="$2"
    local attempts="${3:-4}"
    local attempt sleep_sec

    for ((attempt = 1; attempt <= attempts; attempt++)); do
        if run_exec "${cmd}"; then
            return 0
        fi

        if (( attempt < attempts )); then
            sleep_sec=$((attempt * 2))
            echo "[!] ${description} failed (${attempt}/${attempts}), retrying in ${sleep_sec}s"
            sleep "${sleep_sec}"
        fi
    done

    return 1
}

cleanup() {
    echo "[*] Cleaning up container: ${CONTAINER_NAME}"
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[*] Starting OCI container"
(
    cd "${BUNDLE_DIR}"
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    ${SUDO} runc run -d "${CONTAINER_NAME}"
)

if ${SUDO} runc list | awk 'NR>1 {print $1}' | grep -Fxq "${CONTAINER_NAME}"; then
    echo "[+] Container is running: ${CONTAINER_NAME}"
else
    echo "[ERROR] Container did not start correctly."
    exit 1
fi

echo "[*] Installing llama.cpp dependencies inside container"
# Copy guest VM's resolv.conf into container for DNS resolution
if ! run_exec "nslookup archive.ubuntu.com" >/dev/null 2>&1; then
    echo "[!] DNS resolution failed, copying guest VM's resolv.conf to container"
    ${SUDO} cat /etc/resolv.conf | run_exec "cat > /etc/resolv.conf"
fi
run_exec_with_retry \
    "container apt dependencies install" \
    "export DEBIAN_FRONTEND=noninteractive; \
    apt-get -qq update -o Acquire::Retries=3 -o Acquire::http::Timeout=20; \
    apt-get -qq install -y --no-install-recommends build-essential cmake git ca-certificates pkg-config curl" \
    5

echo "[*] Building llama.cpp inside container rootfs"
run_exec "if [ ! -d /opt/llama.cpp/.git ]; then git clone --depth 1 --quiet https://github.com/ggerganov/llama.cpp /opt/llama.cpp; fi"
if ! run_exec "cmake -S /opt/llama.cpp -B /opt/llama.cpp/build -DCMAKE_BUILD_TYPE=Release -DGGML_NATIVE=OFF >/tmp/llama-cmake-config.log 2>&1"; then
    run_exec "tail -n 120 /tmp/llama-cmake-config.log" || true
    exit 1
fi
# Build both llama-cli and llama-server
if ! run_exec "cmake --build /opt/llama.cpp/build --target llama-cli llama-server -j 2 >/tmp/llama-cmake-build.log 2>&1"; then
    run_exec "tail -n 120 /tmp/llama-cmake-build.log" || true
    exit 1
fi

run_exec "test -x /opt/llama.cpp/build/bin/llama-cli"
run_exec "test -x /opt/llama.cpp/build/bin/llama-server"

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
     -m /models/${MODEL_FILE} \
     --host /sockets/llama.sock \
     --port 8080 \
     --ctx-size 2048 \
     --threads 2 \
     > /tmp/llama-server.log 2>&1"

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
        run_exec "tail -5 /tmp/llama-server.log 2>/dev/null" || true
    fi
    sleep 1
done

if [[ ${socket_ready} -ne 1 ]]; then
    echo "[ERROR] llama-server socket not ready after ${SERVER_TIMEOUT}s"
    run_exec "cat /tmp/llama-server.log" || true
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
run_exec "cat /tmp/llama-server.log" > /tmp/vmize-worker/out/llama-server.log 2>&1 || true

# Show server log for debugging
echo "[DEBUG] llama-server log:"
run_exec "tail -20 /tmp/llama-server.log" || true

# Send inference request via UDS
echo "[*] Sending inference request via UDS"
prompt_escaped="$(printf '%s' "${PROMPT_TEXT}" | sed 's/"/\\"/g')"

# Ensure output directory exists
mkdir -p /tmp/vmize-worker/out

set +e
# Use sudo for curl if socket is owned by root and we're not root
if [[ -S "${SOCKET_PATH}" ]] && [[ "$(stat -c %U "${SOCKET_PATH}" 2>/dev/null)" == "root" ]] && [[ "$(id -u)" -ne 0 ]]; then
    echo "[DEBUG] Running curl with sudo (socket owned by root)"
    http_code=$(${SUDO} curl --unix-socket "${SOCKET_PATH}" \
        -X POST http://localhost/v1/chat/completions \
        -H "Content-Type: application/json" \
        -d "{
            \"messages\": [{\"role\": \"user\", \"content\": \"${prompt_escaped}\"}],
            \"max_tokens\": 48,
            \"temperature\": 0.2,
            \"seed\": 42
        }" \
        -s -o /tmp/vmize-worker/out/llama-answer.txt \
        -w "%{http_code}" \
        --max-time 30 \
        2> /tmp/vmize-worker/out/llama-error.txt)
    curl_status=$?
else
    http_code=$(curl --unix-socket "${SOCKET_PATH}" \
        -X POST http://localhost/v1/chat/completions \
        -H "Content-Type: application/json" \
        -d "{
            \"messages\": [{\"role\": \"user\", \"content\": \"${prompt_escaped}\"}],
            \"max_tokens\": 48,
            \"temperature\": 0.2,
            \"seed\": 42
        }" \
        -s -o /tmp/vmize-worker/out/llama-answer.txt \
        -w "%{http_code}" \
        --max-time 30 \
        2> /tmp/vmize-worker/out/llama-error.txt)
    curl_status=$?
fi
set -e

echo "[DEBUG] curl_status=${curl_status}, http_code=${http_code}"
echo "[DEBUG] llama-answer.txt content:"
cat /tmp/vmize-worker/out/llama-answer.txt 2>&1 || true
echo ""
echo "[DEBUG] llama-error.txt content:"
cat /tmp/vmize-worker/out/llama-error.txt 2>&1 || true

${SUDO} runc list > /tmp/vmize-worker/out/runc-list.txt
run_exec "/opt/llama.cpp/build/bin/llama-cli --version" > /tmp/vmize-worker/out/llama-version.txt 2>&1 || true
printf '%s\n' "${PROMPT_TEXT}" > /tmp/vmize-worker/out/prompt.txt

if [[ ${curl_status} -ne 0 ]] || [[ "${http_code}" != "200" ]]; then
    echo "[ERROR] UDS inference failed (curl_status=${curl_status}, http_code=${http_code})"
    if [[ -s /tmp/vmize-worker/out/llama-error.txt ]]; then
        cat /tmp/vmize-worker/out/llama-error.txt >&2
    fi
    if [[ -s /tmp/vmize-worker/out/llama-answer.txt ]]; then
        cat /tmp/vmize-worker/out/llama-answer.txt >&2
    fi
    exit 1
fi

if ! grep -q '[[:alnum:]]' /tmp/vmize-worker/out/llama-answer.txt; then
    echo "[ERROR] llama-server produced empty output"
    if [[ -s /tmp/vmize-worker/out/llama-error.txt ]]; then
        cat /tmp/vmize-worker/out/llama-error.txt >&2
    fi
    exit 1
fi

echo "[+] llama-server inference succeeded via UDS"
