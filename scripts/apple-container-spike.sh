#!/usr/bin/env bash
# Run the Apple container provider feasibility spike on an Apple Silicon host.
#
# This script is intentionally transcript-oriented: #488 needs exact host,
# container, command, and observed-result evidence before #489 can start.

set -euo pipefail

OUT="${OUT:-apple-container-spike-transcript.md}"
IMAGE="${AGENTIC_SPIKE_IMAGE:-docker.io/library/alpine:latest}"
CONTAINER_NAME="${AGENTIC_SPIKE_NAME:-agentic-apple-container-spike}"
WORKSPACE="${AGENTIC_SPIKE_WORKSPACE:-/tmp/agentic-apple-container-workspace}"

if [[ $# -gt 0 ]]; then
    OUT="$1"
fi

mkdir -p "$(dirname "${OUT}")"
: > "${OUT}"

append() {
    printf '%s\n' "$*" >> "${OUT}"
}

run_capture() {
    local title="$1"
    shift

    append
    append "### ${title}"
    append
    append '```bash'
    printf '$' >> "${OUT}"
    printf ' %q' "$@" >> "${OUT}"
    printf '\n' >> "${OUT}"
    append '```'
    append
    append '```text'
    set +e
    "$@" >> "${OUT}" 2>&1
    local status=$?
    set -e
    append '```'
    append
    append "Exit status: ${status}"
    return "${status}"
}

run_shell() {
    local title="$1"
    local command="$2"

    append
    append "### ${title}"
    append
    append '```bash'
    append "$ ${command}"
    append '```'
    append
    append '```text'
    set +e
    bash -lc "${command}" >> "${OUT}" 2>&1
    local status=$?
    set -e
    append '```'
    append
    append "Exit status: ${status}"
    return "${status}"
}

append "# Apple container feasibility spike transcript"
append
append "- Issues: #438, #488, #489"
append "- Generated: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
append "- Host: $(hostname 2>/dev/null || echo unknown)"
append "- Image under test: ${IMAGE}"

run_capture "macOS version" sw_vers || true
run_capture "Kernel and architecture" uname -a || true
run_shell "Hardware summary" "system_profiler SPHardwareDataType | sed -n '1,40p' | grep -Ev 'Serial Number|Hardware UUID|Provisioning UDID'" || true

if ! command -v container >/dev/null 2>&1; then
    append
    append "## Recommendation"
    append
    append "**Defer:** Apple \`container\` is not installed or not on PATH on this host."
    append
    append "Provider implementation (#489) must remain blocked until #488 can run with Apple \`container\` installed."
    exit 0
fi

run_capture "container version" container --version || true
run_capture "container help" container --help || true
run_capture "container system status before start" container system status || true
run_capture "container system start" container system start || true
run_capture "container system status after start" container system status || true
run_capture "container image list before pull" container image list || true

run_capture "Pull image under test" container image pull "${IMAGE}" || true
run_capture "Image list after pull" container image list || true

run_capture "Cleanup stale spike container before run" container delete "${CONTAINER_NAME}" || true
run_capture "Create/start lifecycle probe" container run --name "${CONTAINER_NAME}" --rm "${IMAGE}" uname -a || true
run_capture "Container list after lifecycle probe" container list --all || true

rm -rf "${WORKSPACE}"
mkdir -p "${WORKSPACE}"
printf '%s\n' "apple container workspace probe" > "${WORKSPACE}/probe.txt"

run_capture "Workspace help discovery" container run --help || true
run_shell "Workspace transfer fallback with container cp if available" "set -euo pipefail
container run --name ${CONTAINER_NAME}-workspace ${IMAGE} sh -lc 'sleep 30' &
pid=\$!
sleep 3
container cp ${WORKSPACE}/probe.txt ${CONTAINER_NAME}-workspace:/tmp/probe.txt
container exec ${CONTAINER_NAME}-workspace sh -lc 'cat /tmp/probe.txt && echo guest-output > /tmp/agent-created.txt'
container cp ${CONTAINER_NAME}-workspace:/tmp/agent-created.txt ${WORKSPACE}/agent-created.txt
cat ${WORKSPACE}/agent-created.txt
container stop ${CONTAINER_NAME}-workspace || true
wait \$pid || true
container delete ${CONTAINER_NAME}-workspace || true" || true

run_capture "Final stale runtime cleanup" container delete "${CONTAINER_NAME}" || true
run_capture "Final container list" container list --all || true

append
append "## Provider Contract Gap Checklist"
append
append "| Capability | Result | Notes |"
append "|------------|--------|-------|"
append "| create/start by deterministic name | observed | See lifecycle probe output. |"
append "| stop/destroy | observed | See cleanup commands. |"
append "| state query | observed | See container list output. |"
append "| IP/endpoint discovery | pending | Requires management-connectivity follow-up. |"
append "| logs | partial | Command output is captured; persistent log API still needs provider mapping. |"
append "| exec/attach strategy | partial | Workspace fallback tries container exec if available. |"
append "| workspace/agentshare setup | partial | container cp fallback tested; mount semantics still need provider decision. |"
append "| image pull/build | partial | Pull tested for image under test; repo agent image build remains separate. |"
append "| resource limits | pending | Needs explicit CPU/memory flag validation. |"
append "| stale runtime cleanup | observed | See final cleanup/list output. |"
append "| bootstrap enrollment | pending | Requires agent image and management endpoint. |"
append "| secure transport without plaintext non-loopback | pending | Requires management endpoint reachable from guest without unsafe plaintext exposure. |"
append "| credential-aware startup helpers | pending | Requires agent image/loadout validation. |"

append
append "## Recommendation"
append
append "**Proceed with gaps** only if the lifecycle/workspace commands above succeeded. Otherwise defer #489 and convert the concrete failed commands into follow-up work."
