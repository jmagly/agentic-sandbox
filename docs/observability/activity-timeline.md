# Correlated activity timeline runbook

The activity timeline reconstructs metadata-only evidence from guest, runtime,
host, control-plane, and provider collectors. It never substitutes for raw
terminal content and never describes a timeline as complete until coverage has
been evaluated.

## Configure signed exports

Create a root-readable file containing at least 32 random bytes and configure:

```sh
export AGENTIC_ACTIVITY_EXPORT_HMAC_KEY_FILE=/run/secrets/activity-export-hmac
export AGENTIC_ACTIVITY_EXPORT_KEY_ID=activity-export-2026-08
```

The management server reads the key at startup. It logs the path and key ID
only, never the key. `POST /api/v2/activity/export` returns `503` when the key is
not configured. Rotate keys by changing both material and key ID; retain old
verification keys through the longest applicable retention or hold period.

## Scope and authorization

All activity routes require operator-admin authentication plus these exact scope
headers:

```text
x-agentic-tenant-id
x-agentic-host-id
x-agentic-instance-id
x-agentic-agent-id
```

`x-agentic-collector-id` is required for ingest and optional for query/export.
Scope is applied in the SQLite predicate before event filters. A caller cannot
query another tenant by placing tenant, host, instance, or agent identifiers in
the URL. The supported timeline is metadata-only. Restricted content, live raw
streams, and transcript links are deliberately absent from this surface and
remain behind their separate grant/content-custodian authorization boundary.

## CLI queries

Every command starts with the coverage assessment. Example session timeline:

```sh
sandboxctl activity timeline \
  --tenant tenant-a --host host-a --instance vm-01 --agent agent-01 \
  --session session-42 --since 2026-08-01T12:00:00Z --limit 500
```

Narrow an investigation by any correlation identifier:

```sh
sandboxctl activity timeline \
  --tenant tenant-a --host host-a --instance vm-01 --agent agent-01 \
  --mission mission-7 --task task-9 --tool tool-11 --command command-13 \
  --process 8172 --event process.exec --outcome denied --trust observed
```

Inspect coverage without events:

```sh
sandboxctl activity coverage \
  --tenant tenant-a --host host-a --instance vm-01 --agent agent-01 --json
```

Create an audited signed export:

```sh
sandboxctl activity export \
  --tenant tenant-a --host host-a --instance vm-01 --agent agent-01 \
  --session session-42 \
  --output session-42.activity-export.json
```

The audit actor comes from the authenticated mTLS, Unix-peer, or bearer identity;
it is never accepted from a caller-supplied header. The export contains metadata
events and an HMAC-SHA256/Merkle manifest with its key ID. Anchor the manifest
root outside workload and collector control as
described in [activity governance](activity-governance.md).

## API and dashboard

- `GET /api/v2/activity/timeline` returns `events`, per-collector `coverage`, and
  the aggregate `completeness` assessment.
- `GET /api/v2/activity/coverage` returns coverage without event records.
- `POST /api/v2/activity/export` accepts the same filter object and returns the
  governed signed export.

The dashboard Activity tab asks for the exact scope, fetches coverage first,
then renders the timeline. Independently observed, attested, self-reported, and
derived records use distinct badges. Event and correlation values are inserted
with DOM `textContent`; terminal control bytes or forged log-like strings cannot
create markup or replace displayed metadata.

## Reading coverage

`complete=true` requires at least one collector and no sequence gaps, durable
loss records, drop counters, restarts, stale collectors, or unsupported event
classes. Clock uncertainty is always shown even when all loss checks pass. A
restart makes the conservative assessment incomplete until an operator confirms
continuity across the boundary.

For the #707 proof-of-concept workflow, correlate `session_id`, `mission_id`,
`task_id`, `tool_call_id`, `command_id`, `process_id`, and trace IDs; compare
self-reported action events with independently observed process/network/runtime
events; then record the coverage line with the investigation. Never omit the
coverage line from an incident report or signed evidence handoff.

## Verification

```sh
cd management
cargo test --lib activity
node --check ui/app.js

cd ../cli
cargo test
```
