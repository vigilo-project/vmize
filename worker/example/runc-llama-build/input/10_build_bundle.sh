#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="llama-basic"
BUNDLE_DIR="/tmp/vmize-worker/work/bundle"
ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
MODELS_DIR="${BUNDLE_DIR}/models"
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

TEMP_ROOTFS_TAR="$(mktemp)"
cleanup() {
    rm -f "${TEMP_ROOTFS_TAR}"
}
trap cleanup EXIT

rm -rf "${ROOTFS_DIR}"
mkdir -p "${ROOTFS_DIR}" "${ARTIFACT_DIR}" "${MODELS_DIR}"

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

# Some minimal rootfs archives omit writable temp dirs; apt needs these.
mkdir -p "${ROOTFS_DIR}/tmp" "${ROOTFS_DIR}/var/tmp"
chmod 1777 "${ROOTFS_DIR}/tmp" "${ROOTFS_DIR}/var/tmp"

(
    cd "${BUNDLE_DIR}"
    runc spec
)

echo "[*] Customizing OCI config"
TMP_CONFIG="${BUNDLE_DIR}/config.json.tmp"
jq \
    --arg model_source "${MODELS_DIR}" \
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
     .process.terminal = false |
     .process.noNewPrivileges = false |
     .process.capabilities.bounding = $caps |
     .process.capabilities.effective = $caps |
     .process.capabilities.permitted = $caps |
     .root.readonly = false |
     .linux.namespaces |= map(select(.type != "network")) |
     .mounts = ((.mounts | map(select(.destination != "/models"))) + [{
        "destination": "/models",
        "type": "bind",
        "source": $model_source,
        "options": ["rbind", "ro"]
     }])' \
    "${BUNDLE_DIR}/config.json" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${BUNDLE_DIR}/config.json"

ROOTFS_ETC="${ROOTFS_DIR}/etc"
mkdir -p "${ROOTFS_ETC}"
rm -f "${ROOTFS_ETC}/resolv.conf"
if [[ -f "/run/systemd/resolve/resolv.conf" ]]; then
    cp "/run/systemd/resolve/resolv.conf" "${ROOTFS_ETC}/resolv.conf"
elif [[ -f "/etc/resolv.conf" ]]; then
    cp "/etc/resolv.conf" "${ROOTFS_ETC}/resolv.conf"
fi

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
PROMPT_TEXT_ESCAPED="$(printf '%s' "${PROMPT_TEXT}" | sed "s/'/'\"'\"'/g")"
MODEL_URL_ESCAPED="$(printf '%s' "${MODEL_URL}" | sed "s/'/'\"'\"'/g")"

cat > "${ARTIFACT_DIR}/bundle.env" <<ENVEOF
CONTAINER_NAME='${CONTAINER_NAME}'
MODEL_FILE='${MODEL_FILE}'
MODEL_PATH='${MODEL_PATH}'
MODEL_URL='${MODEL_URL_ESCAPED}'
PROMPT_TEXT='${PROMPT_TEXT_ESCAPED}'
ENVEOF

echo "[+] Bundle prepared"
echo "    config: ${BUNDLE_DIR}/config.json"
echo "    rootfs: ${ROOTFS_DIR}"
echo "    model:  ${MODEL_PATH}"

cp "${ARTIFACT_DIR}/bundle.env" /tmp/vmize-worker/out/bundle.env
cp "${BUNDLE_DIR}/config.json" /tmp/vmize-worker/out/config.json
