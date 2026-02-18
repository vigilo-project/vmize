#!/usr/bin/env bash
set -euo pipefail

echo "result-script" > /tmp/batch/out/result.txt
ls -1 /tmp/batch/work > /tmp/batch/out/scripts.txt
