#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t agentic-install-tests.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

pass() { printf 'ok - %s\n' "$*"; }
fail() { printf 'not ok - %s\n' "$*" >&2; exit 1; }

run_installer() {
  ( cd "$ROOT" && "$@" )
}

test_local_package_alias() {
  touch "$TMP/local.deb"
  run_installer scripts/install.sh --local-deb "$TMP/local.deb" --dry-run >/dev/null
  pass "local deb alias dry-runs"
}

test_invalid_package_format() {
  if run_installer scripts/install.sh --package zip --dry-run >/dev/null 2>&1; then
    fail "invalid package format unexpectedly succeeded"
  fi
  pass "invalid package format fails"
}

write_fake_curl() {
  local checksum_mode="$1"
  mkdir -p "$TMP/bin"
  cat > "$TMP/bin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
out=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      out="$2"
      shift 2
      ;;
    -H)
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

if [ -z "$out" ]; then
    printf '[{"tag_name":"v2026.6.1","draft":false,"prerelease":false}]\n'
    exit 0
fi

case "$url" in
  */agentic-sandbox_2026.6.1-1_amd64.deb)
    printf 'package-payload' > "$out"
    ;;
  */SHA256SUMS-linux-packages)
    if [ "${FAKE_CHECKSUM_MODE:-ok}" = "bad" ]; then
      printf '0000000000000000000000000000000000000000000000000000000000000000  agentic-sandbox_2026.6.1-1_amd64.deb\n' > "$out"
    else
      printf 'c5a548f061a9d4002377096d4cb0143b6660b2ae678aebb71f1793f5d927a23e  agentic-sandbox_2026.6.1-1_amd64.deb\n' > "$out"
    fi
    ;;
  *)
    printf 'unexpected fake curl URL: %s\n' "$url" >&2
    exit 22
    ;;
esac
EOF
  chmod +x "$TMP/bin/curl"
  export PATH="$TMP/bin:$PATH"
  export FAKE_CHECKSUM_MODE="$checksum_mode"
}

test_latest_resolution_and_checksum() {
  write_fake_curl ok
  AGENTIC_RELEASE_BASE="https://example.invalid/agentic-sandbox" \
    AGENTIC_RELEASE_API="https://api.example.invalid/repos/agentic-sandbox" \
    run_installer scripts/install.sh --dry-run >/dev/null
  pass "latest release resolution verifies checksum"
}

test_checksum_failure() {
  write_fake_curl bad
  if AGENTIC_RELEASE_BASE="https://example.invalid/agentic-sandbox" \
    AGENTIC_RELEASE_API="https://api.example.invalid/repos/agentic-sandbox" \
    run_installer scripts/install.sh --dry-run >/dev/null 2>&1; then
    fail "checksum mismatch unexpectedly succeeded"
  fi
  pass "checksum mismatch fails closed"
}

test_idempotent_same_version_skips_install() {
  local bin="$TMP/idempotent-bin"
  mkdir -p "$bin"
  touch "$TMP/local.deb"
  cat > "$bin/dpkg-deb" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = "-f" ]; then
  printf '2026.6.1-1'
  exit 0
fi
exec /usr/bin/dpkg-deb "$@"
EOF
  cat > "$bin/dpkg-query" <<'EOF'
#!/usr/bin/env bash
printf '2026.6.1-1'
EOF
  cat > "$bin/apt-get" <<'EOF'
#!/usr/bin/env bash
printf 'apt-get should not run for same-version reinstall\n' >&2
exit 99
EOF
  cat > "$bin/sandboxctl" <<'EOF'
#!/usr/bin/env bash
printf '2026.6.1\n'
EOF
  ln -s sandboxctl "$bin/agentic-sandbox"
  cat > "$bin/agentic-mgmt" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
  cat > "$bin/agent-client" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
  chmod +x "$bin/"*

  PATH="$bin:$PATH" run_installer scripts/install.sh --local-deb "$TMP/local.deb" >/dev/null
  pass "same-version install is idempotent"
}

test_local_package_alias
test_invalid_package_format
test_latest_resolution_and_checksum
test_checksum_failure
test_idempotent_same_version_skips_install
