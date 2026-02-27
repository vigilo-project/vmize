#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="setup_custom_kernel_boot.sh"
WORKSPACE_DIR="$HOME/.local/share/vm/images/custom"
KERNEL_REPO="https://github.com/torvalds/linux.git"
KERNEL_BRANCH="master"
KERNEL_FORCE_REBUILD="false"
SKIP_KERNEL_BUILD="false"
SKIP_ROOTFS_BUILD="false"
ROOTFS_SOURCE=""
ROOTFS_SIZE="20G"
ROOTFS_FORMAT="raw"
if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi
JOBS="$(
    if command -v nproc >/dev/null 2>&1; then
        nproc
    elif command -v sysctl >/dev/null 2>&1; then
        sysctl -n hw.ncpu 2>/dev/null || echo 4
    else
        echo 4
    fi
)"

usage() {
    cat <<USAGE
Usage: ${SCRIPT_NAME} [options]

Create a custom kernel/rootfs workspace for vm --kernel / --rootfs.

Options:
  --workspace <path>          Workspace path (default: ~/.local/share/vm/images/custom)
  --kernel-repo <url>         Linux git repo URL (default: https://github.com/torvalds/linux.git)
  --kernel-branch <name>      Kernel branch to build (default: master)
  --skip-kernel-build         Skip kernel clone/build step
  --force-kernel-rebuild      Re-run kernel build from scratch
  --rootfs-source <path|url>  Rootfs tar source (default: Ubuntu cloud minimal rootfs tarball for host arch)
  --rootfs-size <size>        Size for generated rootfs image (default: 20G)
  --rootfs-format <raw|qcow2> Output rootfs image format (default: raw)
  --skip-rootfs-build          Skip rootfs image generation (assumes --rootfs-source is already a disk image)
  --jobs <n>                  Parallel build jobs for Linux (default: detected cores)
  --help                      Show this help
USAGE
}

require_cmd() {
    local cmd="$1"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "[ERROR] required command not found: ${cmd}" >&2
        exit 1
    fi
}

ensure_cmds() {
    require_cmd git
    require_cmd tar
    require_cmd make
    require_cmd sed
    require_cmd awk

    if [[ "${SKIP_KERNEL_BUILD}" == "false" ]]; then
        require_cmd gcc
    fi

    if [[ "${SKIP_ROOTFS_BUILD}" == "false" ]]; then
        require_cmd mkfs.ext4
    fi

    if [[ "${ROOTFS_SOURCE}" == http* ]] || [[ "${SKIP_ROOTFS_BUILD}" == "false" ]]; then
        if ! command -v wget >/dev/null 2>&1 && ! command -v curl >/dev/null 2>&1; then
            echo "[ERROR] either wget or curl is required" >&2
            exit 1
        fi
    fi

    if [[ "${ROOTFS_FORMAT}" == "qcow2" ]]; then
        require_cmd qemu-img
    fi
}

download_with_fallback() {
    local url="$1"
    local out="$2"

    if command -v wget >/dev/null 2>&1; then
        wget -O "$out" "$url"
        return
    fi

    curl -L --fail --output "$out" "$url"
}

extract_tar_archive() {
    local archive="$1"
    local output_dir="$2"

    echo "[*] Extracting ${archive} -> ${output_dir}"
    "${SUDO}" rm -rf "${output_dir}"
    mkdir -p "${output_dir}"
    "${SUDO}" tar --exclude='dev/*' --exclude='./dev/*' \
        --acls --xattrs --numeric-owner \
        -xaf "${archive}" -C "${output_dir}"
}

build_kernel() {
    if [[ "${SKIP_KERNEL_BUILD}" == "true" ]]; then
        echo "[+] Skipping kernel build"
        return
    fi

    local kernel_dir="${WORKSPACE_DIR}/linux"
    local kernel_output="${kernel_dir}/${KERNEL_OUTPUT}"

    if [[ -d "${kernel_dir}" ]]; then
        echo "[+] Updating Linux source in ${kernel_dir}"
        git -C "${kernel_dir}" fetch --depth=1 origin "${KERNEL_BRANCH}"
        git -C "${kernel_dir}" checkout "origin/${KERNEL_BRANCH}"
    else
        echo "[+] Cloning Linux source"
        git clone --depth=1 --branch "${KERNEL_BRANCH}" "${KERNEL_REPO}" "${kernel_dir}"
    fi

    if [[ "${KERNEL_FORCE_REBUILD}" == "true" ]]; then
        make -C "${kernel_dir}" clean
    fi

    echo "[+] Configuring kernel (${KERNEL_ARCH}, ${KERNEL_DEFCONFIG})"
    make -C "${kernel_dir}" ARCH="${KERNEL_ARCH}" "${KERNEL_DEFCONFIG}"

    echo "[+] Building kernel image: ${KERNEL_TARGET}"
    make -C "${kernel_dir}" ARCH="${KERNEL_ARCH}" -j "${JOBS}" "${KERNEL_TARGET}"

    if [[ ! -f "${kernel_output}" ]]; then
        echo "[ERROR] kernel image not found: ${kernel_output}" >&2
        exit 1
    fi

    echo "[+] Kernel ready: ${kernel_output}"
}

build_rootfs_image() {
    if [[ "${SKIP_ROOTFS_BUILD}" == "true" ]]; then
        if [[ ! -f "${ROOTFS_SOURCE}" && ! -f "${WORKSPACE_DIR}/${ROOTFS_SOURCE}" ]]; then
            echo "[ERROR] --rootfs-source must point to an existing disk image when --skip-rootfs-build is used" >&2
            exit 1
        fi

        if [[ -f "${ROOTFS_SOURCE}" ]]; then
            ROOTFS_OUTPUT="${ROOTFS_SOURCE}"
        else
            ROOTFS_OUTPUT="${WORKSPACE_DIR}/${ROOTFS_SOURCE}"
        fi
        echo "[+] Using prebuilt rootfs image: ${ROOTFS_OUTPUT}"
        return
    fi

    local rootfs_tar=""
    local temp_rootfs_dir="${WORKSPACE_DIR}/rootfs-tree"
    local prepared_source=""
    local workspace_tmp="${WORKSPACE_DIR}/tmp"
    mkdir -p "${workspace_tmp}"

    if [[ "${ROOTFS_SOURCE}" == http* ]]; then
        rootfs_tar="${workspace_tmp}/ubuntu-minimal-rootfs.tar.xz"
        echo "[*] Downloading rootfs source from ${ROOTFS_SOURCE}"
        download_with_fallback "${ROOTFS_SOURCE}" "${rootfs_tar}"
    elif [[ -f "${ROOTFS_SOURCE}" ]]; then
        rootfs_tar="${ROOTFS_SOURCE}"
    else
        echo "[ERROR] Rootfs source does not exist: ${ROOTFS_SOURCE}" >&2
        exit 1
    fi

    prepared_source="${rootfs_tar}"

    case "${prepared_source##*.}" in
        tar|xz)
            ;;
        *)
            if [[ "${prepared_source}" == *.tar.xz || "${prepared_source}" == *.tar.gz || "${prepared_source}" == *.tgz ]]; then
                :
            else
                echo "[ERROR] Rootfs source is not a supported tar archive: ${prepared_source}" >&2
                exit 1
            fi
            ;;
    esac

    echo "[*] Expanding rootfs tar archive"
    extract_tar_archive "${prepared_source}" "${temp_rootfs_dir}"

    local raw_image="${WORKSPACE_DIR}/rootfs.raw"
    local qcow_image="${WORKSPACE_DIR}/rootfs.qcow2"

    if command -v qemu-img >/dev/null 2>&1; then
        qemu-img create -f raw "${raw_image}" "${ROOTFS_SIZE}"
    else
        truncate -s "${ROOTFS_SIZE}" "${raw_image}"
    fi

    echo "[*] Creating ext4 filesystem and populating rootfs"
    "${SUDO}" mkfs.ext4 -F -d "${temp_rootfs_dir}" "${raw_image}"

    if [[ "${ROOTFS_FORMAT}" == "qcow2" ]]; then
        echo "[*] Converting raw rootfs to qcow2"
        qemu-img convert -f raw -O qcow2 "${raw_image}" "${qcow_image}"
        ROOTFS_OUTPUT="${qcow_image}"
        rm -f "${raw_image}"
    else
        ROOTFS_OUTPUT="${raw_image}"
    fi

    echo "[+] Rootfs image ready: ${ROOTFS_OUTPUT}"
}

run_plan() {
    echo
    echo "Custom kernel boot workspace is ready"
    echo "Workspace: ${WORKSPACE_DIR}"
    echo "Kernel: ${WORKSPACE_DIR}/linux/${KERNEL_OUTPUT}"
    echo "Rootfs: ${ROOTFS_OUTPUT}"
    echo
    echo "Run with:"
    echo "  (cd \"$(pwd)\" && cargo run -p vm -- run --kernel \"${WORKSPACE_DIR}/linux/${KERNEL_OUTPUT}\" --rootfs \"${ROOTFS_OUTPUT}\" --verbose)"
}

main() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --workspace)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --workspace needs a value" >&2
                    exit 1
                fi
                WORKSPACE_DIR="$2"
                shift 2
                ;;
            --kernel-repo)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --kernel-repo needs a value" >&2
                    exit 1
                fi
                KERNEL_REPO="$2"
                shift 2
                ;;
            --kernel-branch)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --kernel-branch needs a value" >&2
                    exit 1
                fi
                KERNEL_BRANCH="$2"
                shift 2
                ;;
            --skip-kernel-build)
                SKIP_KERNEL_BUILD="true"
                shift
                ;;
            --force-kernel-rebuild)
                KERNEL_FORCE_REBUILD="true"
                shift
                ;;
            --rootfs-source)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --rootfs-source needs a value" >&2
                    exit 1
                fi
                ROOTFS_SOURCE="$2"
                shift 2
                ;;
            --rootfs-size)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --rootfs-size needs a value" >&2
                    exit 1
                fi
                ROOTFS_SIZE="$2"
                shift 2
                ;;
            --rootfs-format)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --rootfs-format needs a value" >&2
                    exit 1
                fi
                ROOTFS_FORMAT="$2"
                if [[ "${ROOTFS_FORMAT}" != "raw" && "${ROOTFS_FORMAT}" != "qcow2" ]]; then
                    echo "[ERROR] --rootfs-format must be raw or qcow2" >&2
                    exit 1
                fi
                shift 2
                ;;
            --skip-rootfs-build)
                SKIP_ROOTFS_BUILD="true"
                shift
                ;;
            --jobs)
                if [[ $# -lt 2 ]]; then
                    echo "[ERROR] --jobs needs a value" >&2
                    exit 1
                fi
                JOBS="$2"
                shift 2
                ;;
            --help)
                usage
                exit 0
                ;;
            *)
                echo "[ERROR] Unknown argument: $1" >&2
                usage
                exit 1
                ;;
        esac
    done

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
                echo "[ERROR] Unsupported host architecture: $(uname -m)" >&2
                exit 1
            ;;
        esac
    fi

    case "${HOST_ARCH}" in
        x86_64)
            KERNEL_ARCH="x86_64"
            KERNEL_DEFCONFIG="x86_64_defconfig"
            KERNEL_TARGET="bzImage"
            KERNEL_OUTPUT="arch/x86/boot/bzImage"
            ;;
        aarch64|arm64)
            KERNEL_ARCH="arm64"
            KERNEL_DEFCONFIG="defconfig"
            KERNEL_TARGET="Image"
            KERNEL_OUTPUT="arch/arm64/boot/Image"
            ;;
        *)
            echo "[ERROR] Unsupported host architecture: $(uname -m)" >&2
            exit 1
            ;;
    esac

    if [[ "${SKIP_KERNEL_BUILD}" == "true" ]]; then
        local kernel_output_path="${WORKSPACE_DIR}/linux/${KERNEL_OUTPUT}"
        if [[ ! -f "${kernel_output_path}" ]]; then
            echo "[ERROR] --skip-kernel-build was set but no kernel image exists at ${kernel_output_path}" >&2
            exit 1
        fi
    fi

    ensure_cmds
    mkdir -p "${WORKSPACE_DIR}"
    build_kernel
    build_rootfs_image
    run_plan
}

main "$@"
