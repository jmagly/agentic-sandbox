#!/usr/bin/env bash
# shellcheck disable=SC2016  # contract assertions intentionally match literals
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

workflow=.gitea/workflows/macos-release-ceremony.yml
ci=.gitea/workflows/ci.yaml
preflight=scripts/macos-release-preflight.sh

for forbidden_trigger in pull_request push schedule workflow_call workflow_run; do
  if grep -Eq "^  ${forbidden_trigger}:" "$workflow"; then
    echo "manual Apple ceremony must not have a ${forbidden_trigger} trigger" >&2
    exit 1
  fi
done
grep -Fq 'workflow_dispatch:' "$workflow"
grep -Fq -- '- inventory' "$workflow"
grep -Fq -- '- prepare' "$workflow"
grep -Fq -- '- ceremony' "$workflow"
grep -Fq 'ref: ${{ inputs.source_commit }}' "$workflow"
grep -Fq 'git archive --format=tar.gz' "$workflow"
if grep -Fq 'refs/tags/${RELEASE_TAG}^{commit}' "$workflow"; then
  echo "pre-tag ceremony must not require an already-created release tag" >&2
  exit 1
fi
grep -Fq 'management/Cargo.toml agent-rs/Cargo.toml cli/Cargo.toml' "$workflow"

grep -Fq 'keyfile MUTSU_KEY_FILE ${MUTSU_SSH_KEY_VAULT_PATH} ${MUTSU_SSH_KEY_VAULT_FIELD}' \
  "$workflow"
grep -Fq 'MUTSU_SSH_HOST_KEY: ${{ vars.MUTSU_SSH_HOST_KEY }}' "$workflow"
grep -Fq 'MUTSU_SSH_HOST_KEY_FINGERPRINT: ${{ vars.MUTSU_SSH_HOST_KEY_FINGERPRINT }}' \
  "$workflow"
grep -Fq '[[ "$observed_fingerprint" == "$MUTSU_SSH_HOST_KEY_FINGERPRINT" ]]' \
  "$workflow"
if grep -Fq 'ssh-keyscan' "$workflow"; then
  echo "manual Apple ceremony must not trust a live-scanned host key" >&2
  exit 1
fi
grep -Fq 'UserKnownHostsFile=~/.ssh/mutsu_known_hosts' "$workflow"
grep -Fq 'StrictHostKeyChecking=yes' "$workflow"
grep -Fq 'rm -f "$archive" "$config"' "$workflow"
grep -Fq 'rm -rf -- "$REMOTE_ROOT"' "$workflow"
grep -Fq 'ci/vault-fetch.sh --cleanup' "$workflow"

grep -Fq -- '--inventory' "$workflow"
grep -Fq 'stage=sanitized-public-identity-inventory' "$workflow"
grep -Fq 'security find-identity -v -p "$policy" 2>/dev/null' "$preflight"
grep -Fq 'collect_public_selectors codesigning Application' "$preflight"
grep -Fq 'collect_public_selectors basic Installer' "$preflight"
grep -Fq 'credential_contents_retained:false' "$preflight"
if grep -Eq 'echo.*(identity_inventory|notary_history)|cat.*(identity_inventory|notary_history)' \
  "$preflight"; then
  echo "public inventory must not print raw identity or notary responses" >&2
  exit 1
fi
grep -Fq 'agentic.macos-release-approval-bundle.v1' "$workflow"
grep -Fq 'prepared="$base/prepared/$release_tag/$source_commit/$approved_preview_manifest_sha256/$approved_preview_package_sha256"' \
  "$workflow"
grep -Fq 'package_source="$prepared/package-source"' "$workflow"
grep -Fq 'exact immutable approval bundle is unavailable' "$workflow"

grep -Fq 'approved/$release_tag/$source_commit' "$workflow"
grep -Fq 'release-evidence-sha256' docs/releases/macos-signing-ceremony.md
grep -Fq 'immutable:true' "$workflow"
grep -Fq 'mv "$pending" "$handoff"' "$workflow"
grep -Fq '[[ ! -e "$handoff" && ! -e "$pending" ]]' "$workflow"

grep -Fq 'release-macos-promote:' "$ci"
grep -Fq "if: startsWith(gitea.ref, 'refs/tags/v')" "$ci"
if rg -n 'MACOS_RELEASE_PROMOTION_ARMED' "$ci" docs/releases >/dev/null; then
  echo "new release tags must not have a macOS silent-skip switch" >&2
  exit 1
fi
grep -Fq 'needs.release-macos-promote.result == '\''success'\''' "$ci"
grep -Fq 'needs: [release-binaries, release-linux-packages, release-macos-promote, docker, integration]' \
  "$ci"
grep -Fq 'scripts/verify-macos-release-handoff.sh' "$ci"
grep -Fq 'APPROVED_RELEASE_TAG: ${{ vars.MACOS_APPROVED_RELEASE_TAG }}' "$ci"
grep -Fq 'APPROVED_RELEASE_EVIDENCE_SHA256: ${{ vars.MACOS_APPROVED_RELEASE_EVIDENCE_SHA256 }}' \
  "$ci"
grep -Fq -- '--handoff-dir "handoff-staging/${APPROVED_RELEASE_EVIDENCE_SHA256}"' "$ci"
grep -Fq -- '--expected-evidence-sha256 "$APPROVED_RELEASE_EVIDENCE_SHA256"' "$ci"
grep -Fq -- '-name '\''*.release-evidence.json'\''' "$ci"
grep -Fq -- '-name '\''SHA256SUMS-macos'\''' "$ci"
grep -Fq 'needs.release-attach.result == '\''success'\''' "$ci"
grep -Fq 'existing asset digest mismatch' "$ci"
grep -Fq 'GitHub tag does not resolve to exact Gitea release commit' "$ci"
if grep -Fq -- '--clobber' "$ci"; then
  echo "release asset publication must never replace existing bytes" >&2
  exit 1
fi

promotion_block="$(
  sed -n '/^  release-macos-promote:/,/^  release-attach:/p' "$ci"
)"
if grep -Fq 'ssh-keyscan' <<<"$promotion_block"; then
  echo "macOS promotion must not trust a live-scanned host key" >&2
  exit 1
fi
grep -Fq 'MUTSU_SSH_HOST_KEY_FINGERPRINT' <<<"$promotion_block"

echo "macOS release ceremony contract: ok"
