#!/usr/bin/env bash
set -euo pipefail

cd /mnt/vigilo

START_EPOCH="$(date +%s)"
START_ISO="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
echo "[vmize] build_start=${START_ISO}"

sudo make kernel VERBOSE=1

END_EPOCH="$(date +%s)"
END_ISO="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
ELAPSED="$((END_EPOCH - START_EPOCH))"
echo "[vmize] build_end=${END_ISO}"
echo "[vmize] build_elapsed_sec=${ELAPSED}"

mkdir -p /tmp/vmize-worker/out

if [[ -f /mnt/vigilo/out/Image ]]; then
  cp -f /mnt/vigilo/out/Image /tmp/vmize-worker/out/kernel-image
elif [[ -f /mnt/vigilo/out/bzImage ]]; then
  cp -f /mnt/vigilo/out/bzImage /tmp/vmize-worker/out/kernel-image
else
  echo "kernel image not found in /mnt/vigilo/out" >&2
  exit 1
fi

if [[ -f /mnt/vigilo/out/Image.config ]]; then
  cp -f /mnt/vigilo/out/Image.config /tmp/vmize-worker/out/kernel.config
elif [[ -f /mnt/vigilo/out/bzImage.config ]]; then
  cp -f /mnt/vigilo/out/bzImage.config /tmp/vmize-worker/out/kernel.config
else
  echo "kernel config not found in /mnt/vigilo/out" >&2
  exit 1
fi

cat > /tmp/vmize-worker/out/kernel-build-timing.txt <<TIMING
build_start=${START_ISO}
build_end=${END_ISO}
build_elapsed_sec=${ELAPSED}
TIMING
