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

    if ! grep -Eq "[[:space:]]${host_name}([[:space:]]|$)" /etc/hosts; then
        ${SUDO} sh -c "printf '127.0.1.1 %s\n' '${host_name}' >> /etc/hosts"
    fi
}

write_fallback_resolver() {
    ${SUDO} sh -c "cat > /etc/resolv.conf <<'RESOLVE'
nameserver 1.1.1.1
nameserver 8.8.8.8
options timeout:2 attempts:3 rotate
RESOLVE"
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
        attr \
        cryptsetup-bin \
        curl \
        ima-evm-utils \
        jq \
        keyutils \
        runc \
        squashfs-tools \
        util-linux
}

apt_has_candidate() {
    local pkg candidate
    for pkg in attr cryptsetup-bin curl ima-evm-utils jq keyutils runc squashfs-tools util-linux; do
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

ensure_hostname_mapping
with_retries "apt-get update" apt_update
with_retries "apt-get update/install dependencies" apt_update_and_install

echo "[+] runc-llama-ima-verify-run bootstrap complete"
