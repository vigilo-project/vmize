#!/usr/bin/env bash
set -euo pipefail

cd /mnt/vigilo

# Official Vigilo dependency bootstrap before kernel build.
./scripts/deps/pkgs.sh

# 9p mount ownership differs from VM user; root operations require safe.directory.
sudo git config --global --add safe.directory /mnt/vigilo
