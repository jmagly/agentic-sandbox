#!/usr/bin/env bash
# Enforce the project runner split:
#   - build01 (`s9-build`) handles validation, build, and VM-backed E2E;
#   - Titan (`titan`) handles GPU, macOS bridge, and release work;
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
  echo "Route validation, builds, and VM-backed E2E to s9-build; keep release work on titan."
  echo "A failure before checkout is runner infrastructure evidence, not a project test result."
  echo "See: docs/releases/runbook.md, issues #626 and #666"
  exit 1
fi

portable_runner_findings="$({
  awk '
    /^jobs:$/ { in_jobs=1; next }
    in_jobs && /^  [[:alnum:]_-]+:$/ {
      job=$1
      sub(/:$/, "", job)
      next
    }
    (job == "lint" || job == "test" || job == "build" || job == "docker" || job == "security") && /^    runs-on:/ && $2 != "s9-build" {
      print FILENAME ":" job ":" $2
    }
  ' .gitea/workflows/ci.yaml

  awk '
    /^jobs:$/ { in_jobs=1; next }
    in_jobs && /^  [[:alnum:]_-]+:$/ {
      job=$1
      sub(/:$/, "", job)
      next
    }
    /^    runs-on:/ && $2 != "s9-build" { print FILENAME ":" job ":" $2 }
  ' .gitea/workflows/schema-lint.yml \
    .gitea/workflows/supply-chain-lint.yml \
    .gitea/workflows/conformance.yml \
    .gitea/workflows/host-runtime.yml
} || true)"

if [ -n "$portable_runner_findings" ]; then
  echo "✗ lint-ci-runner-policy: portable jobs must use the build01 host label"
  printf '  %s\n' "$portable_runner_findings"
  echo "The generic rust label is container-backed and cannot run JavaScript actions."
  exit 1
fi

echo "✓ lint-ci-runner-policy: portable jobs use the build01 host executor"

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

if [ "$integration_runner" != "s9-build" ]; then
  echo "✗ lint-ci-runner-policy: VM-backed integration job must use the build01 host label"
  echo "  observed runs-on: ${integration_runner:-<missing>}"
  echo "build01 provides the verified KVM/libvirt and storage contract required by E2E."
  exit 1
fi

echo "✓ lint-ci-runner-policy: VM-backed integration uses the build01 host executor"

vm_root_forward_count="$(
  grep -F -c -- '--vm-root "${VM_STORAGE_DIR}"' .gitea/workflows/ci.yaml || true
)"
virsh_uri_forward_count="$(
  grep -F -c -- '--virsh-uri "${LIBVIRT_DEFAULT_URI}"' .gitea/workflows/ci.yaml || true
)"

if [ "$vm_root_forward_count" -ne 2 ] || [ "$virsh_uri_forward_count" -ne 2 ]; then
  echo "✗ lint-ci-runner-policy: both E2E reaper calls must receive the configured storage root and libvirt URI"
  echo "  --vm-root forwards: $vm_root_forward_count (expected 2)"
  echo "  --virsh-uri forwards: $virsh_uri_forward_count (expected 2)"
  echo "The reaper CLI does not derive its VM root from VM_STORAGE_DIR; pass both values explicitly."
  exit 1
fi

echo "✓ lint-ci-runner-policy: both E2E reaper calls use the shared storage contract"

docker_cache_reclaim_count="$(
  grep -F -c -- 'docker builder prune --all --force' .gitea/workflows/ci.yaml || true
)"

if [ "$docker_cache_reclaim_count" -ne 1 ]; then
  echo "✗ lint-ci-runner-policy: integration must reclaim completed Docker build cache"
  echo "  cache-reclaim steps: $docker_cache_reclaim_count (expected 1)"
  echo "Docker publishing and VM E2E share runner storage; keep /tmp and libvirt writable (#627)."
  exit 1
fi

echo "✓ lint-ci-runner-policy: shared-runner E2E reclaims completed Docker build cache"
