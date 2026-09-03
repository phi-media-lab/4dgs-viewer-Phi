#!/usr/bin/env bash
set -euo pipefail

PLAYER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$PLAYER_DIR/.." && pwd)"
cd "$PLAYER_DIR"

width="${PHI_WIDTH:-1280}"
height="${PHI_HEIGHT:-720}"
# 720p30 is the conservative reference profile. Higher cadence remains an
# explicit operator choice because encoder and receiver budgets are hardware-
# and network-dependent.
fps="${PHI_FPS:-30}"
port="${PHI_PORT:-4191}"
manifest="${PHI_MANIFEST:-$REPO_DIR/examples/synthetic-motion-sh3/manifest.json}"

cargo build --release --locked

renderer_args=(
  --serve --width "$width" --height "$height" --fps "$fps"
  --slots 3 --port "$port"
  --bind 127.0.0.1
  --manifest "$manifest"
  --shaders "$PLAYER_DIR/shaders"
)

if [[ ${PHI_INTERACTION_ALPHA_MIN+x} == x ]]; then
  renderer_args+=(--interaction-alpha-min "$PHI_INTERACTION_ALPHA_MIN")
fi
renderer_args+=("$@")

consecutive_fast_failures=0
while true; do
  renderer_started_at="$SECONDS"
  set +e
  target/release/phi-4dgs-player "${renderer_args[@]}"
  renderer_status="$?"
  set -e
  renderer_runtime_seconds=$((SECONDS - renderer_started_at))

  case "$renderer_status" in
    0|130|143)
      exit "$renderer_status"
      ;;
    75)
      consecutive_fast_failures=0
      printf '[run] restarting WebRTC session after browser reload or media recovery\n'
      sleep 0.2
      ;;
    *)
      # Runtime GPU/media errors must not leave the preview permanently dead.
      # Three rapid failures still fail closed so invalid configuration does
      # not become an infinite restart loop. A process that ran for ten seconds
      # starts a fresh retry budget.
      if ((renderer_runtime_seconds >= 10)); then
        consecutive_fast_failures=0
      fi
      consecutive_fast_failures=$((consecutive_fast_failures + 1))
      if ((consecutive_fast_failures >= 3)); then
        printf '[run] renderer failed %d times without stabilizing; exiting with %d\n' \
          "$consecutive_fast_failures" "$renderer_status" >&2
        exit "$renderer_status"
      fi
      retry_delay_seconds=$consecutive_fast_failures
      printf '[run] renderer exited with %d after %ds; retrying in %ds (%d/3)\n' \
        "$renderer_status" "$renderer_runtime_seconds" "$retry_delay_seconds" \
        "$consecutive_fast_failures" >&2
      sleep "$retry_delay_seconds"
      ;;
  esac
done
