#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT=""
STATE_DIR=""
DURATION_SECONDS=604800
RATE=100
BURST_RATE=1000
BURST_SECONDS=60
DEBUG_BUILD=0

usage() {
  printf '%s\n' \
    'Usage: scripts/run-activity-reliability-campaign.sh --output <report.json> [options]' \
    '' \
    'Options:' \
    '  --state-dir <dir>          durable resume state (default: <output>.state)' \
    '  --duration-seconds <n>     real wall time (default: 604800 / seven days)' \
    '  --rate <n>                 steady events/second (default: 100)' \
    '  --burst-rate <n>           burst events/second (default: 1000)' \
    '  --burst-seconds <n>        initial burst duration (default: 60)' \
    '  --debug-build              contract tests only; not budget evidence'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) OUTPUT="${2:-}"; shift 2 ;;
    --state-dir) STATE_DIR="${2:-}"; shift 2 ;;
    --duration-seconds) DURATION_SECONDS="${2:-}"; shift 2 ;;
    --rate) RATE="${2:-}"; shift 2 ;;
    --burst-rate) BURST_RATE="${2:-}"; shift 2 ;;
    --burst-seconds) BURST_SECONDS="${2:-}"; shift 2 ;;
    --debug-build) DEBUG_BUILD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

[ -n "$OUTPUT" ] || { echo "missing --output" >&2; usage >&2; exit 2; }
if [ -z "$STATE_DIR" ]; then STATE_DIR="${OUTPUT}.state"; fi

cd "$ROOT/management"
set -- run --quiet --example activity_reliability_campaign
if [ "$DEBUG_BUILD" = "0" ]; then set -- "$@" --release; fi
cargo "$@" -- \
  --output "$OUTPUT" \
  --state-dir "$STATE_DIR" \
  --duration-seconds "$DURATION_SECONDS" \
  --rate "$RATE" \
  --burst-rate "$BURST_RATE" \
  --burst-seconds "$BURST_SECONDS"
