#!/usr/bin/env bash
# lint-admin-openapi-coverage.sh — structural diff between the admin OpenAPI
# spec at docs/contracts/admin-api.openapi.yaml and the Axum router at
# management/src/http/admin_v2.rs.
#
# Issue #215 acceptance: keep the spec and the implementation in lock-step.
# Failure modes detected:
#   - Spec declares a path that the router does not register (broken contract)
#   - Router registers a path that the spec does not document (silent surface)
#
# Path normalization: Axum's `{x}` and `{*x}` (wildcard) both map to the
# OpenAPI `{x}` shape for the purposes of structural diff. Param names are
# preserved so renames are detected.
#
# Exits 0 if spec and router agree, 1 otherwise.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SPEC="docs/contracts/admin-api.openapi.yaml"
CODE="management/src/http/admin_v2.rs"

[ -f "$SPEC" ] || { echo "✗ lint-admin-openapi-coverage: $SPEC not found" >&2; exit 1; }
[ -f "$CODE" ] || { echo "✗ lint-admin-openapi-coverage: $CODE not found" >&2; exit 1; }

# Extract spec paths: lines starting with "  /...:" at indent 2 (top-level under "paths:")
spec_paths=$(awk '
  /^paths:/ { in_paths = 1; next }
  in_paths && /^[a-zA-Z]/ { in_paths = 0 }
  in_paths && /^  \// {
    sub(/:$/, "", $1);
    print $1;
  }
' "$SPEC" | sort -u)

# Extract code routes: .route("PATH", ...). Some are split across lines so
# fold the file first to a single line per `.route(...)` call. Normalize
# `{*name}` → `{name}` (Axum wildcard) for diffability against OpenAPI.
code_paths=$(tr '\n' ' ' < "$CODE" \
  | grep -oE '\.route\([[:space:]]*"[^"]+"' \
  | sed -E 's/^\.route\([[:space:]]*"([^"]+)".*/\1/' \
  | sed -E 's/\{\*([^}]+)\}/{\1}/g' \
  | sort -u)

# Diff
spec_only=$(comm -23 <(echo "$spec_paths") <(echo "$code_paths"))
code_only=$(comm -13 <(echo "$spec_paths") <(echo "$code_paths"))

fail=0
if [ -n "$spec_only" ]; then
  echo "✗ Paths declared in spec but NOT implemented in router:"
  echo "$spec_only" | sed 's/^/    /'
  fail=1
fi
if [ -n "$code_only" ]; then
  echo "✗ Paths implemented in router but NOT documented in spec:"
  echo "$code_only" | sed 's/^/    /'
  fail=1
fi

if [ $fail -eq 0 ]; then
  spec_count=$(echo "$spec_paths" | grep -c '^/')
  echo "✓ lint-admin-openapi-coverage: spec and router agree on $spec_count paths"
  exit 0
fi

echo
echo "Spec: $SPEC"
echo "Code: $CODE"
echo "Fix the surface mismatch and re-run."
echo "See: issue #215"
exit 1
