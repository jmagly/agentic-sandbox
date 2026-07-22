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

echo "macOS validation contract: ok"
