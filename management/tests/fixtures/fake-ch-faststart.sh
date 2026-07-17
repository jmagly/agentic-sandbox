#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "${FAKE_CH_FASTSTART_LOG:?FAKE_CH_FASTSTART_LOG required}"
if [[ -n "${AGENT_BOOTSTRAP_TOKEN:-}" || -n "${CH_CHILD_BOOTSTRAP_ENVELOPES:-}" ]]; then
  printf 'bootstrap_secret_env_present=true\n' >> "$FAKE_CH_FASTSTART_LOG"
else
  printf 'bootstrap_secret_env_present=false\n' >> "$FAKE_CH_FASTSTART_LOG"
fi

bootstrap_payload=""
if [[ "${CH_BOOTSTRAP_STDIN:-}" == "1" ]]; then
  IFS= read -r bootstrap_payload
  printf 'bootstrap_stdin=true\n' >> "$FAKE_CH_FASTSTART_LOG"
fi

case "${1:-}" in
  snapshot)
    printf '{"snapshot_id":"%s","snapshot_dir":"/tmp/fake-ch-snapshot","pre_enrollment":true,"posture":"clean-base"}\n' "${5:-fake-snapshot}"
    ;;
  restore)
    bootstrap_issued=false
    bootstrap_spiffe=""
    if [[ -n "$bootstrap_payload" ]] && [[ "$(printf '%s' "$bootstrap_payload" | jq -r '.single.token // empty')" != "" ]]; then
      bootstrap_issued=true
      bootstrap_spiffe="$(printf '%s' "$bootstrap_payload" | jq -r '.single.spiffe_id // empty')"
    fi
    printf '{"name":"%s","snapshot_id":"%s","vsock_cid":3,"duration_ms":17,"enroll_on_restore":true,"bootstrap_token_issued":%s,"bootstrap_spiffe_id":"%s"}\n' "${5:-fake-child}" "${3:-fake-snapshot}" "$bootstrap_issued" "$bootstrap_spiffe"
    ;;
  fork)
    bootstrap_issued=false
    if [[ -n "$bootstrap_payload" ]] && [[ "$(printf '%s' "$bootstrap_payload" | jq -r '.children | length')" -gt 0 ]]; then
      bootstrap_issued=true
    fi
    printf '{"manifest":"/tmp/fake-fork.json","children":[{"name":"fake-child-1","vsock_cid":3}],"bootstrap_token_issued":%s}\n' "$bootstrap_issued"
    ;;
  warm-init)
    printf '{"pool":"%s","snapshot_id":"%s","target_size":2,"idle":2}\n' "${7:-fake-pool}" "${3:-fake-snapshot}"
    ;;
  warm-handoff)
    bootstrap_issued=false
    bootstrap_spiffe=""
    if [[ -n "$bootstrap_payload" ]] && [[ "$(printf '%s' "$bootstrap_payload" | jq -r '.single.token // empty')" != "" ]]; then
      bootstrap_issued=true
      bootstrap_spiffe="$(printf '%s' "$bootstrap_payload" | jq -r '.single.spiffe_id // empty')"
    fi
    printf '{"name":"%s","snapshot_id":"fake-snapshot","vsock_cid":3,"duration_ms":17,"enroll_on_restore":true,"bootstrap_token_issued":%s,"bootstrap_spiffe_id":"%s"}\n' "${5:-fake-child}" "$bootstrap_issued" "$bootstrap_spiffe"
    ;;
  *)
    echo "unexpected fake ch-faststart subcommand: ${1:-}" >&2
    exit 2
    ;;
esac
