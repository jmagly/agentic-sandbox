#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MGMT_BIN="${MGMT_BIN:-${ROOT_DIR}/management/target/debug/agentic-mgmt}"
GRPC_PORT="${FLEET_LIVE_GRPC_PORT:-19340}"
HTTP_PORT=$((GRPC_PORT + 2))
BASE_URL="http://127.0.0.1:${HTTP_PORT}"
INSTANCE_ID="${CONFORMANCE_INSTANCE_ID:-00000000-0000-7000-8000-000000000001}"
TOKEN="${FLEET_LIVE_TOKEN:-fleet-live-test-token}"
RUN_ROOT="${FLEET_LIVE_RUN_ROOT:-$(mktemp -d /tmp/agentic-fleet-live.XXXXXX)}"
SECRETS_DIR="${RUN_ROOT}/data/secrets"
SERVER_LOG="${RUN_ROOT}/server.log"
SERVER_PID=""

cleanup_server() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  SERVER_PID=""
}

cleanup() {
  local rc=$?
  cleanup_server
  if [[ "${KEEP_FLEET_LIVE_RUN:-0}" != "1" ]]; then
    rm -rf "${RUN_ROOT}"
  fi
  exit "${rc}"
}
trap cleanup EXIT

json_get() {
  python3 -c 'import json,sys
value=json.load(sys.stdin)
for part in sys.argv[1].split("."):
    value=value[int(part)] if isinstance(value,list) else value[part]
print(json.dumps(value) if isinstance(value,(dict,list)) else str(value).lower() if isinstance(value,bool) else value)' "$1"
}

start_server() {
  LISTEN_ADDR="127.0.0.1:${GRPC_PORT}" \
    SECRETS_DIR="${SECRETS_DIR}" \
    AIWG_CONFORMANCE_MODE=1 \
    RUST_LOG="${RUST_LOG:-info}" \
    "${MGMT_BIN}" >>"${SERVER_LOG}" 2>&1 &
  SERVER_PID=$!
  for i in $(seq 1 120); do
    if curl -fsS "${BASE_URL}/healthz/http" >/dev/null 2>&1; then
      echo "fleet live server healthy after ${i}s"
      return
    fi
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
      tail -n 120 "${SERVER_LOG}" >&2 || true
      exit 1
    fi
    sleep 1
  done
  echo "fleet live server did not become healthy" >&2
  exit 1
}

auth_curl() {
  curl -fsS -H "Authorization: Bearer ${TOKEN}" "$@"
}

if [[ ! -x "${MGMT_BIN}" ]]; then
  echo "management binary not executable: ${MGMT_BIN}" >&2
  exit 1
fi

mkdir -p "${SECRETS_DIR}"
chmod 700 "${RUN_ROOT}/data" "${SECRETS_DIR}"
printf '%s' "${TOKEN}" >"${SECRETS_DIR}/admin.token"
chmod 600 "${SECRETS_DIR}/admin.token"
printf '[[tokens]]\ntoken = "%s"\nrole = "admin"\n' "${TOKEN}" >"${SECRETS_DIR}/operator-tokens.toml"
chmod 600 "${SECRETS_DIR}/operator-tokens.toml"

RECORD_1="${RUN_ROOT}/workload-1.json"
RECORD_2="${RUN_ROOT}/workload-retry.json"
python3 - "${RECORD_1}" "${RECORD_2}" <<'PY'
import json,sys
base={
  "document_type":"workload","api_version":"agentic-orchestration/v1","kind":"persistent-agent",
  "lineage":{"orchestrator_id":"non-aiwg-live-proof","mission_id":"mission-live","dispatch_id":"dispatch-live","idempotency_key":"idem-live","parent_id":"mission-live","child_id":"child-live","target_id":"00000000-0000-7000-8000-000000000001","executor_id":"sandbox-live","runtime_id":"runtime-host","session_id":None,"task_id":None,"command_id":None},
  "spec":{"desired_state":"running","capabilities":[],"policy":{"trust_tier":"T1","isolation_kind":"host"},"budgets":{"max_attempts":2,"timeout_seconds":300}},
  "status":{"observed_state":"pending","revision":0,"last_seen":"2026-08-02T16:00:00Z","artifacts":[]}
}
for path,seen in zip(sys.argv[1:],["2026-08-02T16:00:00Z","2026-08-02T16:05:00Z"]):
  value=json.loads(json.dumps(base)); value["status"]["last_seen"]=seen
  with open(path,"w",encoding="utf-8") as f: json.dump(value,f,separators=(",",":"),sort_keys=True)
PY

start_server

ADMISSION="$(auth_curl -X POST -H 'Content-Type: application/json' --data-binary "@${RECORD_1}" "${BASE_URL}/api/v2/fleet/workloads")"
[[ "$(printf '%s' "${ADMISSION}" | json_get workload.status.observed_state)" == "admitted" ]]

MESSAGE_BODY="${RUN_ROOT}/message.json"
python3 - "${MESSAGE_BODY}" <<'PY'
import json,sys
with open(sys.argv[1],"w",encoding="utf-8") as f:
  json.dump({"message":{"messageId":"dispatch-live","role":"user","parts":[{"kind":"text","text":"fleet live persistent task"}]}},f,separators=(",",":"),sort_keys=True)
PY
TASK="$(auth_curl -X POST -H 'Content-Type: application/json' -H 'A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1, https://agentic-sandbox.aiwg.io/extensions/idempotency/v1' --data-binary "@${MESSAGE_BODY}" "${BASE_URL}/agents/${INSTANCE_ID}/v1/messages:send")"
TASK_ID="$(printf '%s' "${TASK}" | json_get id)"

BINDING="${RUN_ROOT}/binding.json"
python3 - "${BINDING}" "${TASK_ID}" <<'PY'
import json,sys
with open(sys.argv[1],"w",encoding="utf-8") as f:
  json.dump({"expected_revision":1,"runtime_identity":{"task_id":sys.argv[2]},"status":{"observed_state":"running","revision":2,"last_seen":"2026-08-02T16:01:00Z","artifacts":[]}},f,separators=(",",":"),sort_keys=True)
PY
BOUND="$(auth_curl -X POST -H 'Content-Type: application/json' --data-binary "@${BINDING}" "${BASE_URL}/api/v2/fleet/workloads/child-live/observations")"
[[ "$(printf '%s' "${BOUND}" | json_get lineage.task_id)" == "${TASK_ID}" ]]
[[ "$(printf '%s' "${BOUND}" | json_get status.observed_state)" == "running" ]]

cleanup_server
start_server

REPLAY="$(auth_curl -X POST -H 'Content-Type: application/json' --data-binary "@${RECORD_2}" "${BASE_URL}/api/v2/fleet/workloads")"
[[ "$(printf '%s' "${REPLAY}" | json_get replayed)" == "true" ]]
[[ "$(printf '%s' "${REPLAY}" | json_get workload.lineage.task_id)" == "${TASK_ID}" ]]

INVENTORY="$(auth_curl "${BASE_URL}/api/v2/fleet/workloads")"
[[ "$(printf '%s' "${INVENTORY}" | json_get records.0.lineage.task_id)" == "${TASK_ID}" ]]
[[ "$(printf '%s' "${INVENTORY}" | json_get records.0.status.observed_state)" == "running" ]]
INVENTORY_REVISION="$(printf '%s' "${INVENTORY}" | json_get inventory_revision)"

RECONCILE_BODY="${RUN_ROOT}/reconcile.json"
python3 - "${RECONCILE_BODY}" "${INVENTORY_REVISION}" <<'PY'
import json,sys
with open(sys.argv[1],"w",encoding="utf-8") as f:
  json.dump({"before_revision":int(sys.argv[2]),"child_ids":["child-live"]},f,separators=(",",":"),sort_keys=True)
PY
RECONCILE="$(auth_curl -X POST -H 'Content-Type: application/json' --data-binary "@${RECONCILE_BODY}" "${BASE_URL}/api/v2/fleet/reconcile")"
[[ "$(printf '%s' "${RECONCILE}" | json_get rows.0.classification)" == "re-adopted" ]]

echo "fleet live proof passed: child=child-live task=${TASK_ID} state=running replay=true reconciliation=re-adopted"
