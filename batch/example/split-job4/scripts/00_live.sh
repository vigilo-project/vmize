#!/usr/bin/env bash
set -euo pipefail

job="split-job4"

echo "[$job] starting background runner"
(
  trap 'exit 0' TERM INT
  tick=0
  while true; do
    tick=$((tick + 1))
    obstacle=$((RANDOM % 5))
    lane=$((RANDOM % 3 + 1))
    echo "[$job][runner] tick=$tick lane=$lane obstacle=$obstacle"
    sleep 1
  done
) &
runner_pid=$!

echo "[$job] runner pid=$runner_pid"
for sec in 1 2 3 4 5 6 7; do
  echo "[$job] monitor second=$sec"
  sleep 1
done

echo "[$job] stopping runner pid=$runner_pid"
kill "$runner_pid"
wait "$runner_pid" 2>/dev/null || true

echo "runner_stopped=1" > /tmp/batch/out/${job}.txt
echo "[$job] saved result to /tmp/batch/out/${job}.txt"
