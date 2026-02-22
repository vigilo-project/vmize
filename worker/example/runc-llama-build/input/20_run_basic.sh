#!/usr/bin/env bash
set -euo pipefail

BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
BUNDLE_ENV="${ARTIFACT_DIR}/bundle.env"

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
run_exec "cat > /etc/resolv.conf <<'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
options timeout:2 attempts:3 rotate
EOF"
run_exec_with_retry \
    "container apt dependencies install" \
    "export DEBIAN_FRONTEND=noninteractive; \
    apt-get -qq update -o Acquire::Retries=3 -o Acquire::http::Timeout=20; \
    apt-get -qq install -y --no-install-recommends build-essential cmake git ca-certificates pkg-config" \
    5

echo "[*] Building llama.cpp inside container rootfs"
run_exec "if [ ! -d /opt/llama.cpp/.git ]; then git clone --depth 1 --quiet https://github.com/ggerganov/llama.cpp /opt/llama.cpp; fi"
if ! run_exec "cmake -S /opt/llama.cpp -B /opt/llama.cpp/build -DCMAKE_BUILD_TYPE=Release -DGGML_NATIVE=OFF >/tmp/llama-cmake-config.log 2>&1"; then
    run_exec "tail -n 120 /tmp/llama-cmake-config.log" || true
    exit 1
fi
if ! run_exec "cmake --build /opt/llama.cpp/build --target llama-cli -j 2 >/tmp/llama-cmake-build.log 2>&1"; then
    run_exec "tail -n 120 /tmp/llama-cmake-build.log" || true
    exit 1
fi

run_exec "test -x /opt/llama.cpp/build/bin/llama-cli"

prompt_escaped="$(printf '%s' "${PROMPT_TEXT}" | sed "s/'/'\"'\"'/g")"

echo "[*] Running prompt against mounted model /models/${MODEL_FILE}"
# Keep subprocess output deterministic and force one-shot exit.
run_exec "/opt/llama.cpp/build/bin/llama-cli -m /models/${MODEL_FILE} -p '${prompt_escaped}' -n 48 --temp 0.2 --seed 42 --single-turn --simple-io" \
    > /tmp/vmize-worker/out/llama-answer.txt \
    2> /tmp/vmize-worker/out/llama-error.txt

if ! grep -q '[[:alnum:]]' /tmp/vmize-worker/out/llama-answer.txt; then
    echo "[ERROR] llama.cpp produced empty output"
    if [[ -s /tmp/vmize-worker/out/llama-error.txt ]]; then
        cat /tmp/vmize-worker/out/llama-error.txt >&2
    fi
    exit 1
fi

${SUDO} runc list > /tmp/vmize-worker/out/runc-list.txt
run_exec "/opt/llama.cpp/build/bin/llama-cli --version" > /tmp/vmize-worker/out/llama-version.txt 2>&1 || true
printf '%s\n' "${PROMPT_TEXT}" > /tmp/vmize-worker/out/prompt.txt

echo "[+] llama.cpp prompt execution succeeded"
