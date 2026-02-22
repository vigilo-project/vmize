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
SERVICE_RELAY_SCRIPT="${RUNTIME_DIR}/uds-relay.sh"
SERVICE_RELAY_LOG="${RUNTIME_DIR}/uds-relay.log"
DIRECT_SMOKE_LOG="${RUNTIME_DIR}/direct-smoke.log"

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

CONTAINER_NAME="llama-ima-verified-runtime"
VERITY_NAME="vmize-verity-ima-$$-$(date +%s)"
VERITY_DEVICE="/dev/mapper/${VERITY_NAME}"
SERVICE_SOCKET="vmize_llama_ima_uds_$$-$(date +%s)"
PROMPT_TEXT="${LLAMA_PROMPT:-Say in one short sentence that IMA verified tar runtime stage works.}"

DATA_LOOP=""
HASH_LOOP=""
VERITY_OPENED=0
ROOTFS_MOUNTED=0
BIND_MOUNTED=0
CONTAINER_STARTED=0
SERVICE_PID=""

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

command -v evmctl >/dev/null 2>&1 || { echo "[ERROR] evmctl not found"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }
command -v losetup >/dev/null 2>&1 || { echo "[ERROR] losetup not found"; exit 1; }
command -v mount >/dev/null 2>&1 || { echo "[ERROR] mount not found"; exit 1; }
command -v mountpoint >/dev/null 2>&1 || { echo "[ERROR] mountpoint not found"; exit 1; }
command -v runc >/dev/null 2>&1 || { echo "[ERROR] runc not found"; exit 1; }
command -v socat >/dev/null 2>&1 || { echo "[ERROR] socat not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }
command -v veritysetup >/dev/null 2>&1 || { echo "[ERROR] veritysetup not found"; exit 1; }

cleanup() {
    set +e

    if [[ -n "${SERVICE_PID}" ]]; then
        kill "${SERVICE_PID}" >/dev/null 2>&1 || true
    fi

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
mkdir -p "${PAYLOAD_DIR}" "${VERIFIED_MOUNT}" "${BUNDLE_ROOTFS}" "${MODEL_DIR}"

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

jq \
    --arg model_source "${MODEL_DIR}" \
    '.root.path = "rootfs" |
     .process.args = ["/bin/sh", "-lc", "sleep infinity"] |
     .mounts = (
        (.mounts // [])
        | map(select(.destination != "/models" and .destination != "/tmp"))
        + [{
            "destination": "/models",
            "type": "bind",
            "source": $model_source,
            "options": ["rbind", "ro"]
        }, {
            "destination": "/tmp",
            "type": "tmpfs",
            "source": "tmpfs",
            "options": ["nosuid", "nodev", "mode=1777", "size=256m"]
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

set +e
if [[ -n "${SUDO}" ]]; then
    ${SUDO} runc exec "${CONTAINER_NAME}" /bin/sh -lc \
        "env LD_LIBRARY_PATH=/opt/llama.cpp/build/bin:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu:/usr/lib:/lib /opt/llama.cpp/build/bin/llama-cli -m /models/model.gguf -p 'Say in one short sentence that IMA verified runtime direct smoke works.' -n 32 --temp 0.2 --seed 42 --single-turn --simple-io 2>&1" \
        > "${DIRECT_SMOKE_LOG}" \
        2>&1
else
    runc exec "${CONTAINER_NAME}" /bin/sh -lc \
        "env LD_LIBRARY_PATH=/opt/llama.cpp/build/bin:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu:/usr/lib:/lib /opt/llama.cpp/build/bin/llama-cli -m /models/model.gguf -p 'Say in one short sentence that IMA verified runtime direct smoke works.' -n 32 --temp 0.2 --seed 42 --single-turn --simple-io 2>&1" \
        > "${DIRECT_SMOKE_LOG}" \
        2>&1
fi
direct_status=$?
set -e

if [[ ${direct_status} -ne 0 ]] || ! grep -q '[[:alnum:]]' "${DIRECT_SMOKE_LOG}"; then
    cp -f "${DIRECT_SMOKE_LOG}" "${OUTPUT_SERVICE_LOG}" || true
    echo "[ERROR] direct llama smoke run failed (status=${direct_status})"
    [[ -s "${DIRECT_SMOKE_LOG}" ]] && cat "${DIRECT_SMOKE_LOG}" >&2
    exit 1
fi

cat > "${SERVICE_RELAY_SCRIPT}" <<EOF_RELAY
#!/usr/bin/env bash
set -euo pipefail

IFS= read -r PROMPT || true
if [[ -z "\${PROMPT}" ]]; then
    PROMPT="Say in one short sentence that IMA verified runtime stage works."
fi

PROMPT_ESCAPED="\$(printf '%s' "\${PROMPT}" | sed "s/'/'\"'\"'/g")"
RUNTIME_LD_LIBRARY_PATH="/opt/llama.cpp/build/bin:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu:/usr/lib:/lib"

if [[ -n "${SUDO}" ]]; then
    ${SUDO} runc exec "${CONTAINER_NAME}" /bin/sh -lc \\
        "env LD_LIBRARY_PATH=\${RUNTIME_LD_LIBRARY_PATH} /opt/llama.cpp/build/bin/llama-cli -m /models/model.gguf -p '\${PROMPT_ESCAPED}' -n 48 --temp 0.2 --seed 42 --single-turn --simple-io 2>&1"
else
    runc exec "${CONTAINER_NAME}" /bin/sh -lc \\
        "env LD_LIBRARY_PATH=\${RUNTIME_LD_LIBRARY_PATH} /opt/llama.cpp/build/bin/llama-cli -m /models/model.gguf -p '\${PROMPT_ESCAPED}' -n 48 --temp 0.2 --seed 42 --single-turn --simple-io 2>&1"
fi
EOF_RELAY
chmod +x "${SERVICE_RELAY_SCRIPT}"

: > "${SERVICE_RELAY_LOG}"
nohup socat -d -d "ABSTRACT-LISTEN:${SERVICE_SOCKET},fork" "EXEC:${SERVICE_RELAY_SCRIPT},stderr" > "${SERVICE_RELAY_LOG}" 2>&1 &
SERVICE_PID=$!

socket_ready=0
for _ in $(seq 1 30); do
    if grep -Eq "@${SERVICE_SOCKET}$" /proc/net/unix; then
        socket_ready=1
        break
    fi
    sleep 1
done

if [[ ${socket_ready} -ne 1 ]]; then
    cp -f "${SERVICE_RELAY_LOG}" "${OUTPUT_SERVICE_LOG}" || true
    echo "[ERROR] abstract UDS service did not become ready: @${SERVICE_SOCKET}"
    exit 1
fi

printf '%s\n' "${PROMPT_TEXT}" > "${OUTPUT_PROMPT}"

set +e
printf '%s\n' "${PROMPT_TEXT}" \
    | socat "STDIO,ignoreeof" "ABSTRACT-CONNECT:${SERVICE_SOCKET}" \
    > "${OUTPUT_ANSWER}" \
    2> "${OUTPUT_ERROR}"
client_status=$?
set -e

${SUDO} runc list > "${OUTPUT_RUNC_LIST}" 2>&1 || true
cp -f "${SERVICE_RELAY_LOG}" "${OUTPUT_SERVICE_LOG}" || true

if [[ ${client_status} -ne 0 ]] || ! grep -q '[[:alnum:]]' "${OUTPUT_ANSWER}"; then
    echo "[ERROR] UDS llama inference failed (client_status=${client_status})"
    [[ -s "${OUTPUT_ERROR}" ]] && cat "${OUTPUT_ERROR}" >&2
    exit 1
fi

{
    echo "mode=stage5-ima-verified-runtime"
    echo "verified_tar=${INPUT_SIGNED_TAR}"
    echo "cert=${INPUT_CERT}"
    echo "verity_data_loop=${DATA_LOOP}"
    echo "verity_hash_loop=${HASH_LOOP}"
    echo "verity_name=${VERITY_NAME}"
    echo "verity_device=${VERITY_DEVICE}"
    echo "uds_socket=@${SERVICE_SOCKET}"
    echo "prompt=${PROMPT_TEXT}"
    echo "client_status=${client_status}"
} > "${OUTPUT_SUMMARY}"

echo "[+] IMA verification and runtime UDS inference completed"
