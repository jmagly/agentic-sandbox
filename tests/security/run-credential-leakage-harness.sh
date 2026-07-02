#!/usr/bin/env bash
# Release evidence harness for #518.
#
# The harness runs the credential non-exposure checks that can execute without a
# live VM/container. It captures command output, scans it for sentinel secret
# values, and emits a markdown report suitable for release verification records.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ARTIFACT_DIR="${ARTIFACT_DIR:-$ROOT_DIR/.aiwg/testing}"
REPORT_PATH="${REPORT_PATH:-$ARTIFACT_DIR/credential-leakage-harness-$(date +%Y-%m-%d).md}"
LOG_DIR="$(mktemp -d)"
trap 'rm -rf "$LOG_DIR"' EXIT

SENTINELS=(
  "proxy-secret-fake"
  "sk-not-real"
  "sk-ant-not-real"
  "sk-test"
  "sk-test-readiness-secret"
)

mkdir -p "$ARTIFACT_DIR"

COMMANDS=(
  "cargo test --manifest-path management/Cargo.toml credential_proxy -- --nocapture"
  "cargo test --manifest-path management/Cargo.toml credentials -- --nocapture"
  "cargo test --manifest-path management/Cargo.toml startup_profile -- --nocapture"
  "cargo test --manifest-path management/Cargo.toml transcript_query_redacts_provider_secrets_from_hot_and_archive_records -- --nocapture"
  "bash images/qemu/loadouts/tests/test_generate_from_manifest.sh"
)

declare -a RESULTS=()

run_check() {
    local index="$1"
    local command="$2"
    local log="$LOG_DIR/check-$index.log"

    echo "[credential-leakage] running: $command"
    if (cd "$ROOT_DIR" && bash -lc "$command") >"$log" 2>&1; then
        for sentinel in "${SENTINELS[@]}"; do
            if grep -qF -- "$sentinel" "$log"; then
                RESULTS+=("| $index | fail | \`$command\` | Sentinel \`$sentinel\` appeared in harness output. |")
                return 1
            fi
        done
        RESULTS+=("| $index | pass | \`$command\` | Completed; no sentinel appeared in harness output. |")
        return 0
    fi

    RESULTS+=("| $index | fail | \`$command\` | Command failed; see captured log in this run. |")
    cat "$log" >&2
    return 1
}

status=0
for i in "${!COMMANDS[@]}"; do
    run_check "$((i + 1))" "${COMMANDS[$i]}" || status=1
done

{
    echo "# Credential leakage harness evidence"
    echo
    echo "Date: $(date -Iseconds)"
    echo
    echo "## Scope"
    echo
    echo "This harness covers deterministic credential non-exposure checks for:"
    echo
    echo "- HTTP credential proxy allowed, denied, rate-limited, wrong-scope, revoked, expired, and policyless paths."
    echo "- Credential metadata and lease API responses."
    echo "- Startup profile metadata and inline secret rejection."
    echo "- PTY transcript hot replay and archive redaction."
    echo "- QEMU loadout generated cloud-init credential reference policy."
    echo
    echo "Direct upstream bypass is not claimed as denied by this harness. Profiles"
    echo "without an egress allowlist are reported as unsupported for broad proxy"
    echo "non-exposure claims; use network-policy or allowlist verification before"
    echo "claiming bypass prevention."
    echo
    echo "## Results"
    echo
    echo "| # | Status | Command | Evidence |"
    echo "| --- | --- | --- | --- |"
    printf '%s\n' "${RESULTS[@]}"
    echo
    echo "## Sentinel Scan"
    echo
    if [[ "$status" -eq 0 ]]; then
        echo "No configured sentinel value appeared in captured harness command output."
    else
        echo "One or more commands failed or emitted a configured sentinel value."
    fi
    echo
    echo "Sentinels scanned:"
    for sentinel in "${SENTINELS[@]}"; do
        echo "- \`$sentinel\`"
    done
} > "$REPORT_PATH"

echo "[credential-leakage] report: $REPORT_PATH"
exit "$status"
