#!/bin/bash
# resolve-manifest.sh - Resolve loadout manifest inheritance
#
# Reads a YAML manifest, recursively resolves its extends chain,
# and outputs a single merged YAML document to stdout.
#
# Merge rules:
#   - Scalars: last value wins (most-specific manifest)
#   - Lists of strings: union with deduplication
#   - Lists of objects: concatenate (e.g., aiwg.frameworks)
#   - Maps: deep merge (recursive)
#
# Usage: ./resolve-manifest.sh <manifest.yaml>
# Output: Merged YAML to stdout
#
# Dependencies: python3, PyYAML (usually pre-installed on Ubuntu)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOADOUTS_DIR="$SCRIPT_DIR"

die() { echo "error: $1" >&2; exit 1; }

# Resolve a manifest path (relative to loadouts/ or absolute)
resolve_path() {
    local path="$1"
    if [[ "$path" = /* ]]; then
        echo "$path"
    else
        echo "$LOADOUTS_DIR/$path"
    fi
}

# Python-based deep merge (handles all merge rules correctly)
deep_merge_py() {
    python3 -c "
import sys, yaml, json

def deep_merge(base, overlay):
    \"\"\"Recursively merge overlay into base.\"\"\"
    if not isinstance(base, dict) or not isinstance(overlay, dict):
        return overlay if overlay is not None else base

    result = dict(base)
    for key, val in overlay.items():
        if key in result:
            bval = result[key]
            if isinstance(bval, dict) and isinstance(val, dict):
                result[key] = deep_merge(bval, val)
            elif isinstance(bval, list) and isinstance(val, list):
                # String arrays: union + dedup preserving order
                if bval and val and isinstance(bval[0], str) and isinstance(val[0], str):
                    seen = set()
                    merged = []
                    for item in bval + val:
                        if item not in seen:
                            seen.add(item)
                            merged.append(item)
                    result[key] = sorted(merged)
                # Object arrays: concatenate (e.g., frameworks)
                elif bval and val and isinstance(bval[0], dict) and isinstance(val[0], dict):
                    result[key] = bval + val
                else:
                    result[key] = val
            else:
                result[key] = val
        else:
            result[key] = val
    return result

with open(sys.argv[1]) as f:
    base = yaml.safe_load(f) or {}
with open(sys.argv[2]) as f:
    overlay = yaml.safe_load(f) or {}

merged = deep_merge(base, overlay)
yaml.dump(merged, sys.stdout, default_flow_style=False, sort_keys=False, width=120)
" "$1" "$2"
}

# Track visited manifests for cycle detection
declare -A VISITED=()

# Recursively resolve a manifest's extends chain
# Returns path to a tempfile containing the fully merged YAML
resolve() {
    local manifest_path
    manifest_path=$(resolve_path "$1")

    [[ -f "$manifest_path" ]] || die "manifest not found: $manifest_path"

    # Cycle detection
    local abs_path
    abs_path=$(realpath "$manifest_path")
    if [[ -n "${VISITED[$abs_path]:-}" ]]; then
        die "circular extends detected: $abs_path"
    fi
    VISITED["$abs_path"]=1

    # Extract extends list
    local extends
    extends=$(python3 -c "
import yaml, sys
with open('$manifest_path') as f:
    d = yaml.safe_load(f) or {}
for e in d.get('extends', []):
    print(e)
" 2>/dev/null || true)

    # Start with empty base
    local merged
    merged=$(mktemp /tmp/loadout-merge.XXXXXX.yaml)
    echo '{}' > "$merged"

    # Recursively resolve and merge each parent (depth-first, left-to-right)
    if [[ -n "$extends" ]]; then
        while IFS= read -r parent; do
            [[ -n "$parent" ]] || continue
            local resolved_parent
            resolved_parent=$(resolve "$parent")
            local new_merged
            new_merged=$(mktemp /tmp/loadout-merge.XXXXXX.yaml)
            deep_merge_py "$merged" "$resolved_parent" > "$new_merged"
            rm -f "$merged" "$resolved_parent"
            merged="$new_merged"
        done <<< "$extends"
    fi

    # Merge this manifest on top (highest priority)
    # Strip the extends field first (it's been resolved)
    local stripped
    stripped=$(mktemp /tmp/loadout-merge.XXXXXX.yaml)
    python3 -c "
import yaml, sys
with open('$manifest_path') as f:
    d = yaml.safe_load(f) or {}
d.pop('extends', None)
yaml.dump(d, sys.stdout, default_flow_style=False, sort_keys=False, width=120)
" > "$stripped"

    local final
    final=$(mktemp /tmp/loadout-merge.XXXXXX.yaml)
    deep_merge_py "$merged" "$stripped" > "$final"
    rm -f "$merged" "$stripped"

    # Unmark for cycle detection (allow diamond inheritance)
    unset VISITED["$abs_path"]

    echo "$final"
}

# --- Main ---

[[ $# -ge 1 ]] || die "usage: $0 <manifest.yaml>"

manifest="$1"
result=$(resolve "$manifest")
cat "$result"
rm -f "$result"

# Clean up any stale temp files
rm -f /tmp/loadout-merge.*.yaml 2>/dev/null || true
