#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

cat >"$TMPDIR/gateway-ssh-fixture.json" <<'JSON'
{
  "rows": [
    {
      "profile": "local",
      "fanout": 1,
      "startup_to_prompt_ms": 91.5,
      "attach_reattach_ms": 90.75,
      "keystroke_rtt_ms": 3.4,
      "bytes_burst_output": 424242,
      "notes": "fixture-backed gateway SSH measurement"
    }
  ]
}
JSON

python3 "$ROOT/scripts/benchmark-terminal-transports.py" \
  --out-dir "$TMPDIR/out" \
  --prefix terminal-transport-benchmark-fixture \
  --gateway-ssh-fixture "$TMPDIR/gateway-ssh-fixture.json" >/dev/null

python3 - "$TMPDIR/out/terminal-transport-benchmark-fixture.json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as fh:
    artifact = json.load(fh)

rows = [
    row for row in artifact["rows"]
    if row["transport"] == "gateway-ssh"
    and row["profile"] == "local"
    and row["fanout"] == 1
]
assert len(rows) == 1, rows
row = rows[0]
assert row["measured"] is True, row
assert row["startup_to_prompt_ms"] == 91.5, row
assert row["bytes_burst_output"] == 424242, row
assert artifact["summary"]["gateway_ssh_measured_rows"] == 1, artifact["summary"]
PY
