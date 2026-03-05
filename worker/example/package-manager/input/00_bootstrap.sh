#!/usr/bin/env bash
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
    SUDO="sudo"
else
    SUDO=""
fi

if ! command -v apt-get >/dev/null 2>&1; then
    echo "[ERROR] This script currently supports apt-based Ubuntu systems only."
    exit 1
fi

APT_ATTEMPTS="${APT_ATTEMPTS:-5}"
APT_TIMEOUT="${APT_TIMEOUT:-20}"

ensure_hostname_mapping() {
    local host_name
    host_name="$(hostname)"

    if ! grep -Eq "[[:space:]]${host_name}([[:space:]]|\$)" /etc/hosts; then
        echo "[*] Adding hostname mapping for sudo stability: ${host_name}"
        ${SUDO} sh -c "printf '127.0.1.1 %s\n' '${host_name}' >> /etc/hosts"
    fi
}

write_fallback_resolver() {
    echo "[*] Applying fallback DNS resolver settings"
    ${SUDO} sh -c "cat > /etc/resolv.conf <<'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
options timeout:2 attempts:3 rotate
EOF"
}

with_retries() {
    local description="$1"
    shift

    local attempt sleep_sec
    for ((attempt = 1; attempt <= APT_ATTEMPTS; attempt++)); do
        if "$@"; then
            return 0
        fi

        echo "[!] ${description} failed (${attempt}/${APT_ATTEMPTS})"
        write_fallback_resolver || true

        if (( attempt < APT_ATTEMPTS )); then
            sleep_sec=$((attempt * 2))
            echo "[*] Retrying in ${sleep_sec}s"
            sleep "${sleep_sec}"
        fi
    done

    return 1
}

apt_update() {
    ${SUDO} apt-get -qq update \
        -o Acquire::Retries=3 \
        -o Acquire::http::Timeout="${APT_TIMEOUT}"
}

apt_install_deps() {
    ${SUDO} env DEBIAN_FRONTEND=noninteractive apt-get -qq install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cryptsetup-bin \
        curl \
        jq \
        pkg-config \
        runc \
        squashfs-tools
}

apt_has_candidate() {
    local pkg candidate
    for pkg in runc cryptsetup-bin curl jq squashfs-tools build-essential pkg-config; do
        candidate="$(apt-cache policy "${pkg}" | awk '/Candidate:/ {print $2; exit}')"
        if [[ -z "${candidate}" || "${candidate}" == "(none)" ]]; then
            echo "[!] Package '${pkg}' has no candidate in apt cache"
            return 1
        fi
    done
}

apt_update_and_install() {
    apt_update
    apt_has_candidate
    apt_install_deps
}

echo "[*] Updating package metadata"
ensure_hostname_mapping
with_retries "apt-get update" apt_update

echo "[*] Installing system dependencies"
with_retries "apt-get update/install dependencies" apt_update_and_install

echo "[*] Installing Rust via rustup"
if ! command -v rustup >/dev/null 2>&1; then
    with_retries "rustup install" bash -c \
        'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable'
fi

# Source cargo env for subsequent scripts
if [[ -f "${HOME}/.cargo/env" ]]; then
    # shellcheck source=/dev/null
    . "${HOME}/.cargo/env"
fi

rustc --version
cargo --version

echo "[+] Bootstrap complete"
