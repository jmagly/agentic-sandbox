#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t agentic-macos-activity.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fail() { printf 'not ok - %s\n' "$*" >&2; exit 1; }
pass() { printf 'ok - %s\n' "$*"; }

python3 -m py_compile "$ROOT/scripts/macos-unified-log-adapter.py"
printf '%s\n' \
  '{"subsystem":"io.aiwg.agentic-sandbox","category":"collector","messageType":"Notice","processID":42,"eventMessage":"token=do-not-retain"}' \
  '{"subsystem":"untrusted.example","category":"collector","eventMessage":"ignore-me"}' |
  "$ROOT/scripts/macos-unified-log-adapter.py" > "$TMP/unified.jsonl"

test "$(wc -l < "$TMP/unified.jsonl" | tr -d ' ')" = 1 \
  || fail "Unified Logging allowlist did not reject an unknown subsystem"
grep -F '"message_digest":"sha256:' "$TMP/unified.jsonl" >/dev/null \
  || fail "Unified Logging content was not represented by a digest"
if grep -F 'do-not-retain' "$TMP/unified.jsonl" >/dev/null; then
  fail "Unified Logging adapter retained message content"
fi
pass "Unified Logging adapter is metadata-only and allowlisted"

grep -F '<key>com.apple.developer.endpoint-security.client</key>' \
  "$ROOT/native/macos-activity-collector/EndpointSecurity.entitlements.plist" >/dev/null \
  || fail "Endpoint Security entitlement declaration is missing"
grep -F '/usr/local/bin/agentic-macos-activity-collector' \
  "$ROOT/deploy/launchd/io.aiwg.agentic-sandbox.activity-collector.plist" >/dev/null \
  || fail "LaunchDaemon template does not use the installed collector path"
test ! -e "$ROOT/deploy/packaging/macos/scripts/postinstall" \
  || fail "macOS package must not silently activate Endpoint Security"
pass "Endpoint Security activation remains explicit"

grep -F 'ES_EVENT_TYPE_NOTIFY_EXEC' "$ROOT/native/macos-activity-collector/main.swift" >/dev/null \
  || fail "exec subscription is missing"
grep -F 'ES_EVENT_TYPE_NOTIFY_EXIT' "$ROOT/native/macos-activity-collector/main.swift" >/dev/null \
  || fail "exit subscription is missing"
grep -F 'ES_EVENT_TYPE_NOTIFY_CLOSE' "$ROOT/native/macos-activity-collector/main.swift" >/dev/null \
  || fail "file mutation subscription is missing"
grep -F '"content_captured": false' "$ROOT/native/macos-activity-collector/main.swift" >/dev/null \
  || fail "native adapter lacks the metadata-only contract"
pass "native source contract covers process and selected file metadata"

for lifecycle in entitlement signing notarization install upgrade disable uninstall; do
  grep -iF "$lifecycle" "$ROOT/docs/observability/macos-activity-collector.md" >/dev/null \
    || fail "collector lifecycle documentation omitted $lifecycle"
done
if "$ROOT/scripts/build-macos-activity-collector.sh" "$TMP/collector" >/dev/null 2>&1; then
  fail "collector build unexpectedly succeeded without the Apple Silicon/full Xcode gate"
fi
pass "build and lifecycle contracts fail closed before Apple trust-chain approval"
