#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "${FAKE_CH_FASTSTART_LOG:?FAKE_CH_FASTSTART_LOG required}"

case "${1:-}" in
  snapshot)
    printf '{"snapshot_id":"%s","snapshot_dir":"/tmp/fake-ch-snapshot","pre_enrollment":true,"posture":"clean-base"}\n' "${5:-fake-snapshot}"
    ;;
  restore)
    printf '{"name":"%s","snapshot_id":"%s","vsock_cid":3,"duration_ms":17,"enroll_on_restore":true}\n' "${5:-fake-child}" "${3:-fake-snapshot}"
    ;;
  fork)
    printf '{"manifest":"/tmp/fake-fork.json","children":[{"name":"fake-child-1","vsock_cid":3}]}\n'
    ;;
  warm-init)
    printf '{"pool":"%s","snapshot_id":"%s","target_size":2,"idle":2}\n' "${7:-fake-pool}" "${3:-fake-snapshot}"
    ;;
  warm-handoff)
    printf '{"name":"%s","snapshot_id":"fake-snapshot","vsock_cid":3,"duration_ms":17,"enroll_on_restore":true}\n' "${5:-fake-child}"
    ;;
  *)
    echo "unexpected fake ch-faststart subcommand: ${1:-}" >&2
    exit 2
    ;;
esac
