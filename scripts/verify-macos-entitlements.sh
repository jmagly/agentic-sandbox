#!/usr/bin/env bash
# Require the exact declared entitlement set for every signed Mach-O payload.

set -euo pipefail
umask 077

EXPECTED=""
OUTPUT=""
BINARIES=()

usage() {
  cat <<'EOF'
Usage: scripts/verify-macos-entitlements.sh \
  --expected <entitlements.plist> --output <results.tsv> <Mach-O>...

The output contains only payload paths, SHA-256 digests, and exact-match state.
Any missing or additional entitlement fails closed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expected) EXPECTED="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --) shift; BINARIES+=("$@"); break ;;
    -*) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    *) BINARIES+=("$1"); shift ;;
  esac
done

[[ -f "$EXPECTED" ]] || { echo "expected entitlements plist is unavailable" >&2; exit 1; }
[[ -n "$OUTPUT" ]] || { echo "missing --output" >&2; exit 2; }
((${#BINARIES[@]} > 0)) || { echo "no Mach-O payloads supplied" >&2; exit 2; }

for command_name in codesign jq plutil shasum; do
  command -v "$command_name" >/dev/null 2>&1 \
    || { printf 'required command not found: %s\n' "$command_name" >&2; exit 1; }
done

scratch="$(mktemp -d -t agentic-macos-entitlements.XXXXXX)"
cleanup() {
  rm -rf "$scratch"
}
trap cleanup EXIT INT TERM

plutil -convert json -o - "$EXPECTED" | jq -cS . > "$scratch/expected.json"
expected_sha256="$(shasum -a 256 "$EXPECTED" | awk '{print $1}')"
[[ "$expected_sha256" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "expected entitlements digest is invalid" >&2; exit 1; }

: > "$OUTPUT"
chmod 0600 "$OUTPUT"
for binary in "${BINARIES[@]}"; do
  [[ -f "$binary" ]] || { echo "signed Mach-O payload is unavailable" >&2; exit 1; }
  actual_plist="$scratch/actual.plist"
  actual_json="$scratch/actual.json"
  : > "$actual_plist"
  if ! codesign -d --entitlements :- "$binary" >"$actual_plist" 2>/dev/null; then
    echo "unable to read signed Mach-O entitlements" >&2
    exit 1
  fi
  if [[ -s "$actual_plist" ]]; then
    plutil -convert json -o - "$actual_plist" | jq -cS . > "$actual_json"
  else
    printf '{}\n' > "$actual_json"
  fi
  if ! cmp -s "$scratch/expected.json" "$actual_json"; then
    echo "signed Mach-O entitlements differ from the closed expected set" >&2
    exit 1
  fi
  binary_sha256="$(shasum -a 256 "$binary" | awk '{print $1}')"
  [[ "$binary_sha256" =~ ^[0-9a-f]{64}$ ]] \
    || { echo "signed Mach-O digest is invalid" >&2; exit 1; }
  printf '%s\t%s\t%s\texact\n' \
    "${binary##*/}" "$binary_sha256" "$expected_sha256" >> "$OUTPUT"
done

chmod 0644 "$OUTPUT"
