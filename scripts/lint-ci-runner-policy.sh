#!/usr/bin/env bash
# Enforce the project runner split:
#   - build01 and Titan (`rust`) share validation, build, and VM-backed E2E;
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

# Runner registration tokens are reusable credentials. Diagnostics must only
# call runner inventory endpoints; retrieving a registration token is a
# provisioning/rotation operation and must never appear in repository
# automation. Keep the endpoint assembled so this guard does not match itself.
runner_token_endpoint='actions/runners/'"registration-token"
mapfile -t runner_token_endpoint_findings < <(
  grep -rnF -- "$runner_token_endpoint" .gitea/ scripts/ Makefile \
    --exclude='lint-ci-runner-policy.sh' 2>/dev/null || true
)

if [ "${#runner_token_endpoint_findings[@]}" -ne 0 ]; then
  echo "✗ lint-ci-runner-policy: automation must not retrieve runner registration tokens"
  printf '  %s\n' "${runner_token_endpoint_findings[@]}"
  echo "Use scripts/audit-gitea-runner-inventory.sh for metadata-only diagnostics."
  exit 1
fi

echo "✓ lint-ci-runner-policy: diagnostics avoid runner registration-token endpoints"

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
  echo "Route validation, builds, and VM-backed E2E to rust; keep release work on titan."
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
    (job == "lint" || job == "test" || job == "build" || job == "docker" || job == "security") && /^    runs-on:/ && $2 != "rust" {
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
    /^    runs-on:/ && $2 != "rust" { print FILENAME ":" job ":" $2 }
  ' .gitea/workflows/schema-lint.yml \
    .gitea/workflows/supply-chain-lint.yml \
    .gitea/workflows/conformance.yml \
    .gitea/workflows/host-runtime.yml
} || true)"

if [ -n "$portable_runner_findings" ]; then
  echo "✗ lint-ci-runner-policy: portable jobs must use the shared rust runner label"
  printf '  %s\n' "$portable_runner_findings"
  echo "The rust label allows either build01 or Titan to claim portable work."
  exit 1
fi

echo "✓ lint-ci-runner-policy: portable jobs use the shared build01/Titan pool"

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

if [ "$integration_runner" != "rust" ]; then
  echo "✗ lint-ci-runner-policy: VM-backed integration job must use the shared rust runner label"
  echo "  observed runs-on: ${integration_runner:-<missing>}"
  echo "Both build01 and Titan provide the KVM/libvirt and storage contract required by E2E."
  exit 1
fi

echo "✓ lint-ci-runner-policy: VM-backed integration uses the shared build01/Titan pool"

integration_max_parallel="$(
  awk '
    /^  integration:$/ { in_job=1; next }
    in_job && /^  [[:alnum:]_-]+:$/ { exit }
    in_job && /^      max-parallel:/ { print $2; exit }
  ' .gitea/workflows/ci.yaml
)"

if [ "$integration_max_parallel" != "2" ]; then
  echo "✗ lint-ci-runner-policy: E2E matrix must expose both host-isolated capacity lanes"
  echo "  observed max-parallel: ${integration_max_parallel:-<missing>}"
  echo "Gitea does not enforce max-parallel=1 across runners; two capacity-one hosts are explicitly allowed to overlap (#796)."
  exit 1
fi

echo "✓ lint-ci-runner-policy: E2E matrix uses two explicit host-isolated capacity lanes"

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

maintenance=.gitea/workflows/runner-maintenance.yml
mount_first_count="$(grep -F -c "mount-table-before-path-probes:" "$maintenance" || true)"
typed_probe_count="$(grep -F -c '::error::path-probe-timeout path=${path} timeout=5s' "$maintenance" || true)"
bounded_stat_count="$(grep -F -c 'timeout --signal=TERM --kill-after=1s 5s' "$maintenance" || true)"

if [ "$mount_first_count" -ne 2 ] || [ "$typed_probe_count" -ne 2 ] || [ "$bounded_stat_count" -lt 2 ]; then
  echo "✗ lint-ci-runner-policy: both maintenance audits must inspect mountinfo before bounded typed path probes"
  echo "  mount-table markers: $mount_first_count (expected 2)"
  echo "  typed timeout markers: $typed_probe_count (expected 2)"
  echo "  bounded probe commands: $bounded_stat_count (expected at least 2)"
  exit 1
fi

titan_dependency="$(
  awk '
    /^  bootstrap-titan:$/ { in_job=1; next }
    in_job && /^  [[:alnum:]_-]+:$/ { exit }
    in_job && /^    needs:/ { print $2; exit }
  ' "$maintenance"
)"

if [ "$titan_dependency" != "audit-titan" ]; then
  echo "✗ lint-ci-runner-policy: Titan mutation must depend on a successful Titan audit"
  echo "  observed dependency: ${titan_dependency:-<missing>}"
  exit 1
fi

if ! grep -Fq '::error::maintenance-timeout host=titan timeout=15m' "$maintenance"; then
  echo "✗ lint-ci-runner-policy: Titan maintenance mutation needs a typed outer timeout"
  exit 1
fi

echo "✓ lint-ci-runner-policy: runner maintenance is bounded and audit-gated"
