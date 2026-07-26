#!/usr/bin/env bash
# Fail when an active project workflow assigns build or test work to teroknor.
#
# teroknor is an infrastructure endpoint, not a build/test runner for this
# repository. Keeping this as a repository lint makes a pre-checkout runner
# bootstrap failure distinguishable from a project test failure: project jobs
# must target the supported Titan runner contract and execute this check only
# after checkout.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mapfile -t findings < <(
  grep -rnE '^[[:space:]]*runs-on:[[:space:]]*teroknor([[:space:]#]|$)' \
    .gitea/workflows/ 2>/dev/null || true
)

if [ "${#findings[@]}" -eq 0 ]; then
  echo "✓ lint-ci-runner-policy: no project workflow targets teroknor"
  exit 0
fi

echo "✗ lint-ci-runner-policy: teroknor is not a project build/test runner"
printf '  %s\n' "${findings[@]}"
echo
echo "Route project validation to the supported Titan runner contract."
echo "A failure before checkout is runner infrastructure evidence, not a project test result."
echo "See: docs/releases/runbook.md, issue #666"
exit 1
