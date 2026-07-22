#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

workflow=.gitea/workflows/macos-validation.yml
validation=scripts/macos-validation.sh

# The remote login/session owns the location of its existing Rust toolchain.
# CI may extend that PATH, but must not replace it or pin a workstation path.
grep -Fq "export PATH=\"\$PATH:" "$workflow"
if grep -Fq '/Volumes/build/bt6' "$workflow"; then
  echo "macOS validation workflow must not pin mutsu's current toolchain volume" >&2
  exit 1
fi

# Prerequisites fail before the expensive build, and Docker errors stay
# sanitized rather than emitting daemon socket or user-directory details.
grep -Fq 'required_tools=(rustc cargo docker jq curl file lsof)' "$validation"
grep -Fq 'required mutsu validation tools are unavailable on PATH' "$validation"
grep -Fq 'the active Docker CLI context cannot reach a daemon' "$validation"
grep -Fq "docker version --format 'docker_client={{.Client.Version}} docker_server={{.Server.Version}} docker_arch={{.Server.Arch}}' 2>/dev/null" "$validation"

# Bootstrap enrollment carries the one-time credential over the dedicated
# server-authenticated TLS listener. The agent intentionally rejects HTTP.
grep -Fq 'for port in 48120 48122 48123 48124' "$validation"
grep -Fq 'AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL=https://host.docker.internal:48124/api/v1/bootstrap-enrollment/consume' "$validation"
if grep -Fq 'AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL=http://' "$validation"; then
  echo "macOS validation must not send bootstrap enrollment over plaintext HTTP" >&2
  exit 1
fi

echo "macOS validation contract: ok"
