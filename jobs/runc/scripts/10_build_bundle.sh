#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="basic"
BUNDLE_DIR="/tmp/vm-batch/work/bundle"

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

command -v runc >/dev/null 2>&1 || { echo "[ERROR] runc not found"; exit 1; }
command -v wget >/dev/null 2>&1 || { echo "[ERROR] wget not found"; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "[ERROR] tar not found"; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "[ERROR] jq not found"; exit 1; }

ARTIFACT_DIR="${BUNDLE_DIR}/artifacts"
TEMP_ROOTFS_TAR="$(mktemp)"

cleanup() {
    rm -f "$TEMP_ROOTFS_TAR"
}
trap cleanup EXIT

rm -rf "${BUNDLE_DIR}/rootfs"
mkdir -p "${BUNDLE_DIR}/rootfs"
mkdir -p "${ARTIFACT_DIR}"

echo "[*] Downloading Ubuntu rootfs"
echo "    URL: ${ROOTFS_URL}"
wget -q --show-progress -O "${TEMP_ROOTFS_TAR}" "${ROOTFS_URL}"

echo "[*] Extracting rootfs"
tar -xJf "${TEMP_ROOTFS_TAR}" -C "${BUNDLE_DIR}/rootfs" --exclude='dev' --exclude='dev/*'

mkdir -p "${BUNDLE_DIR}/rootfs/dev"
touch \
    "${BUNDLE_DIR}/rootfs/dev/null" \
    "${BUNDLE_DIR}/rootfs/dev/zero" \
    "${BUNDLE_DIR}/rootfs/dev/full" \
    "${BUNDLE_DIR}/rootfs/dev/random" \
    "${BUNDLE_DIR}/rootfs/dev/urandom"

(
    cd "${BUNDLE_DIR}"
    runc spec
)

echo "[*] Customizing bundle config for ${CONTAINER_NAME}"

TMP_CONFIG="${BUNDLE_DIR}/config.json.tmp"
jq '.process.args = ["/bin/sh", "-c", "sleep infinity"]' \
    "${BUNDLE_DIR}/config.json" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${BUNDLE_DIR}/config.json"

jq '.linux.namespaces |= map(select(.type != "network"))' \
    "${BUNDLE_DIR}/config.json" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${BUNDLE_DIR}/config.json"

jq '.process.capabilities.bounding += ["CAP_SETUID", "CAP_SETGID", "CAP_CHOWN", "CAP_FOWNER", "CAP_DAC_OVERRIDE"] |
    .process.capabilities.effective += ["CAP_SETUID", "CAP_SETGID", "CAP_CHOWN", "CAP_FOWNER", "CAP_DAC_OVERRIDE"] |
    .process.capabilities.permitted += ["CAP_SETUID", "CAP_SETGID", "CAP_CHOWN", "CAP_FOWNER", "CAP_DAC_OVERRIDE"]' \
    "${BUNDLE_DIR}/config.json" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${BUNDLE_DIR}/config.json"

jq '.process.terminal = false' "${BUNDLE_DIR}/config.json" > "${TMP_CONFIG}"
mv "${TMP_CONFIG}" "${BUNDLE_DIR}/config.json"

ROOTFS_ETC="${BUNDLE_DIR}/rootfs/etc"
mkdir -p "${ROOTFS_ETC}"
rm -f "${ROOTFS_ETC}/resolv.conf"
if [[ -f "/run/systemd/resolve/resolv.conf" ]]; then
    cp "/run/systemd/resolve/resolv.conf" "${ROOTFS_ETC}/resolv.conf"
elif [[ -f "/etc/resolv.conf" ]]; then
    cp "/etc/resolv.conf" "${ROOTFS_ETC}/resolv.conf"
fi

echo "[*] Creating rootfs.tar.xz artifact"
(
    cd "${BUNDLE_DIR}/rootfs"
    tar -cJf "${ARTIFACT_DIR}/rootfs.tar.xz" . --ignore-failed-read
)

cp "${BUNDLE_DIR}/config.json" "${ARTIFACT_DIR}/config.json"

echo "[+] Bundle created at ${BUNDLE_DIR}"
echo "    Container name: ${CONTAINER_NAME}"
echo "    Artifact: ${ARTIFACT_DIR}/config.json"
echo "    Artifact: ${ARTIFACT_DIR}/rootfs.tar.xz"

# Copy artifacts to vm-batch output directory
cp "${ARTIFACT_DIR}/config.json" /tmp/vm-batch/out/config.json
cp "${ARTIFACT_DIR}/rootfs.tar.xz" /tmp/vm-batch/out/rootfs.tar.xz
