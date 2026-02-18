#!/usr/bin/env bash
set -euo pipefail

OS="$(uname -s)"
ARCH="$(uname -m)"

IMAGE_DIR="./vms/images"
IMAGE_URL=""
IMAGE_PATH=""

install_linux_deps() {
    if [ "${EUID}" -ne 0 ]; then
        echo "Linux x86_64 setup requires sudo/root. Run: sudo ./deps.sh"
        exit 1
    fi

    apt-get update
    apt-get install -y \
        qemu-kvm \
        libvirt-daemon-system \
        libvirt-clients \
        bridge-utils \
        virtinst \
        cpu-checker \
        genisoimage \
        openssh-client \
        wget
}

install_macos_deps() {
    if [ "${EUID}" -eq 0 ]; then
        echo "macOS arm64 setup must run without sudo. Run: ./deps.sh"
        exit 1
    fi

    if ! command -v brew >/dev/null 2>&1; then
        echo "Homebrew is required. Install it first: https://brew.sh"
        exit 1
    fi

    brew update
    brew install qemu xorriso wget openssh
}

configure_platform() {
    case "${OS}:${ARCH}" in
        Linux:x86_64)
            IMAGE_URL="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-amd64.img"
            ;;
        Darwin:arm64)
            IMAGE_URL="https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-arm64.img"
            ;;
        *)
            echo "Unsupported platform: ${OS}/${ARCH}"
            echo "Supported platforms: Linux/x86_64, Darwin/arm64"
            exit 1
            ;;
    esac

    IMAGE_PATH="${IMAGE_DIR}/$(basename "${IMAGE_URL}")"
}

download_image() {
    mkdir -p "${IMAGE_DIR}"

    if [ -f "${IMAGE_PATH}" ]; then
        echo "Image already exists at ${IMAGE_PATH}"
        return
    fi

    wget -O "${IMAGE_PATH}" "${IMAGE_URL}"
    echo "Image downloaded to ${IMAGE_PATH}"
}

verify_image() {
    if [ ! -f "${IMAGE_PATH}" ]; then
        echo "Image download failed: ${IMAGE_PATH}"
        exit 1
    fi

    SIZE="$(du -h "${IMAGE_PATH}" | cut -f1)"
    echo "Image size: ${SIZE}"
}

main() {
    echo "Installing dependencies for ${OS}/${ARCH}..."
    configure_platform

    case "${OS}:${ARCH}" in
        Linux:x86_64)
            install_linux_deps
            ;;
        Darwin:arm64)
            install_macos_deps
            ;;
    esac

    echo "System dependencies installed."
    echo "Downloading Ubuntu Cloud 24.04 minimal image..."
    download_image
    verify_image

    echo "Setup complete."
    echo "You can now run: cargo test"
}

main "$@"
