#!/usr/bin/env bash
set -euo pipefail

if [[ ! -f /tmp/vmize-worker/work/handoff.txt ]]; then
  echo "missing handoff artifact" >&2
  exit 1
fi

tr '[:lower:]' '[:upper:]' < /tmp/vmize-worker/work/handoff.txt > /tmp/vmize-worker/out/final.txt
