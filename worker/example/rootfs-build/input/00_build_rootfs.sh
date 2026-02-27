#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUTPUT_DIR="/tmp/vmize-worker/out"
ROOTFS_TMP="${WORK_DIR}/rootfs"
ROOTFS_RAW="${WORK_DIR}/rootfs.raw"
OUTPUT_QCOW2="${OUTPUT_DIR}/rootfs.qcow2"
OUTPUT_RAW="${OUTPUT_DIR}/rootfs.raw"
INFO_FILE="${OUTPUT_DIR}/rootfs-build-info.txt"
ROOTFS_SOURCE="${ROOTFS_SOURCE:-}"
ROOTFS_SIZE="${ROOTFS_SIZE:-20G}"
ROOTFS_FORMAT="${ROOTFS_FORMAT:-qcow2}"

if [[ "$(id -u)" -ne 0 ]]; then
  SUDO="sudo"
else
  SUDO=""
fi

HOST_ARCH="$(uname -m)"

if [[ -z "${ROOTFS_SOURCE}" ]]; then
  case "${HOST_ARCH}" in
    x86_64)
      ROOTFS_SOURCE="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-amd64-root.tar.xz"
      ;;
    aarch64|arm64)
      ROOTFS_SOURCE="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-arm64-root.tar.xz"
      ;;
    *)
      echo "[ERROR] Unsupported host architecture: ${HOST_ARCH}" >&2
      exit 1
      ;;
  esac
fi

if [[ "${ROOTFS_FORMAT}" != "raw" && "${ROOTFS_FORMAT}" != "qcow2" ]]; then
  echo "[ERROR] ROOTFS_FORMAT must be raw or qcow2" >&2
  exit 1
fi

run_with_retries() {
  local attempts="$1"
  local description="$2"
  shift 2

  local attempt
  for attempt in $(seq 1 "$attempts"); do
    if "$@"; then
      return 0
    fi

    echo "[!] ${description} failed (${attempt}/${attempts})"
    if (( attempt < attempts )); then
      local sleep_seconds=$((attempt * 2))
      echo "[*] Retrying in ${sleep_seconds}s"
      sleep "${sleep_seconds}"
    fi
  done

  echo "[ERROR] ${description} failed after ${attempts} attempts" >&2
  return 1
}

ensure_deps() {
  if ! command -v apt-get >/dev/null 2>&1; then
    echo "[ERROR] This script expects Ubuntu/Debian guest for package install" >&2
    exit 1
  fi

  echo "[*] Installing rootfs toolchain dependencies"
  run_with_retries 3 "installing build deps" \
    "${SUDO}" env DEBIAN_FRONTEND=noninteractive apt-get update

  run_with_retries 3 "installing rootfs deps" \
    "${SUDO}" env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      e2fsprogs \
      qemu-utils \
      tar \
      wget \
      xz-utils
}

fetch_rootfs_source() {
  local archive_path="${WORK_DIR}/rootfs.tar.xz"

  if [[ -f "${ROOTFS_SOURCE}" ]]; then
    cp "${ROOTFS_SOURCE}" "${archive_path}"
    echo "[+] Using local rootfs source: ${ROOTFS_SOURCE}"
    ROOTFS_ARCHIVE="${archive_path}"
    return
  fi

  if [[ "${ROOTFS_SOURCE}" == http://* || "${ROOTFS_SOURCE}" == https://* ]]; then
    echo "[*] Downloading rootfs source from ${ROOTFS_SOURCE}"
    if command -v wget >/dev/null 2>&1; then
      run_with_retries 3 "download rootfs" wget -O "${archive_path}" "${ROOTFS_SOURCE}"
    else
      run_with_retries 3 "download rootfs" curl -fsSL -o "${archive_path}" "${ROOTFS_SOURCE}"
    fi
    ROOTFS_ARCHIVE="${archive_path}"
    return
  fi

  echo "[ERROR] ROOTFS_SOURCE must be a local file path or URL: ${ROOTFS_SOURCE}" >&2
  exit 1
}

build_rootfs() {
  local extract_dir="${ROOTFS_TMP}/tree"

  rm -rf "${ROOTFS_TMP}" "${ROOTFS_RAW}"
  mkdir -p "${extract_dir}"

  echo "[*] Extracting rootfs tar archive"
  # Preserve ownership/permissions from the Ubuntu rootfs tarball.
  # Using --no-same-owner/permissions breaks sudo/sshd and many base files.
  "${SUDO}" tar --exclude='dev/*' --exclude='./dev/*' \
    --acls --xattrs --numeric-owner \
    -xaf "${ROOTFS_ARCHIVE}" -C "${extract_dir}"

  if command -v qemu-img >/dev/null 2>&1; then
    echo "[*] Creating raw image ${ROOTFS_RAW} (${ROOTFS_SIZE})"
    qemu-img create -f raw "${ROOTFS_RAW}" "${ROOTFS_SIZE}"
  else
    echo "[*] qemu-img not found, creating raw image with truncate"
    truncate -s "${ROOTFS_SIZE}" "${ROOTFS_RAW}"
  fi

  echo "[*] Formatting rootfs image as ext4 and populating contents"
  # mkfs must read root-owned files (e.g. /etc/shadow, host keys).
  "${SUDO}" mkfs.ext4 -F -d "${extract_dir}" "${ROOTFS_RAW}"

  if [[ "${ROOTFS_FORMAT}" == "qcow2" ]]; then
    if ! command -v qemu-img >/dev/null 2>&1; then
      echo "[ERROR] qcow2 output requested but qemu-img is not installed" >&2
      exit 1
    fi

    echo "[*] Converting raw rootfs to qcow2"
    qemu-img convert -f raw -O qcow2 "${ROOTFS_RAW}" "${OUTPUT_QCOW2}"
    echo "[*] Rootfs image created: ${OUTPUT_DIR}/rootfs.qcow2"
  else
    cp "${ROOTFS_RAW}" "${OUTPUT_RAW}"
    cp "${ROOTFS_RAW}" "${OUTPUT_QCOW2}"
    echo "[*] Rootfs raw image created: ${OUTPUT_RAW}"
    echo "[*] Compatibility copy created: ${OUTPUT_DIR}/rootfs.qcow2"
  fi

  printf 'rootfs_source=%s\n' "${ROOTFS_SOURCE}" > "${INFO_FILE}"
  printf 'rootfs_size=%s\n' "${ROOTFS_SIZE}" >> "${INFO_FILE}"
  printf 'rootfs_format=%s\n' "${ROOTFS_FORMAT}" >> "${INFO_FILE}"
  printf 'rootfs_output=%s\n' "${OUTPUT_DIR}/rootfs.qcow2" >> "${INFO_FILE}"
  printf 'host_arch=%s\n' "${HOST_ARCH}" >> "${INFO_FILE}"
  printf 'built_at=%s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" >> "${INFO_FILE}"
}

ensure_deps
mkdir -p "${WORK_DIR}" "${OUTPUT_DIR}"
fetch_rootfs_source
build_rootfs
