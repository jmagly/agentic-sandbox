#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t agentic-macos-recovery-tests.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fail() { printf 'not ok - %s\n' "$*" >&2; exit 1; }
pass() { printf 'ok - %s\n' "$*"; }

REPORT="$TMP/recovery.json"
"$ROOT/scripts/rehearse-macos-release-recovery.sh" \
  --workspace "$TMP/workspace" \
  --output "$REPORT" \
  --source-commit 3333333333333333333333333333333333333333 \
  --release-tag v2026.7.13 \
  --superseding-tag v2026.7.14 \
  --operator-approval-ref synthetic-issue-677-witness \
  --synthetic-certificate-ref synthetic-developer-id-certificate \
  --synthetic-notary-profile synthetic-retired-profile

jq -e '
  .synthetic == true and
  .scenarios.abort.publication_state == "blocked" and
  .scenarios.quarantine.publication_eligible == false and
  .scenarios.supersede.superseding_tag == "v2026.7.14" and
  .scenarios.supersede.original_artifact_state == "quarantined" and
  .scenarios.certificate_revocation.operator_rotation_required == true and
  .scenarios.certificate_revocation.credential_operation_performed == false and
  .scenarios.notary_profile_retirement.operator_retirement_required == true and
  .scenarios.notary_profile_retirement.keychain_operation_performed == false and
  .credential_contents_retained == false
' "$REPORT" >/dev/null || fail "synthetic recovery report is incomplete"
test ! -e "$TMP/workspace/candidate/agentic-sandbox-v2026.7.13.pkg" \
  || fail "aborted candidate remained publication eligible"
test -f "$TMP/workspace/quarantine/agentic-sandbox-v2026.7.13.pkg" \
  || fail "aborted candidate was not quarantined"
test -f "$TMP/workspace/superseded/v2026.7.13.json" \
  || fail "superseding release record was not retained"

if rg -n '^[[:space:]]*(security|codesign|xcrun|stapler)([[:space:]]|$)' \
  "$ROOT/scripts/rehearse-macos-release-recovery.sh" >/dev/null; then
  fail "synthetic rehearsal contains a credential or Apple-service operation"
fi
pass "abort, quarantine, supersede, revocation, and profile retirement are rehearsed synthetically"

if "$ROOT/scripts/rehearse-macos-release-recovery.sh" \
  --workspace "$TMP/unsafe" \
  --output "$TMP/unsafe.json" \
  --source-commit 3333333333333333333333333333333333333333 \
  --release-tag v2026.7.13 \
  --superseding-tag v2026.7.14 \
  --operator-approval-ref real-approval \
  --synthetic-certificate-ref synthetic-certificate \
  --synthetic-notary-profile synthetic-profile >/dev/null 2>&1; then
  fail "rehearsal accepted a non-synthetic operator reference"
fi
pass "rehearsal refuses non-synthetic identity and approval references"
