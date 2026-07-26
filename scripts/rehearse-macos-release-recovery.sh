#!/usr/bin/env bash
# Exercise release state transitions without touching Keychain or Apple services.

set -euo pipefail
umask 077

WORKSPACE=""
OUTPUT=""
SOURCE_COMMIT=""
RELEASE_TAG=""
SUPERSEDING_TAG=""
OPERATOR_APPROVAL_REF=""
SYNTHETIC_CERTIFICATE_REF=""
SYNTHETIC_NOTARY_PROFILE=""

usage() {
  cat <<'EOF'
Usage: scripts/rehearse-macos-release-recovery.sh [options]

Required:
  --workspace <empty-dir>
  --output <report.json>
  --source-commit <40-char-sha>
  --release-tag <vCalVer>
  --superseding-tag <vCalVer>
  --operator-approval-ref <synthetic-ref>
  --synthetic-certificate-ref <synthetic-ref>
  --synthetic-notary-profile <synthetic-name>

This rehearsal moves only generated fixture artifacts. It never invokes
security, codesign, notarytool, stapler, Keychain, or a release publisher.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --workspace) WORKSPACE="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    --source-commit) SOURCE_COMMIT="${2:-}"; shift 2 ;;
    --release-tag) RELEASE_TAG="${2:-}"; shift 2 ;;
    --superseding-tag) SUPERSEDING_TAG="${2:-}"; shift 2 ;;
    --operator-approval-ref) OPERATOR_APPROVAL_REF="${2:-}"; shift 2 ;;
    --synthetic-certificate-ref) SYNTHETIC_CERTIFICATE_REF="${2:-}"; shift 2 ;;
    --synthetic-notary-profile) SYNTHETIC_NOTARY_PROFILE="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$WORKSPACE" && -n "$OUTPUT" ]] || { echo "workspace and output are required" >&2; exit 2; }
[[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid synthetic source commit" >&2; exit 2; }
[[ "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "invalid release tag" >&2; exit 2; }
[[ "$SUPERSEDING_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "invalid superseding tag" >&2; exit 2; }
[[ "$SUPERSEDING_TAG" != "$RELEASE_TAG" ]] || { echo "superseding tag must differ" >&2; exit 2; }
[[ "$OPERATOR_APPROVAL_REF" == synthetic-* ]] \
  || { echo "rehearsal approval reference must be synthetic" >&2; exit 2; }
[[ "$SYNTHETIC_CERTIFICATE_REF" == synthetic-* ]] \
  || { echo "certificate reference must be synthetic" >&2; exit 2; }
[[ "$SYNTHETIC_NOTARY_PROFILE" == synthetic-* ]] \
  || { echo "notary profile must be synthetic" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }
command -v shasum >/dev/null 2>&1 || { echo "shasum is required" >&2; exit 1; }

[[ ! -e "$WORKSPACE" ]] || {
  [[ -d "$WORKSPACE" && -z "$(find "$WORKSPACE" -mindepth 1 -print -quit)" ]] \
    || { echo "rehearsal workspace must be absent or empty" >&2; exit 1; }
}
mkdir -p "$WORKSPACE/candidate" "$WORKSPACE/quarantine" "$WORKSPACE/superseded"

candidate="$WORKSPACE/candidate/agentic-sandbox-${RELEASE_TAG}.pkg"
printf 'synthetic release recovery fixture\n' > "$candidate"
candidate_digest="$(shasum -a 256 "$candidate" | awk '{print $1}')"

# Abort immediately blocks publication; quarantine removes the candidate from
# the only publication-eligible directory.
mv "$candidate" "$WORKSPACE/quarantine/"
[[ ! -e "$candidate" ]] || { echo "synthetic abort failed to block publication" >&2; exit 1; }
quarantined="$WORKSPACE/quarantine/$(basename "$candidate")"
[[ -f "$quarantined" ]] || { echo "synthetic quarantine failed" >&2; exit 1; }

# Superseding creates a metadata record; it never deletes or re-labels the
# quarantined artifact as publishable.
jq -n \
  --arg superseded_tag "$RELEASE_TAG" \
  --arg superseding_tag "$SUPERSEDING_TAG" \
  '{superseded_tag:$superseded_tag,superseding_tag:$superseding_tag}' \
  > "$WORKSPACE/superseded/${RELEASE_TAG}.json"

jq -n \
  --arg schema_version "agentic.macos-release-recovery-rehearsal.v1" \
  --arg source_commit "$SOURCE_COMMIT" \
  --arg release_tag "$RELEASE_TAG" \
  --arg superseding_tag "$SUPERSEDING_TAG" \
  --arg operator_approval_ref "$OPERATOR_APPROVAL_REF" \
  --arg artifact_sha256 "$candidate_digest" \
  --arg certificate_ref "$SYNTHETIC_CERTIFICATE_REF" \
  --arg notary_profile "$SYNTHETIC_NOTARY_PROFILE" \
  '{
    schema_version: $schema_version,
    synthetic: true,
    source_commit: $source_commit,
    release_tag: $release_tag,
    operator_approval_ref: $operator_approval_ref,
    scenarios: {
      abort: {
        result: "passed",
        publication_state: "blocked"
      },
      quarantine: {
        result: "passed",
        artifact_sha256: $artifact_sha256,
        publication_eligible: false
      },
      supersede: {
        result: "passed",
        superseding_tag: $superseding_tag,
        original_artifact_state: "quarantined"
      },
      certificate_revocation: {
        result: "passed",
        certificate_ref: $certificate_ref,
        publication_state: "blocked",
        operator_rotation_required: true,
        credential_operation_performed: false
      },
      notary_profile_retirement: {
        result: "passed",
        profile_name: $notary_profile,
        future_submission_state: "blocked",
        operator_retirement_required: true,
        keychain_operation_performed: false
      }
    },
    credential_contents_retained: false
  }' > "$OUTPUT"
chmod 0644 "$OUTPUT"
