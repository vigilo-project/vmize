#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
OUTPUT_DIR="/tmp/vmize-worker/out"
KERNEL_DIR="${WORK_DIR}/linux"
OUTPUT_IMAGE="${OUTPUT_DIR}/kernel"
INFO_FILE="${OUTPUT_DIR}/kernel-build-info.txt"
KERNEL_REPO="${KERNEL_REPO:-https://github.com/torvalds/linux.git}"
KERNEL_BRANCH="${KERNEL_BRANCH:-master}"
FORCE_REBUILD="${FORCE_REBUILD:-false}"
JOBS="${JOBS:-$(command -v nproc >/dev/null 2>&1 && nproc || echo 4)}"

if [[ "$(id -u)" -ne 0 ]]; then
  SUDO="sudo"
else
  SUDO=""
fi

HOST_ARCH="$(uname -m)"

case "${HOST_ARCH}" in
  x86_64)
    KERNEL_ARCH="x86_64"
    KERNEL_DEFCONFIG="x86_64_defconfig"
    KERNEL_TARGET="bzImage"
    KERNEL_REL_PATH="arch/x86/boot/bzImage"
    ;;
  aarch64|arm64)
    KERNEL_ARCH="arm64"
    KERNEL_DEFCONFIG="defconfig"
    KERNEL_TARGET="Image"
    KERNEL_REL_PATH="arch/arm64/boot/Image"
    ;;
  *)
    echo "[ERROR] Unsupported architecture for kernel build: ${HOST_ARCH}" >&2
    exit 1
    ;;
esac

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

  echo "[*] Installing kernel build dependencies"
  run_with_retries 3 "installing kernel build deps" \
    "${SUDO}" env DEBIAN_FRONTEND=noninteractive apt-get update

  run_with_retries 3 "installing kernel build deps" \
    "${SUDO}" env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      bc \
      bison \
      build-essential \
      ca-certificates \
      curl \
      flex \
      git \
      libelf-dev \
      libncurses-dev \
      libssl-dev \
      python3 \
      rsync \
      xz-utils
}

prepare_tree() {
  mkdir -p "${WORK_DIR}" "${OUTPUT_DIR}"

  if [[ -d "${KERNEL_DIR}/.git" ]]; then
    echo "[+] Updating existing Linux tree"
    git -C "${KERNEL_DIR}" fetch --depth=1 origin "${KERNEL_BRANCH}"
    git -C "${KERNEL_DIR}" checkout "origin/${KERNEL_BRANCH}"
  else
    echo "[+] Cloning Linux from ${KERNEL_REPO} (${KERNEL_BRANCH})"
    rm -rf "${KERNEL_DIR}"
    git clone --depth=1 --branch "${KERNEL_BRANCH}" "${KERNEL_REPO}" "${KERNEL_DIR}"
  fi

  if [[ "${FORCE_REBUILD}" == "true" ]]; then
    make -C "${KERNEL_DIR}" clean
  fi
}

build_kernel() {
  local source_path="${KERNEL_DIR}/${KERNEL_REL_PATH}"
  local config_script="${KERNEL_DIR}/scripts/config"

  echo "[+] Configuring kernel: ${KERNEL_ARCH} ${KERNEL_DEFCONFIG}"
  make -C "${KERNEL_DIR}" ARCH="${KERNEL_ARCH}" "${KERNEL_DEFCONFIG}"

  if [[ ! -x "${config_script}" ]]; then
    echo "[ERROR] Missing kernel config helper: ${config_script}" >&2
    exit 1
  fi

  echo "[+] Applying kernel config overrides for VM direct boot + cloud-init seed"
  "${config_script}" --file "${KERNEL_DIR}/.config" \
    --enable DEVTMPFS \
    --enable DEVTMPFS_MOUNT \
    --enable EXT4_FS \
    --enable FAT_FS \
    --enable MSDOS_FS \
    --enable VFAT_FS \
    --enable ISO9660_FS \
    --enable JOLIET \
    --enable ZISOFS \
    --enable SQUASHFS \
    --enable SQUASHFS_XZ
  make -C "${KERNEL_DIR}" ARCH="${KERNEL_ARCH}" olddefconfig

  echo "[+] Building kernel target: ${KERNEL_TARGET} (jobs=${JOBS})"
  make -C "${KERNEL_DIR}" ARCH="${KERNEL_ARCH}" -j"${JOBS}" "${KERNEL_TARGET}"

  if [[ ! -f "${source_path}" ]]; then
    echo "[ERROR] Built kernel image not found at ${source_path}" >&2
    exit 1
  fi

  cp "${source_path}" "${OUTPUT_IMAGE}"

  printf 'kernel_arch=%s\n' "${KERNEL_ARCH}" > "${INFO_FILE}"
  printf 'kernel_target=%s\n' "${KERNEL_TARGET}" >> "${INFO_FILE}"
  printf 'kernel_image=%s\n' "${OUTPUT_IMAGE}" >> "${INFO_FILE}"
  printf 'kernel_repo=%s\n' "${KERNEL_REPO}" >> "${INFO_FILE}"
  printf 'kernel_branch=%s\n' "${KERNEL_BRANCH}" >> "${INFO_FILE}"
  printf 'source_tree=%s\n' "${KERNEL_DIR}" >> "${INFO_FILE}"
  printf 'built_at=%s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" >> "${INFO_FILE}"

  echo "[+] Kernel build completed: ${OUTPUT_IMAGE}"
}

ensure_deps
prepare_tree
build_kernel
