#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUT_DIR="/tmp/vmize-worker/out"
TMP_DIR="${WORK_DIR}/ima-sign"
SIGNED_DIR="${TMP_DIR}/signed"
HTTP_ROOT="${TMP_DIR}/http-root"
DOWNLOAD_DIR="${TMP_DIR}/download"
EXTRACT_DIR="${TMP_DIR}/extract"
NO_XATTR_DIR="${TMP_DIR}/no-xattr-extract"

SAMPLE_A="${SIGNED_DIR}/sample-a.txt"
SAMPLE_B="${SIGNED_DIR}/sample-b.bin"
TAMPERED_A="${TMP_DIR}/sample-a-tampered.txt"
KEY_PEM="${TMP_DIR}/key.pem"
CERT_PEM="${TMP_DIR}/cert.pem"
CERT_DER="${TMP_DIR}/cert.der"

SIGN_LOG="${TMP_DIR}/ima-sign.log"
VERIFY_LOG="${TMP_DIR}/ima-verify.log"
NEGATIVE_LOG="${TMP_DIR}/ima-negative.log"
ROUNDTRIP_LOG="${TMP_DIR}/ima-http-roundtrip.log"
NO_XATTR_LOG="${TMP_DIR}/ima-no-xattr.log"
XATTR_LOG="${TMP_DIR}/signed-xattr.txt"
SUMMARY_FILE="${OUT_DIR}/ima-sign-summary.txt"

SIGNED_TAR="${HTTP_ROOT}/signed-http.tar"
DOWNLOADED_TAR="${DOWNLOAD_DIR}/downloaded-signed-http.tar"
NO_XATTR_TAR="${HTTP_ROOT}/no-xattr.tar"

SERVER_PID=""
HTTP_PORT=""

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

cleanup() {
    set +e
    if [[ -n "${SERVER_PID}" ]]; then
        kill "${SERVER_PID}" >/dev/null 2>&1 || true
        wait "${SERVER_PID}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

command -v evmctl >/dev/null 2>&1 || { echo "[ERROR] evmctl not found"; exit 1; }
command -v getfattr >/dev/null 2>&1 || { echo "[ERROR] getfattr not found"; exit 1; }
command -v openssl >/dev/null 2>&1 || { echo "[ERROR] openssl not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "[ERROR] curl not found"; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "[ERROR] python3 not found"; exit 1; }

ensure_ima_runtime_available() {
    if [[ ! -d "/sys/kernel/security" ]]; then
        echo "[ERROR] /sys/kernel/security not present"
        exit 1
    fi

    if ! grep -q " /sys/kernel/security securityfs " /proc/mounts; then
        echo "[*] Mounting securityfs"
        ${SUDO} mount -t securityfs securityfs /sys/kernel/security || true
    fi

    if [[ ! -d "/sys/kernel/security/ima" ]]; then
        echo "[ERROR] IMA runtime interface not available at /sys/kernel/security/ima"
        exit 1
    fi
}

verify_signed_with_cert() {
    local target="$1"
    if [[ -n "${SUDO}" ]]; then
        ${SUDO} evmctl -v --key "${CERT_DER}" ima_verify "${target}"
    else
        evmctl -v --key "${CERT_DER}" ima_verify "${target}"
    fi
}

dump_security_ima_xattr() {
    local label="$1"
    local target="$2"
    {
        echo "### ${label}"
        set +e
        if [[ -n "${SUDO}" ]]; then
            ${SUDO} getfattr -m . -n security.ima -e hex "${target}" 2>&1
        else
            getfattr -m . -n security.ima -e hex "${target}" 2>&1
        fi
        set -e
        echo
    } >> "${XATTR_LOG}"
}

rm -rf "${TMP_DIR}"
mkdir -p "${SIGNED_DIR}" "${HTTP_ROOT}" "${DOWNLOAD_DIR}" "${EXTRACT_DIR}" "${NO_XATTR_DIR}"

ensure_ima_runtime_available

printf 'VMize IMA sample A\n' > "${SAMPLE_A}"
head -c 512 /dev/urandom > "${SAMPLE_B}"

openssl req -new -x509 -newkey rsa:2048 -keyout "${KEY_PEM}" -out "${CERT_PEM}" -nodes -days 365 -subj "/CN=vmize-ima-sign/" >/dev/null 2>&1
openssl x509 -in "${CERT_PEM}" -outform DER -out "${CERT_DER}" >/dev/null 2>&1

: > "${SIGN_LOG}"
: > "${VERIFY_LOG}"
: > "${NEGATIVE_LOG}"
: > "${ROUNDTRIP_LOG}"
: > "${NO_XATTR_LOG}"
: > "${XATTR_LOG}"

for target in "${SAMPLE_A}" "${SAMPLE_B}"; do
    echo "[*] Signing ${target}" | tee -a "${SIGN_LOG}"
    if [[ -n "${SUDO}" ]]; then
        ${SUDO} evmctl ima_sign --key "${KEY_PEM}" "${target}" >> "${SIGN_LOG}" 2>&1
    else
        evmctl ima_sign --key "${KEY_PEM}" "${target}" >> "${SIGN_LOG}" 2>&1
    fi

    echo "[*] Verifying ${target}" | tee -a "${VERIFY_LOG}"
    verify_signed_with_cert "${target}" >> "${VERIFY_LOG}" 2>&1
    dump_security_ima_xattr "original:${target##*/}" "${target}"
done

cp -f "${SAMPLE_A}" "${TAMPERED_A}"
printf 'tampered\n' >> "${TAMPERED_A}"

echo "[*] Verifying tampered file ${TAMPERED_A}" | tee -a "${NEGATIVE_LOG}"
set +e
verify_signed_with_cert "${TAMPERED_A}" >> "${NEGATIVE_LOG}" 2>&1
tampered_status=$?
set -e

if [[ ${tampered_status} -eq 0 ]]; then
    echo "[ERROR] tampered file verification unexpectedly succeeded" | tee -a "${NEGATIVE_LOG}" >&2
    exit 1
fi

echo "[*] Tampered verification failed as expected (status=${tampered_status})" | tee -a "${NEGATIVE_LOG}"

echo "[*] Packing signed files with xattrs" | tee -a "${ROUNDTRIP_LOG}"
if [[ -n "${SUDO}" ]]; then
    ${SUDO} tar --xattrs --xattrs-include='*' --numeric-owner --format=posix -cpf "${SIGNED_TAR}" -C "${SIGNED_DIR}" sample-a.txt sample-b.bin >> "${ROUNDTRIP_LOG}" 2>&1
else
    tar --xattrs --xattrs-include='*' --numeric-owner --format=posix -cpf "${SIGNED_TAR}" -C "${SIGNED_DIR}" sample-a.txt sample-b.bin >> "${ROUNDTRIP_LOG}" 2>&1
fi

HTTP_PORT="$(python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"

python3 -m http.server "${HTTP_PORT}" --bind 127.0.0.1 --directory "${HTTP_ROOT}" > "${TMP_DIR}/http-server.log" 2>&1 &
SERVER_PID=$!

server_ready=0
for _ in $(seq 1 20); do
    if curl -fsS "http://127.0.0.1:${HTTP_PORT}/signed-http.tar" -o /dev/null >/dev/null 2>&1; then
        server_ready=1
        break
    fi
    sleep 1
done

if [[ ${server_ready} -ne 1 ]]; then
    echo "[ERROR] HTTP server failed to serve signed-http.tar" | tee -a "${ROUNDTRIP_LOG}" >&2
    exit 1
fi

echo "[*] Downloading tar over HTTP" | tee -a "${ROUNDTRIP_LOG}"
curl -fsSLo "${DOWNLOADED_TAR}" "http://127.0.0.1:${HTTP_PORT}/signed-http.tar" >> "${ROUNDTRIP_LOG}" 2>&1

if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
    SERVER_PID=""
fi

echo "[*] Extracting downloaded tar with xattrs" | tee -a "${ROUNDTRIP_LOG}"
if [[ -n "${SUDO}" ]]; then
    ${SUDO} tar --xattrs --xattrs-include='*' -xpf "${DOWNLOADED_TAR}" -C "${EXTRACT_DIR}" >> "${ROUNDTRIP_LOG}" 2>&1
else
    tar --xattrs --xattrs-include='*' -xpf "${DOWNLOADED_TAR}" -C "${EXTRACT_DIR}" >> "${ROUNDTRIP_LOG}" 2>&1
fi

for target_name in sample-a.txt sample-b.bin; do
    extracted_target="${EXTRACT_DIR}/${target_name}"
    [[ -f "${extracted_target}" ]] || { echo "[ERROR] Missing extracted file: ${extracted_target}" | tee -a "${ROUNDTRIP_LOG}" >&2; exit 1; }

    echo "[*] Verifying extracted ${extracted_target}" | tee -a "${ROUNDTRIP_LOG}"
    verify_signed_with_cert "${extracted_target}" >> "${ROUNDTRIP_LOG}" 2>&1
    dump_security_ima_xattr "roundtrip:${target_name}" "${extracted_target}"
done

echo "[*] Stopping HTTP server after positive roundtrip test"
if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
    SERVER_PID=""
fi

echo "[*] Negative test: packing without xattrs" | tee -a "${NO_XATTR_LOG}"
if [[ -n "${SUDO}" ]]; then
    ${SUDO} tar --numeric-owner --format=posix -cpf "${NO_XATTR_TAR}" -C "${SIGNED_DIR}" sample-a.txt sample-b.bin >> "${NO_XATTR_LOG}" 2>&1
else
    tar --numeric-owner --format=posix -cpf "${NO_XATTR_TAR}" -C "${SIGNED_DIR}" sample-a.txt sample-b.bin >> "${NO_XATTR_LOG}" 2>&1
fi

echo "[*] Restarting HTTP server for no-xattr test" | tee -a "${NO_XATTR_LOG}"
HTTP_PORT="$(python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"

python3 -m http.server "${HTTP_PORT}" --bind 127.0.0.1 --directory "${HTTP_ROOT}" > "${TMP_DIR}/http-server-no-xattr.log" 2>&1 &
SERVER_PID=$!

server_ready=0
for _ in $(seq 1 20); do
    if curl -fsS "http://127.0.0.1:${HTTP_PORT}/no-xattr.tar" -o /dev/null >/dev/null 2>&1; then
        server_ready=1
        break
    fi
    sleep 1
done

if [[ ${server_ready} -ne 1 ]]; then
    echo "[ERROR] HTTP server failed to serve no-xattr.tar" | tee -a "${NO_XATTR_LOG}" >&2
    exit 1
fi

NO_XATTR_DOWNLOADED="${DOWNLOAD_DIR}/downloaded-no-xattr.tar"
echo "[*] Downloading no-xattr tar over HTTP" | tee -a "${NO_XATTR_LOG}"
curl -fsSLo "${NO_XATTR_DOWNLOADED}" "http://127.0.0.1:${HTTP_PORT}/no-xattr.tar" >> "${NO_XATTR_LOG}" 2>&1

echo "[*] Extracting no-xattr tar (without xattrs)" | tee -a "${NO_XATTR_LOG}"
if [[ -n "${SUDO}" ]]; then
    ${SUDO} tar -xpf "${NO_XATTR_DOWNLOADED}" -C "${NO_XATTR_DIR}" >> "${NO_XATTR_LOG}" 2>&1
else
    tar -xpf "${NO_XATTR_DOWNLOADED}" -C "${NO_XATTR_DIR}" >> "${NO_XATTR_LOG}" 2>&1
fi

no_xattr_verify_failed=0
for target_name in sample-a.txt sample-b.bin; do
    no_xattr_target="${NO_XATTR_DIR}/${target_name}"
    [[ -f "${no_xattr_target}" ]] || { echo "[ERROR] Missing extracted file: ${no_xattr_target}" | tee -a "${NO_XATTR_LOG}" >&2; exit 1; }

    echo "[*] Attempting to verify extracted (no-xattr) ${no_xattr_target}" | tee -a "${NO_XATTR_LOG}"
    set +e
    verify_signed_with_cert "${no_xattr_target}" >> "${NO_XATTR_LOG}" 2>&1
    no_xattr_status=$?
    set -e

    if [[ ${no_xattr_status} -eq 0 ]]; then
        echo "[ERROR] no-xattr file verification unexpectedly succeeded (should have no security.ima)" | tee -a "${NO_XATTR_LOG}" >&2
        no_xattr_verify_failed=1
    else
        echo "[*] No-xattr verification failed as expected (status=${no_xattr_status})" | tee -a "${NO_XATTR_LOG}"
    fi
    dump_security_ima_xattr "no-xattr:${target_name}" "${no_xattr_target}"
done

if [[ ${no_xattr_verify_failed} -eq 1 ]]; then
    exit 1
fi


cp -f "${SIGN_LOG}" "${OUT_DIR}/ima-sign.log"
cp -f "${VERIFY_LOG}" "${OUT_DIR}/ima-verify.log"
cp -f "${NEGATIVE_LOG}" "${OUT_DIR}/ima-negative.log"
cp -f "${ROUNDTRIP_LOG}" "${OUT_DIR}/ima-http-roundtrip.log"
cp -f "${NO_XATTR_LOG}" "${OUT_DIR}/ima-no-xattr.log"
cp -f "${XATTR_LOG}" "${OUT_DIR}/signed-xattr.txt"
cp -f "${CERT_DER}" "${OUT_DIR}/cert.der"
cp -f "${SAMPLE_A}" "${OUT_DIR}/sample-a.txt"
cp -f "${SAMPLE_B}" "${OUT_DIR}/sample-b.bin"
cp -f "${SIGNED_TAR}" "${OUT_DIR}/signed-http.tar"
cp -f "${DOWNLOADED_TAR}" "${OUT_DIR}/downloaded-signed-http.tar"
cp -f "${NO_XATTR_TAR}" "${OUT_DIR}/no-xattr.tar"
cp -f "${NO_XATTR_DOWNLOADED}" "${OUT_DIR}/downloaded-no-xattr.tar"

{
    echo "mode=debug-verify"
    echo "appraise_policy=not-enabled"
    echo "signing=success"
    echo "positive verify: success"
    echo "tampered verify: expected failure"
    echo "tampered_verify_exit_status=${tampered_status}"
    echo "tar_bundle=tar --xattrs --xattrs-include='*' --format=posix"
    echo "http_roundtrip=success"
    echo "http_roundtrip_verify=success"
    echo "no_xattr_tar=tar (without --xattrs)"
    echo "no_xattr_verify: expected failure"
    echo "no_xattr_verify_failed=${no_xattr_verify_failed}"
} > "${SUMMARY_FILE}"

rm -f "${KEY_PEM}" "${CERT_PEM}" "${TAMPERED_A}"

echo "[+] IMA sign + HTTP tar roundtrip verification completed"
