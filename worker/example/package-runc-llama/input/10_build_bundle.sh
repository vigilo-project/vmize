#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="llama-basic"
BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
MODELS_DIR="${BUNDLE_DIR}/models"
SOCKETS_DIR="${BUNDLE_DIR}/sockets"
ROOTFS_DIR="${BUNDLE_DIR}/rootfs"
INPUT_MODELS_DIR="/tmp/vmize-worker/work/models"

case "$(uname -m)" in
    aarch64)
        ROOTFS_URL="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-arm64-root.tar.xz"
        ;;
    x86_64)
        ROOTFS_URL="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-amd64-root.tar.xz"
        ;;
    *)
        ROOTFS_URL="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-amd64-root.tar.xz"
        ;;
esac

pick_model_url() {
    local candidate
    local -a candidates=()

    if [[ -n "${LLAMA_MODEL_URL:-}" ]]; then
        candidates+=("${LLAMA_MODEL_URL}")
    fi

    candidates+=(
        "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_0.gguf"
        "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q2_k.gguf"
        "https://huggingface.co/bartowski/TinyLlama-1.1B-Chat-v1.0-GGUF/resolve/main/TinyLlama-1.1B-Chat-v1.0-Q2_K.gguf"
    )

    for candidate in "${candidates[@]}"; do
        [[ -z "${candidate}" ]] && continue
        echo "[*] Probing model URL: ${candidate}" >&2
        if wget -q --spider "${candidate}"; then
            printf '%s\n' "${candidate}"
            return 0
        fi
    done

    return 1
}

pick_local_model_path() {
    local candidate

    if [[ -n "${LOCAL_MODEL_PATH:-}" && -f "${LOCAL_MODEL_PATH}" ]]; then
        printf '%s\n' "${LOCAL_MODEL_PATH}"
        return 0
    fi

    candidate="$(find "${INPUT_MODELS_DIR}" -maxdepth 1 -type f -name '*.gguf' 2>/dev/null | sort | head -n 1 || true)"
    if [[ -n "${candidate}" ]]; then
        printf '%s\n' "${candidate}"
        return 0
    fi

    return 1
}

command -v runc >/dev/null 2>&1 || { echo "[ERROR] runc not found"; exit 1; }
command -v wget >/dev/null 2>&1 || { echo "[ERROR] wget not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }

DOWNLOAD_ATTEMPTS="${DOWNLOAD_ATTEMPTS:-8}"
DOWNLOAD_READ_TIMEOUT="${DOWNLOAD_READ_TIMEOUT:-30}"
DOWNLOAD_TIMEOUT="${DOWNLOAD_TIMEOUT:-30}"
DOWNLOAD_BACKOFF_BASE="${DOWNLOAD_BACKOFF_BASE:-3}"

download_with_retry() {
    local url="$1"
    local dest="$2"
    local attempt sleep_sec

    for ((attempt = 1; attempt <= DOWNLOAD_ATTEMPTS; attempt++)); do
        echo "[*] Download attempt ${attempt}/${DOWNLOAD_ATTEMPTS}: ${url}"
        if wget \
            -q \
            --continue \
            --read-timeout="${DOWNLOAD_READ_TIMEOUT}" \
            --timeout="${DOWNLOAD_TIMEOUT}" \
            --tries=1 \
            -O "${dest}" \
            "${url}"; then
            return 0
        fi

        if (( attempt < DOWNLOAD_ATTEMPTS )); then
            sleep_sec=$((DOWNLOAD_BACKOFF_BASE * attempt))
            echo "[!] Download attempt ${attempt} failed, retrying in ${sleep_sec}s"
            sleep "${sleep_sec}"
        fi
    done

    return 1
}

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
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

TEMP_ROOTFS_TAR="$(mktemp)"
cleanup() {
    set +e
    echo "[*] Cleaning up container: ${CONTAINER_NAME}"
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    rm -f "${TEMP_ROOTFS_TAR}"
}
trap cleanup EXIT

rm -rf "${ROOTFS_DIR}"
mkdir -p "${ROOTFS_DIR}" "${ARTIFACT_DIR}" "${MODELS_DIR}" "${SOCKETS_DIR}"

# --- Download and extract Ubuntu minimal rootfs ---
echo "[*] Downloading Ubuntu minimal rootfs"
echo "    URL: ${ROOTFS_URL}"
if ! download_with_retry "${ROOTFS_URL}" "${TEMP_ROOTFS_TAR}"; then
    echo "[ERROR] Failed to download Ubuntu minimal rootfs after ${DOWNLOAD_ATTEMPTS} attempts"
    exit 1
fi

echo "[*] Extracting rootfs"
tar -xJf "${TEMP_ROOTFS_TAR}" -C "${ROOTFS_DIR}" --exclude='dev' --exclude='dev/*'

mkdir -p "${ROOTFS_DIR}/dev"
touch \
    "${ROOTFS_DIR}/dev/null" \
    "${ROOTFS_DIR}/dev/zero" \
    "${ROOTFS_DIR}/dev/full" \
    "${ROOTFS_DIR}/dev/random" \
    "${ROOTFS_DIR}/dev/urandom"

mkdir -p "${ROOTFS_DIR}/tmp" "${ROOTFS_DIR}/var/tmp"
chmod 1777 "${ROOTFS_DIR}/tmp" "${ROOTFS_DIR}/var/tmp"

# --- Generate OCI config ---
(
    cd "${BUNDLE_DIR}"
    runc spec
)

echo "[*] Customizing OCI config"
TMP_CONFIG="${BUNDLE_DIR}/config.json.tmp"
jq \
    --arg model_source "${MODELS_DIR}" \
    --arg socket_source "${SOCKETS_DIR}" \
    --argjson caps '[
        "CAP_AUDIT_WRITE",
        "CAP_CHOWN",
        "CAP_DAC_OVERRIDE",
        "CAP_FOWNER",
        "CAP_FSETID",
        "CAP_KILL",
        "CAP_MKNOD",
        "CAP_NET_BIND_SERVICE",
        "CAP_NET_RAW",
        "CAP_SETFCAP",
        "CAP_SETGID",
        "CAP_SETPCAP",
        "CAP_SETUID",
        "CAP_SYS_CHROOT"
    ]' \
    '.process.args = ["/bin/sh", "-lc", "sleep infinity"] |
     .process.user.uid = 0 |
     .process.user.gid = 0 |
     .process.user.additionalGids = [] |
     .process.terminal = false |
     .process.noNewPrivileges = false |
     .process.capabilities.bounding = $caps |
     .process.capabilities.effective = $caps |
     .process.capabilities.permitted = $caps |
     .root.readonly = false |
     del(.linux.resources, .linux.cgroupsPath) |
     del(.linux.uidMappings, .linux.gidMappings) |
     .linux.namespaces |= map(select(.type != "network" and .type != "user")) |
     .mounts = ((.mounts | map(select(.destination != "/models" and .destination != "/sockets"))) + [{
        "destination": "/models",
        "type": "bind",
        "source": $model_source,
        "options": ["rbind", "ro"]
     }, {
        "destination": "/sockets",
        "type": "bind",
        "source": $socket_source,
        "options": ["rbind", "rw"]
     }])' \
    "${BUNDLE_DIR}/config.json" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${BUNDLE_DIR}/config.json"

# --- DNS resolution for rootfs ---
ROOTFS_ETC="${ROOTFS_DIR}/etc"
mkdir -p "${ROOTFS_ETC}"
rm -f "${ROOTFS_ETC}/resolv.conf"
if [[ -f "/run/systemd/resolve/resolv.conf" ]]; then
    cp "/run/systemd/resolve/resolv.conf" "${ROOTFS_ETC}/resolv.conf"
elif [[ -f "/etc/resolv.conf" ]]; then
    cp "/etc/resolv.conf" "${ROOTFS_ETC}/resolv.conf"
fi

if [[ ! -s "${ROOTFS_ETC}/resolv.conf" ]] || grep -Eq '^[[:space:]]*nameserver[[:space:]]+(127\.|::1)' "${ROOTFS_ETC}/resolv.conf"; then
    cat > "${ROOTFS_ETC}/resolv.conf" <<'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
options timeout:2 attempts:3 rotate
EOF
fi

# --- Download or locate GGUF model ---
LOCAL_SOURCE_MODEL="$(pick_local_model_path || true)"
MODEL_URL=""

if [[ -n "${LOCAL_SOURCE_MODEL}" ]]; then
    MODEL_FILE="${LLAMA_MODEL_FILE:-$(basename "${LOCAL_SOURCE_MODEL}")}"
    MODEL_PATH="${MODELS_DIR}/${MODEL_FILE}"
    echo "[*] Using preloaded GGUF model"
    echo "    Source: ${LOCAL_SOURCE_MODEL}"
    cp -f "${LOCAL_SOURCE_MODEL}" "${MODEL_PATH}"
    MODEL_URL="local:${LOCAL_SOURCE_MODEL}"
else
    MODEL_URL="$(pick_model_url || true)"
    if [[ -z "${MODEL_URL}" ]]; then
        echo "[ERROR] Failed to find a downloadable GGUF model URL."
        echo "        Put a .gguf in ${INPUT_MODELS_DIR} or set LLAMA_MODEL_URL."
        exit 1
    fi

    MODEL_FILE="${LLAMA_MODEL_FILE:-$(basename "${MODEL_URL%%\?*}")}"
    MODEL_PATH="${MODELS_DIR}/${MODEL_FILE}"

    echo "[*] Downloading GGUF model"
    echo "    URL: ${MODEL_URL}"
    TMP_MODEL="${MODEL_PATH}.partial"
    if ! download_with_retry "${MODEL_URL}" "${TMP_MODEL}"; then
        echo "[ERROR] Failed to download GGUF model after ${DOWNLOAD_ATTEMPTS} attempts"
        exit 1
    fi
    mv "${TMP_MODEL}" "${MODEL_PATH}"
fi

if [[ ! -s "${MODEL_PATH}" ]]; then
    echo "[ERROR] GGUF model is missing or empty at ${MODEL_PATH}"
    exit 1
fi

PROMPT_TEXT="${LLAMA_PROMPT:-Say in one short sentence that batch runc llama demo works.}"

# Save bundle env for later scripts
cat > "${ARTIFACT_DIR}/bundle.env" <<ENVEOF
CONTAINER_NAME='${CONTAINER_NAME}'
MODEL_FILE='${MODEL_FILE}'
MODEL_PATH='${MODEL_PATH}'
ENVEOF

# --- Start runc container ---
echo "[*] Starting OCI container"
(
    cd "${BUNDLE_DIR}"
    ${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    ${SUDO} runc --rootless=false run -d "${CONTAINER_NAME}"
)

if ${SUDO} runc list | awk 'NR>1 {print $1}' | grep -Fxq "${CONTAINER_NAME}"; then
    echo "[+] Container is running: ${CONTAINER_NAME}"
else
    echo "[ERROR] Container did not start correctly."
    exit 1
fi

# --- Install deps and build llama.cpp inside container ---
echo "[*] Installing llama.cpp dependencies inside container"
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
if ! run_exec "cmake --build /opt/llama.cpp/build --target llama-cli llama-server -j 2 >/tmp/llama-cmake-build.log 2>&1"; then
    run_exec "tail -n 120 /tmp/llama-cmake-build.log" || true
    exit 1
fi

# --- Verify binaries exist ---
run_exec "test -x /opt/llama.cpp/build/bin/llama-cli"
run_exec "test -x /opt/llama.cpp/build/bin/llama-server"
echo "[+] llama-cli and llama-server binaries built successfully"

# --- Inference test via UDS ---
SOCKET_PATH="${SOCKETS_DIR}/llama.sock"
SERVER_TIMEOUT=60

echo "[*] Starting llama-server with UDS socket"
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

echo "[DEBUG] llama-server log:"
run_exec "tail -20 /tmp/llama-server.log" || true

echo "[*] Sending inference request via UDS"
prompt_escaped="$(printf '%s' "${PROMPT_TEXT}" | sed 's/"/\\"/g')"

mkdir -p /tmp/vmize-worker/out

set +e
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

if [[ ${curl_status} -ne 0 ]] || [[ "${http_code}" != "200" ]]; then
    echo "[ERROR] UDS inference failed (curl_status=${curl_status}, http_code=${http_code})"
    [[ -s /tmp/vmize-worker/out/llama-error.txt ]] && cat /tmp/vmize-worker/out/llama-error.txt >&2
    [[ -s /tmp/vmize-worker/out/llama-answer.txt ]] && cat /tmp/vmize-worker/out/llama-answer.txt >&2
    exit 1
fi

if ! grep -q '[[:alnum:]]' /tmp/vmize-worker/out/llama-answer.txt; then
    echo "[ERROR] llama-server produced empty output"
    [[ -s /tmp/vmize-worker/out/llama-error.txt ]] && cat /tmp/vmize-worker/out/llama-error.txt >&2
    exit 1
fi

echo "[+] llama-server inference succeeded via UDS"

# Stop container — rootfs at bundle/rootfs/ persists for next scripts
echo "[*] Stopping container"
${SUDO} runc delete -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
# Disarm cleanup trap since we already cleaned up
trap - EXIT

echo "[+] Bundle prepared"
echo "    config: ${BUNDLE_DIR}/config.json"
echo "    rootfs: ${ROOTFS_DIR}"
echo "    model:  ${MODEL_PATH}"
