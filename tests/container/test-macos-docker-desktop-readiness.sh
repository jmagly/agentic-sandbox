#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CHECK="$ROOT/scripts/check-macos-docker-desktop.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

FAKE_DOCKER="$TMP/docker"
cat >"$FAKE_DOCKER" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  context)
    [[ "${2:-}" == "show" ]]
    printf '%s\n' "${FAKE_CONTEXT:-desktop-linux}"
    ;;
  info)
    [[ "${FAKE_DAEMON_REACHABLE:-true}" == "true" ]] || exit 1
    printf '%s %s\n' "${FAKE_DOCKER_OS:-linux}" "${FAKE_DOCKER_ARCH:-aarch64}"
    ;;
  *)
    exit 2
    ;;
esac
FAKE
chmod +x "$FAKE_DOCKER"

assert_diagnostic() {
  local expected_rc="$1"
  local expected_output="$2"
  shift 2
  local output rc=0
  output="$("$@" 2>&1)" || rc=$?
  [[ "$rc" -eq "$expected_rc" ]] || {
    echo "FAIL: expected rc=$expected_rc, got rc=$rc: $output" >&2
    exit 1
  }
  [[ "$output" == "$expected_output" ]] || {
    echo "FAIL: expected '$expected_output', got '$output'" >&2
    exit 1
  }
}

assert_diagnostic 20 \
  "status=not-ready diagnostic=cli-missing" \
  env AGENTIC_DOCKER_BIN="$TMP/missing-docker" "$CHECK"

assert_diagnostic 21 \
  "status=not-ready diagnostic=context-inactive" \
  env AGENTIC_DOCKER_BIN="$FAKE_DOCKER" FAKE_CONTEXT=remote-builder "$CHECK"

assert_diagnostic 22 \
  "status=not-ready diagnostic=daemon-unreachable" \
  env AGENTIC_DOCKER_BIN="$FAKE_DOCKER" FAKE_DAEMON_REACHABLE=false "$CHECK"

assert_diagnostic 21 \
  "status=not-ready diagnostic=context-inactive" \
  env AGENTIC_DOCKER_BIN="$FAKE_DOCKER" FAKE_DOCKER_ARCH=x86_64 "$CHECK"

assert_diagnostic 0 \
  "status=ready diagnostic=none docker_os=linux docker_arch=aarch64" \
  env AGENTIC_DOCKER_BIN="$FAKE_DOCKER" "$CHECK"

echo "macOS Docker Desktop readiness diagnostic tests passed"
