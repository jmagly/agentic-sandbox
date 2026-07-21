#!/bin/sh
set -eu

printf '%s\n' "$*" >> "${FAKE_LIBVIRT_CHECKPOINT_LOG:?FAKE_LIBVIRT_CHECKPOINT_LOG required}"
if [ "${LIBVIRT_BOOTSTRAP_STDIN:-0}" = "1" ]; then
    if IFS= read -r payload && printf '%s' "$payload" | jq -e '.single.token and .single.spiffe_id and .single.ca_pem' >/dev/null; then
        printf 'bootstrap_stdin=true\n' >> "$FAKE_LIBVIRT_CHECKPOINT_LOG"
    else
        printf 'bootstrap_stdin=false\n' >> "$FAKE_LIBVIRT_CHECKPOINT_LOG"
        exit 2
    fi
fi
if env | grep -q '^AGENT_BOOTSTRAP_TOKEN='; then
    printf 'bootstrap_secret_env_present=true\n' >> "$FAKE_LIBVIRT_CHECKPOINT_LOG"
else
    printf 'bootstrap_secret_env_present=false\n' >> "$FAKE_LIBVIRT_CHECKPOINT_LOG"
fi

case "${1:-}" in
    checkpoint)
        printf '%s\n' '{"checkpoint_id":"qemu-clean","pre_enrollment":true,"duration_ms":3200}'
        ;;
    restore-checkpoint)
        printf '%s\n' '{"name":"qemu-base","enroll_on_restore":true,"bootstrap_token_issued":true,"bootstrap_spiffe_id":"spiffe://sandbox.agentic.local/agent/test","enrollment_ready":true,"duration_ms":2800}'
        ;;
    warm-init)
        printf '%s\n' '{"pool":"qemu-pool","size":2,"available":2,"prebooted":true}'
        ;;
    warm-handoff)
        printf '%s\n' '{"name":"qemu-clean-a","pool":"qemu-pool","claimed_slot":"slot-1","enroll_on_restore":true,"bootstrap_token_issued":true,"bootstrap_spiffe_id":"spiffe://sandbox.agentic.local/agent/test","enrollment_ready":true,"duration_ms":120}'
        ;;
    *)
        printf '%s\n' '{"ok":true}'
        ;;
esac
