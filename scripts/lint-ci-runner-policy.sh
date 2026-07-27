#!/usr/bin/env bash
# Enforce the project runner split:
#   - build01 (`s9-build`) handles routine host and Docker validation;
#   - Titan handles VM-backed E2E and release work;
#   - teroknor is never a project build/test runner.
#
# teroknor is an infrastructure endpoint, not a build/test runner for this
# repository. Keeping this as a repository lint makes a pre-checkout runner
# bootstrap failure distinguishable from a project test failure: project jobs
# must target an approved runner contract and execute this check only after
# checkout.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mapfile -t findings < <(
  grep -rnE '^[[:space:]]*runs-on:[[:space:]]*teroknor([[:space:]#]|$)' \
    .gitea/workflows/ 2>/dev/null || true
)

if [ "${#findings[@]}" -eq 0 ]; then
  echo "✓ lint-ci-runner-policy: no project workflow targets teroknor"
else
  echo "✗ lint-ci-runner-policy: teroknor is not a project build/test runner"
  printf '  %s\n' "${findings[@]}"
  echo
  echo "Route routine host/Docker validation to s9-build and VM/release work to Titan."
  echo "A failure before checkout is runner infrastructure evidence, not a project test result."
  echo "See: docs/releases/runbook.md, issues #626 and #666"
  exit 1
fi

integration_runner="$(
  awk '
    /^jobs:$/ { in_jobs=1; next }
    in_jobs && /^  [[:alnum:]_-]+:$/ {
      job=$1
      sub(/:$/, "", job)
      next
    }
    job == "integration" && /^    runs-on:/ {
      print $2
      exit
    }
  ' .gitea/workflows/ci.yaml
)"

if [ "$integration_runner" != "titan" ]; then
  echo "✗ lint-ci-runner-policy: VM-backed integration job must run on titan"
  echo "  observed runs-on: ${integration_runner:-<missing>}"
  echo "build01 has no VM substrate; keep libvirt/cloud-hypervisor E2E on Titan (#626)."
  exit 1
fi

echo "✓ lint-ci-runner-policy: VM-backed integration remains pinned to titan"
