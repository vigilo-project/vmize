#!/usr/bin/env bash
set -euo pipefail

task="split-task3"
choices=(rock paper scissors)
a_score=0
b_score=0

for round in 1 2 3 4 5 6 7 8 9 10; do
  a_pick="${choices[$((RANDOM % 3))]}"
  b_pick="${choices[$((RANDOM % 3))]}"

  if [[ "$a_pick" == "$b_pick" ]]; then
    result="draw"
  elif [[ "$a_pick" == "rock" && "$b_pick" == "scissors" ]] ||
       [[ "$a_pick" == "paper" && "$b_pick" == "rock" ]] ||
       [[ "$a_pick" == "scissors" && "$b_pick" == "paper" ]]; then
    result="alpha"
    a_score=$((a_score + 1))
  else
    result="beta"
    b_score=$((b_score + 1))
  fi

  echo "[$task] round $round: alpha=$a_pick beta=$b_pick => $result"
  echo "[$task] score: alpha=$a_score beta=$b_score"
  sleep 1
done

if (( a_score > b_score )); then
  winner="alpha"
elif (( b_score > a_score )); then
  winner="beta"
else
  winner="draw"
fi

printf 'alpha=%d\nbeta=%d\nwinner=%s\n' "$a_score" "$b_score" "$winner" > /tmp/batch/out/${task}.txt
echo "[$task] saved result to /tmp/batch/out/${task}.txt"
