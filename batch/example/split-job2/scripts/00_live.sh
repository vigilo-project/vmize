#!/usr/bin/env bash
set -euo pipefail

job="split-job2"
secret=73
low=1
high=100
attempt=0

while (( low <= high )); do
  attempt=$((attempt + 1))
  guess=$(((low + high) / 2))
  echo "[$job] attempt $attempt: guess=$guess range=[$low,$high]"

  if (( guess == secret )); then
    echo "[$job] solved: secret=$secret in $attempt attempts"
    break
  elif (( guess < secret )); then
    low=$((guess + 1))
    echo "[$job] hint: too low"
  else
    high=$((guess - 1))
    echo "[$job] hint: too high"
  fi

  sleep 1
done

printf 'secret=%d\nattempts=%d\n' "$secret" "$attempt" > /tmp/batch/out/${job}.txt
echo "[$job] saved result to /tmp/batch/out/${job}.txt"
