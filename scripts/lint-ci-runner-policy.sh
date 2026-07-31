#!/usr/bin/env bash
# Enforce the project runner split:
#   - build01 (`s9-build`) handles routine validation and VM-backed E2E;
#   - Titan handles GPU, macOS bridge, and release work;
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
  echo "Route routine validation and VM-backed E2E to s9-build; keep release work on Titan."
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

if [ "$integration_runner" != "s9-build" ]; then
  echo "✗ lint-ci-runner-policy: VM-backed integration job must run on s9-build"
  echo "  observed runs-on: ${integration_runner:-<missing>}"
  echo "build01 is the dedicated KVM/libvirt runner; keep VM E2E off Titan (#626/#627)."
  exit 1
fi

echo "✓ lint-ci-runner-policy: VM-backed integration is pinned to build01"

docker_runner="$(
  awk '
    /^jobs:$/ { in_jobs=1; next }
    in_jobs && /^  [[:alnum:]_-]+:$/ {
      job=$1
      sub(/:$/, "", job)
      next
    }
    job == "docker" && /^    runs-on:/ {
      print $2
      exit
    }
  ' .gitea/workflows/ci.yaml
)"

if [ "$docker_runner" != "s9-build" ]; then
  echo "✗ lint-ci-runner-policy: Docker image builds must run on s9-build"
  echo "  observed runs-on: ${docker_runner:-<missing>}"
  echo "Broad capability selectors can schedule project work on teroknor; pin the Docker job to build01 (#701)."
  exit 1
fi

echo "✓ lint-ci-runner-policy: Docker image builds are pinned to build01"

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

echo "✓ lint-ci-runner-policy: both E2E reaper calls use the build01 storage contract"

docker_cache_reclaim_count="$(
  grep -F -c -- 'docker builder prune --all --force' .gitea/workflows/ci.yaml || true
)"

if [ "$docker_cache_reclaim_count" -ne 1 ]; then
  echo "✗ lint-ci-runner-policy: build01 integration must reclaim completed Docker build cache"
  echo "  cache-reclaim steps: $docker_cache_reclaim_count (expected 1)"
  echo "Docker publishing and VM E2E share build01 root; keep /tmp and libvirt writable (#627)."
  exit 1
fi

echo "✓ lint-ci-runner-policy: build01 E2E reclaims completed Docker build cache"
