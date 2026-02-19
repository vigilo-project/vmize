#!/usr/bin/env bash
set -euo pipefail

task="split-task4"

echo "[$task] starting background runner"
(
  trap 'exit 0' TERM INT
  tick=0
  while true; do
    tick=$((tick + 1))
    obstacle=$((RANDOM % 5))
    lane=$((RANDOM % 3 + 1))
    echo "[$task][runner] tick=$tick lane=$lane obstacle=$obstacle"
    sleep 1
  done
) &
runner_pid=$!

echo "[$task] runner pid=$runner_pid"
for sec in 1 2 3 4 5 6 7; do
  echo "[$task] monitor second=$sec"
  sleep 1
done

echo "[$task] stopping runner pid=$runner_pid"
kill "$runner_pid"
wait "$runner_pid" 2>/dev/null || true

echo "runner_stopped=1" > /tmp/batch/out/${task}.txt
echo "[$task] saved result to /tmp/batch/out/${task}.txt"
