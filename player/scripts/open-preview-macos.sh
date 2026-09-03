#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: open-preview-macos.sh [http(s)://host:port/]" >&2
}

(( $# <= 1 )) || {
  usage
  exit 2
}
[[ "$(uname -s)" == Darwin ]] || {
  echo "open-preview-macos.sh requires macOS" >&2
  exit 2
}

url="${1:-http://127.0.0.1:4192/?jitter_buffer_ms=browser}"
python3 - "$url" <<'PY'
import sys
from urllib.parse import urlsplit

try:
    parsed = urlsplit(sys.argv[1])
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ValueError
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    if not 1 <= port <= 65535:
        raise ValueError
except ValueError:
    raise SystemExit("expected a valid HTTP(S) preview URL")
PY

exec /usr/bin/open -a "Google Chrome" "$url"
