#!/usr/bin/env bash
set -euo pipefail

player_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$player_dir"

child_pid=""

input_hash() {
  {
    find src web shaders scripts -type f -printf '%T@ %p\n'
    stat -c '%Y %n' Cargo.toml Cargo.lock rust-toolchain.toml
  } | sort | sha256sum | awk '{ print $1 }'
}

stop_renderer() {
  [[ -n "$child_pid" ]] || return 0
  if kill -0 "$child_pid" 2>/dev/null; then
    kill -INT -- "-$child_pid" 2>/dev/null || true
    for _ in {1..20}; do
      kill -0 "$child_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -TERM -- "-$child_pid" 2>/dev/null || true
  fi
  wait "$child_pid" 2>/dev/null || true
  child_pid=""
}

start_renderer() {
  printf '\n[watch] build/restart remote renderer\n'
  setsid ./scripts/run.sh &
  child_pid="$!"
}

cleanup() {
  stop_renderer
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

previous="$(input_hash)"
start_renderer
while true; do
  sleep 0.5
  current="$(input_hash)"
  if [[ "$current" != "$previous" ]]; then
    sleep 0.2
    previous="$(input_hash)"
    stop_renderer
    start_renderer
  fi
done
