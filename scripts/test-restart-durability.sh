#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MGMT_BIN="${MGMT_BIN:-${ROOT_DIR}/management/target/debug/agentic-mgmt}"
GRPC_PORT="${RESTART_DURABILITY_GRPC_PORT:-8130}"
HTTP_PORT=$((GRPC_PORT + 2))
MGMT_BASE="${RESTART_DURABILITY_MGMT_BASE:-http://127.0.0.1:${HTTP_PORT}}"
INSTANCE_ID="${CONFORMANCE_INSTANCE_ID:-00000000-0000-7000-8000-000000000001}"
EXECUTOR_URL="${MGMT_BASE}/agents/${INSTANCE_ID}"
TEST_TOKEN="${TEST_TOKEN:-restart-durability-test-token}"
REPORT_OUT="${RESTART_DURABILITY_REPORT_OUT:-${ROOT_DIR}/conformance.restart-durability.report.md}"
SERVER_LOG_OUT="${RESTART_DURABILITY_SERVER_LOG_OUT:-${ROOT_DIR}/conformance.restart-durability.server.log}"
RUN_ROOT="${RESTART_DURABILITY_RUN_ROOT:-$(mktemp -d /tmp/agentic-restart-durability.XXXXXX)}"
DATA_DIR="${RUN_ROOT}/data"
SECRETS_DIR="${DATA_DIR}/secrets"
SERVER_LOG="${RUN_ROOT}/server.log"

SERVER_PID=""
STARTED_AT_1=""
STOPPED_AT_1=""
STARTED_AT_2=""
FINISHED_AT=""
MESSAGE_ID="restart-durability-$(date -u +%Y%m%dT%H%M%SZ)-$$"
TASK_ID=""
REPLAYED_HEADER=""

redact() {
  python3 -c 'import sys; token=sys.argv[1]; data=sys.stdin.read(); print(data.replace(token, "<redacted-token>"), end="")' "${TEST_TOKEN}"
}

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
  if [[ -f "${SERVER_LOG}" ]]; then
    redact < "${SERVER_LOG}" > "${SERVER_LOG_OUT}" || true
  fi
  if [[ "${KEEP_RESTART_DURABILITY_RUN:-0}" != "1" ]]; then
    rm -rf "${RUN_ROOT}"
  fi
  exit "${rc}"
}
trap cleanup EXIT

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required tool missing: $1" >&2
    exit 1
  fi
}

json_get() {
  local file="$1"
  local expr="$2"
  python3 - "$file" "$expr" <<'PY'
import json
import sys

path, expr = sys.argv[1:3]
with open(path, "r", encoding="utf-8") as f:
    value = json.load(f)

for part in expr.split("."):
    if not part:
        continue
    value = value[part]

if isinstance(value, (dict, list)):
    print(json.dumps(value, separators=(",", ":")))
else:
    print(value)
PY
}

write_payload() {
  local out="$1"
  python3 - "$out" "$MESSAGE_ID" <<'PY'
import json
import sys

path, message_id = sys.argv[1:3]
payload = {
    "message": {
        "messageId": message_id,
        "role": "user",
        "parts": [
            {
                "kind": "text",
                "text": "restart durability conformance probe",
            }
        ],
        "metadata": {
            "scenario": "extensions/idempotency/restart_durability",
        },
    },
    "configuration": {
        "blocking": False,
        "acceptedOutputModes": ["application/json"],
    },
}
with open(path, "w", encoding="utf-8") as f:
    json.dump(payload, f, separators=(",", ":"), sort_keys=True)
PY
}

start_server() {
  local phase="$1"
  LISTEN_ADDR="127.0.0.1:${GRPC_PORT}" \
    SECRETS_DIR="${SECRETS_DIR}" \
    AIWG_CONFORMANCE_MODE=1 \
    RUST_LOG="${RUST_LOG:-info}" \
    "${MGMT_BIN}" >> "${SERVER_LOG}" 2>&1 &
  SERVER_PID=$!

  for i in $(seq 1 120); do
    if curl -fsS "${MGMT_BASE}/healthz" >/dev/null 2>&1; then
      echo "${phase} server healthy after ${i}s"
      return 0
    fi
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
      echo "${phase} server exited before becoming healthy" >&2
      tail -n 200 "${SERVER_LOG}" | redact >&2 || true
      exit 1
    fi
    sleep 1
  done

  echo "${phase} server did not become healthy in 120s" >&2
  tail -n 200 "${SERVER_LOG}" | redact >&2 || true
  exit 1
}

request_send() {
  local body_file="$1"
  local headers_file="$2"
  local response_file="$3"
  local code
  code="$(curl -sS \
    -o "${response_file}" \
    -D "${headers_file}" \
    -w '%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${TEST_TOKEN}" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json" \
    -H "A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/idempotency/v1, https://agentic-sandbox.aiwg.io/extensions/runtime/v1" \
    --data-binary "@${body_file}" \
    "${EXECUTOR_URL}/v1/messages:send")"
  if [[ "${code}" != "202" ]]; then
    echo "messages:send returned HTTP ${code}" >&2
    cat "${response_file}" >&2
    exit 1
  fi
}

write_report() {
  local status="$1"
  local commit
  commit="$(git -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null || echo unknown)"
  FINISHED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  cat > "${REPORT_OUT}" <<EOF
# Restart Durability Conformance Report

- status: ${status}
- management_commit: ${commit}
- management_binary: ${MGMT_BIN}
- data_path: ${DATA_DIR}
- data_cleaned_after_run: $([[ "${KEEP_RESTART_DURABILITY_RUN:-0}" == "1" ]] && echo "false" || echo "true")
- instance_id: ${INSTANCE_ID}
- task_id: ${TASK_ID}
- idempotency_message_id: ${MESSAGE_ID}
- first_start_utc: ${STARTED_AT_1}
- first_stop_utc: ${STOPPED_AT_1}
- second_start_utc: ${STARTED_AT_2}
- finished_utc: ${FINISHED_AT}
- replay_header: ${REPLAYED_HEADER}
- server_log: ${SERVER_LOG_OUT}

## Assertions

- isolated temporary data directory was created under ${RUN_ROOT}
- management wrote durable state to ${DATA_DIR}/missions.db
- replay after clean restart returned the original task id
- replay after clean restart returned Idempotent-Replayed: true
- task state was readable after restart
- bearer token values were redacted from exported logs
EOF
}

require_tool curl
require_tool python3

if [[ ! -x "${MGMT_BIN}" ]]; then
  echo "management binary not executable: ${MGMT_BIN}" >&2
  exit 1
fi

fuser -k "${HTTP_PORT}/tcp" 2>/dev/null || true
fuser -k "${GRPC_PORT}/tcp" 2>/dev/null || true
sleep 1

mkdir -p "${SECRETS_DIR}"
chmod 700 "${DATA_DIR}" "${SECRETS_DIR}"
printf '%s' "${TEST_TOKEN}" > "${SECRETS_DIR}/admin.token"
chmod 600 "${SECRETS_DIR}/admin.token"
cat > "${SECRETS_DIR}/operator-tokens.toml" <<EOF
[[tokens]]
token = "${TEST_TOKEN}"
role = "admin"
EOF
chmod 600 "${SECRETS_DIR}/operator-tokens.toml"

PAYLOAD="${RUN_ROOT}/message-send.json"
HEADERS_1="${RUN_ROOT}/headers-1.txt"
BODY_1="${RUN_ROOT}/body-1.json"
HEADERS_2="${RUN_ROOT}/headers-2.txt"
BODY_2="${RUN_ROOT}/body-2.json"
TASK_BODY="${RUN_ROOT}/task-after-restart.json"
write_payload "${PAYLOAD}"

STARTED_AT_1="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
start_server "first"
curl -fsS -H "Authorization: Bearer ${TEST_TOKEN}" \
  "${EXECUTOR_URL}/.well-known/agent-card.json" >/dev/null
request_send "${PAYLOAD}" "${HEADERS_1}" "${BODY_1}"
TASK_ID="$(json_get "${BODY_1}" "id")"

STOPPED_AT_1="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cleanup_server

if [[ ! -s "${DATA_DIR}/missions.db" ]]; then
  echo "durable TaskStore database was not created at ${DATA_DIR}/missions.db" >&2
  exit 1
fi

STARTED_AT_2="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
start_server "second"
request_send "${PAYLOAD}" "${HEADERS_2}" "${BODY_2}"
TASK_ID_2="$(json_get "${BODY_2}" "id")"
if [[ "${TASK_ID_2}" != "${TASK_ID}" ]]; then
  echo "restart replay returned a different task id: ${TASK_ID_2} != ${TASK_ID}" >&2
  exit 1
fi

REPLAYED_HEADER="$(awk 'BEGIN{IGNORECASE=1} /^Idempotent-Replayed:/ {gsub(/\r/,""); print $2}' "${HEADERS_2}" | tail -1)"
if [[ "${REPLAYED_HEADER}" != "true" ]]; then
  echo "restart replay did not return Idempotent-Replayed: true" >&2
  exit 1
fi

curl -fsS \
  -H "Authorization: Bearer ${TEST_TOKEN}" \
  "${EXECUTOR_URL}/v1/tasks/${TASK_ID}" > "${TASK_BODY}"
TASK_STATE="$(json_get "${TASK_BODY}" "status.state")"
if [[ -z "${TASK_STATE}" ]]; then
  echo "task state was empty after restart" >&2
  exit 1
fi

write_report "pass"
echo "restart durability passed: task_id=${TASK_ID} message_id=${MESSAGE_ID} report=${REPORT_OUT}"
