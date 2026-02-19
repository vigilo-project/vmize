#!/usr/bin/env bash
set -euo pipefail

task="split-task2"
secret=73
low=1
high=100
attempt=0

while (( low <= high )); do
  attempt=$((attempt + 1))
  guess=$(((low + high) / 2))
  echo "[$task] attempt $attempt: guess=$guess range=[$low,$high]"

  if (( guess == secret )); then
    echo "[$task] solved: secret=$secret in $attempt attempts"
    break
  elif (( guess < secret )); then
    low=$((guess + 1))
    echo "[$task] hint: too low"
  else
    high=$((guess - 1))
    echo "[$task] hint: too high"
  fi

  sleep 1
done

printf 'secret=%d\nattempts=%d\n' "$secret" "$attempt" > /tmp/vmize-worker/out/${task}.txt
echo "[$task] saved result to /tmp/vmize-worker/out/${task}.txt"
