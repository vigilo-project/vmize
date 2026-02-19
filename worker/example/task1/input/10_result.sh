#!/usr/bin/env bash
set -euo pipefail

echo "result-script" > /tmp/vmize-worker/out/result.txt
ls -1 /tmp/vmize-worker/work > /tmp/vmize-worker/out/scripts.txt
