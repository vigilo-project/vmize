#!/usr/bin/env bash
set -euo pipefail

WORK_DIR="/tmp/vmize-worker/work"
SRC_TAR="${WORK_DIR}/package-manager-src.tar"
SRC_DIR="${WORK_DIR}/package-manager-src"

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

# Source cargo env
if [[ -f "${HOME}/.cargo/env" ]]; then
    # shellcheck source=/dev/null
    . "${HOME}/.cargo/env"
fi

if [[ ! -f "${SRC_TAR}" ]]; then
    echo "[ERROR] Missing source tarball: ${SRC_TAR}"
    echo "[*] Create it with:"
    echo "    tar -C /path/to/package-manager \\"
    echo "        --exclude=target --exclude=.git --exclude=node_modules \\"
    echo "        -cf worker/example/package-manager/input/package-manager-src.tar \\"
    echo "        Cargo.toml Cargo.lock format builder loader"
    exit 1
fi

echo "[*] Extracting package-manager source"
mkdir -p "${SRC_DIR}"
tar xf "${SRC_TAR}" -C "${SRC_DIR}"

echo "[*] Building loader (release)"
cd "${SRC_DIR}"
cargo build --release -p loader

echo "[*] Installing loader binary"
${SUDO} cp -f target/release/loader /usr/local/bin/loader
${SUDO} chmod +x /usr/local/bin/loader

loader --help

echo "[+] Loader build complete"
