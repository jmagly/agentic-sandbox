#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

inbox="$scratch/instances/test/inbox"
old="$inbox/runs/run-old"
current="$inbox/runs/run-current"
fresh="$inbox/runs/run-fresh"
mkdir -p "$old" "$current" "$fresh"
printf 'sensitive-old\n' >"$old/stdout.log"
printf 'sensitive-current\n' >"$current/commands.log"
printf 'sensitive-fresh\n' >"$fresh/stdout.log"
ln -s "$current" "$inbox/current"
touch -d '3 days ago' "$old" "$current"

dry="$(
    AGENTSHARE_ROOT="$scratch" \
        "$root/scripts/prune-agentshare-runs.sh" --retention-days 1
)"
[[ "$dry" == "retention_apply=false eligible_runs=1" ]]
[[ -d "$old" && -d "$current" && -d "$fresh" ]]

applied="$(
    AGENTSHARE_ROOT="$scratch" \
        "$root/scripts/prune-agentshare-runs.sh" --retention-days 1 --apply
)"
[[ "$applied" == "retention_apply=true pruned_runs=1" ]]
[[ ! -e "$old" ]]
[[ -f "$current/commands.log" ]]
[[ -f "$fresh/stdout.log" ]]

echo "agentshare run retention checks passed"
