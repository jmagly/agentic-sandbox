#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$repo_root/scripts/verify-container-security.sh"

grep -F '198.%d.%d.%d/29' "$script" >/dev/null
grep -F "for attempt in \$(seq 0 15)" "$script" >/dev/null
grep -F -- "--subnet \"\$target_subnet\"" "$script" >/dev/null
grep -F -- "--subnet \"\$source_subnet\"" "$script" >/dev/null
grep -F 'probe_networks_created=1' "$script" >/dev/null

if grep -E 'docker network create --internal --label' "$script" >/dev/null; then
    echo 'security probe still contains an automatic-subnet network create' >&2
    exit 1
fi

echo 'container security probe network contract: PASS'
