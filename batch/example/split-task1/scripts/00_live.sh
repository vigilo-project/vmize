#!/usr/bin/env bash
set -euo pipefail

task="split-task1"
declare -a board=(1 2 3 4 5 6 7 8 9)

declare -a lines=(
  "0 1 2" "3 4 5" "6 7 8"
  "0 3 6" "1 4 7" "2 5 8"
  "0 4 8" "2 4 6"
)

print_board() {
  echo "[$task] board"
  echo "[$task] ${board[0]} | ${board[1]} | ${board[2]}"
  echo "[$task] ${board[3]} | ${board[4]} | ${board[5]}"
  echo "[$task] ${board[6]} | ${board[7]} | ${board[8]}"
}

is_winner() {
  local mark="$1"
  local trio
  for trio in "${lines[@]}"; do
    read -r a b c <<<"$trio"
    if [[ "${board[$a]}" == "$mark" && "${board[$b]}" == "$mark" && "${board[$c]}" == "$mark" ]]; then
      return 0
    fi
  done
  return 1
}

empty_slots() {
  local i
  for i in "${!board[@]}"; do
    if [[ "${board[$i]}" != "X" && "${board[$i]}" != "O" ]]; then
      echo "$i"
    fi
  done
}

turn=0
winner="draw"
while [[ $turn -lt 9 ]]; do
  if (( turn % 2 == 0 )); then
    mark="X"
    player="alpha"
  else
    mark="O"
    player="beta"
  fi

  mapfile -t slots < <(empty_slots)
  pick_index=$((RANDOM % ${#slots[@]}))
  pos="${slots[$pick_index]}"
  board[$pos]="$mark"

  echo "[$task] turn $((turn + 1)): $player plays $mark at cell $((pos + 1))"
  print_board

  if is_winner "$mark"; then
    winner="$player($mark)"
    echo "[$task] winner decided: $winner"
    break
  fi

  turn=$((turn + 1))
  sleep 1
done

echo "winner=$winner" > /tmp/batch/out/${task}.txt
echo "[$task] saved result to /tmp/batch/out/${task}.txt"
