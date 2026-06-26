#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

binary="$TMPDIR/agentic-mgmt"
input="$TMPDIR/src"
mkdir -p "$input"
touch "$input/main.rs"
touch "$binary"
chmod +x "$binary"

fresh=$(
    cd "$ROOT_DIR/management"
    AGENTIC_DEV_BINARY="$binary" \
    AGENTIC_DEV_BUILD_INPUTS="$input" \
        ./dev.sh __needs-build
)
if [[ "$fresh" != "fresh" ]]; then
    echo "FAIL: expected fresh binary, got: $fresh" >&2
    exit 1
fi

sleep 1
touch "$input/main.rs"

stale=$(
    cd "$ROOT_DIR/management"
    AGENTIC_DEV_BINARY="$binary" \
    AGENTIC_DEV_BUILD_INPUTS="$input" \
        ./dev.sh __needs-build
)
if [[ "$stale" != "needs-build" ]]; then
    echo "FAIL: expected stale binary to need build, got: $stale" >&2
    exit 1
fi

rm -f "$binary"

missing=$(
    cd "$ROOT_DIR/management"
    AGENTIC_DEV_BINARY="$binary" \
    AGENTIC_DEV_BUILD_INPUTS="$input" \
        ./dev.sh __needs-build
)
if [[ "$missing" != "needs-build" ]]; then
    echo "FAIL: expected missing binary to need build, got: $missing" >&2
    exit 1
fi

echo "PASS dev.sh build freshness regression"
