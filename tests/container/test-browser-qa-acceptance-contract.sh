#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow="$repo_root/.gitea/workflows/browser-qa-acceptance.yml"
runner="$repo_root/scripts/run-browser-qa-acceptance.sh"
provisioner="$repo_root/images/qemu/provision-vm.sh"

grep -Fq 'CARGO_BUILD_JOBS: "4"' "$workflow"
grep -Fq 'CARGO_TARGET_DIR: ${{ github.workspace }}/agent-client-target' "$workflow"
grep -Fq -- '--manifest-path agentic-sandbox/agent-rs/Cargo.toml' "$workflow"
grep -Fq -- '--bin agent-client' "$workflow"
grep -Fq 'AGENT_CLIENT_SOURCE_BIN=%s' "$workflow"
grep -Fq '>> "$GITHUB_ENV"' "$workflow"
grep -Fq 'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02' \
    "$workflow"

# The exact binary built by the workflow must remain the source installed into
# the guest; falling back to a host-global artifact would break ref fidelity.
grep -Fq '"$PROJECT_ROOT/images/qemu/provision-vm.sh"' "$runner"
grep -Fq 'AGENT_CLIENT_SOURCE_BIN:-$repo_root/agent-rs/target/release/agent-client' \
    "$provisioner"
grep -Fq 'AGENT_CLIENT_SOURCE_BIN="$agent_binary"' "$provisioner"

echo "PASS: browser QA workflow builds and forwards its exact agent client"
