#!/usr/bin/env bash
# Focused browser gate for the build-free management dashboard self-test pages.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BROWSER_BIN="${MANAGEMENT_UI_BROWSER_BIN:-}"
if [[ -z "$BROWSER_BIN" ]]; then
    for candidate in chromium chromium-browser google-chrome google-chrome-stable; do
        if command -v "$candidate" >/dev/null 2>&1; then
            BROWSER_BIN="$(command -v "$candidate")"
            break
        fi
    done
fi
if [[ -z "$BROWSER_BIN" || ! -x "$BROWSER_BIN" ]]; then
    echo "error: set MANAGEMENT_UI_BROWSER_BIN to a Chromium-compatible executable" >&2
    exit 2
fi

browser_args=(
    --headless
    --disable-gpu
    --no-first-run
    --no-default-browser-check
)
if [[ "${MANAGEMENT_UI_BROWSER_DISABLE_SANDBOX:-0}" == "1" ]]; then
    # CI runners may disable unprivileged user namespaces. The disposable
    # browser only loads the loopback test server created below.
    browser_args+=(--no-sandbox)
fi

SMOKE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/management-ui-browser-smoke.XXXXXX")"
SERVER_PID=""
cleanup() {
    if [[ -n "$SERVER_PID" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi
    rm -rf -- "$SMOKE_TMP"
}
trap cleanup EXIT INT TERM

PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    print(sock.getsockname()[1])
PY
)"
python3 -m http.server "$PORT" --bind 127.0.0.1 \
    --directory "$PROJECT_ROOT/management/ui" \
    >"$SMOKE_TMP/server.log" 2>&1 &
SERVER_PID="$!"

for attempt in {1..50}; do
    if python3 - "$PORT" <<'PY' >/dev/null 2>&1
import socket, sys
with socket.create_connection(('127.0.0.1', int(sys.argv[1])), timeout=.2):
    pass
PY
    then
        break
    fi
    if (( attempt == 50 )); then
        echo "error: management UI test server did not start" >&2
        exit 1
    fi
    sleep 0.1
done

pages=(
    test/contract-boundary.test.html
    test/api-client.test.html
    test/tui-redraw-stress.test.html
)
for page in "${pages[@]}"; do
    output="$SMOKE_TMP/$(basename "$page").dom.html"
    profile="$SMOKE_TMP/profile-$(basename "$page")"
    "$BROWSER_BIN" \
        "${browser_args[@]}" \
        --user-data-dir="$profile" \
        --virtual-time-budget=20000 \
        --dump-dom "http://127.0.0.1:${PORT}/${page}" >"$output"
    if ! grep -q 'data-result="pass"' "$output"; then
        failures="$(grep -o 'data-failures="[0-9]*"' "$output" | head -1 || true)"
        echo "FAIL: $page ${failures:-did not complete}" >&2
        exit 1
    fi
    echo "PASS: $page"
done
