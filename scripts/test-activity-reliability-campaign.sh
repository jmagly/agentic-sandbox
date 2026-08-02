#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t activity-reliability.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fail() { printf 'not ok - %s\n' "$*" >&2; exit 1; }
pass() { printf 'ok - %s\n' "$*"; }

"$ROOT/scripts/run-activity-reliability-campaign.sh" \
  --output "$TMP/report.json" \
  --state-dir "$TMP/state" \
  --duration-seconds 1 \
  --rate 100 \
  --burst-rate 1000 \
  --burst-seconds 1 \
  --debug-build >/dev/null

jq -e '
  .schema_version == "agentic.activity-soak-report.v1" and
  .campaign.requested_wall_seconds == 1 and
  .campaign.complete == true and
  .campaign.real_time == true and
  .campaign.accelerated == false and
  .pipeline.accepted_this_run == 1000 and
  .pipeline.security_events_this_run == 100 and
  .pipeline.sequence_gap_count == 0 and
  .pipeline.durable_loss_count == 0 and
  .performance.maximum_resident_bytes > 0 and
  .performance.build_profile == "debug" and
  .performance.database_bytes > 0 and
  .performance.ingest_batch_latency_p95_ms >= 0 and
  .performance.query_latency_p95_ms >= 0
' "$TMP/report.json" >/dev/null || fail "one-second contract report is incomplete"
pass "real-time contract campaign records pipeline and resource evidence"

"$ROOT/scripts/run-activity-reliability-campaign.sh" \
  --output "$TMP/resumed.json" \
  --state-dir "$TMP/state" \
  --duration-seconds 2 \
  --rate 100 \
  --burst-rate 1000 \
  --burst-seconds 1 \
  --debug-build >/dev/null

jq -e '
  .campaign.requested_wall_seconds == 2 and
  .campaign.complete == true and
  .pipeline.accepted_this_run == 100 and
  .pipeline.durable_through_sequence == 1100 and
  .pipeline.sequence_gap_count == 0
' "$TMP/resumed.json" >/dev/null || fail "campaign did not resume from its durable checkpoint"
pass "campaign resumes without replay gaps or a repeated burst"

if jq -e '.campaign.requested_wall_seconds < 604800' "$TMP/resumed.json" >/dev/null; then
  jq -e '.evidence_limits | index("short contract runs are not seven-day evidence") != null' \
    "$TMP/resumed.json" >/dev/null || fail "short run could be confused with seven-day evidence"
fi
pass "short fixtures cannot claim seven-day completion"
