#!/usr/bin/env bash
# Apply the agentshare run/transcript retention policy. Dry-run is the default.
set -euo pipefail

agentshare_root="${AGENTSHARE_ROOT:-/srv/agentshare}"
retention_days="${AGENTIC_RUN_RETENTION_DAYS:-7}"
apply=0

usage() {
    echo "Usage: AGENTSHARE_ROOT=/srv/agentshare $0 [--apply] [--retention-days DAYS]"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --apply) apply=1; shift ;;
        --retention-days) retention_days="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "ERROR: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ "$agentshare_root" != /* || "$agentshare_root" == "/" || ! -d "$agentshare_root" ]]; then
    echo "ERROR: AGENTSHARE_ROOT must be an existing, specific absolute directory" >&2
    exit 2
fi
if [[ ! "$retention_days" =~ ^[0-9]+$ ]]; then
    echo "ERROR: retention days must be a non-negative integer" >&2
    exit 2
fi

pruned=0
while IFS= read -r -d '' run_dir; do
    inbox_dir="$(dirname "$(dirname "$run_dir")")"
    current_target="$(readlink -f "$inbox_dir/current" 2>/dev/null || true)"
    canonical_run="$(readlink -f "$run_dir")"
    if [[ -n "$current_target" && "$current_target" == "$canonical_run" ]]; then
        continue
    fi
    case "$canonical_run" in
        "$agentshare_root"/*/runs/run-*|"$agentshare_root"/*/*/runs/run-*|"$agentshare_root"/*/*/*/runs/run-*) ;;
        *) echo "ERROR: refusing unexpected run path: $canonical_run" >&2; exit 1 ;;
    esac
    pruned=$((pruned + 1))
    if [[ "$apply" == 1 ]]; then
        find "$canonical_run" -xdev -depth -delete
    fi
done < <(
    find "$agentshare_root" -xdev -type d -path '*/runs/run-*' \
        -mtime "+$retention_days" -print0
)

if [[ "$apply" == 1 ]]; then
    echo "retention_apply=true pruned_runs=$pruned"
else
    echo "retention_apply=false eligible_runs=$pruned"
fi
