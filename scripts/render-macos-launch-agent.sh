#!/usr/bin/env bash
# Render the opt-in per-user host-runtime LaunchAgent for a package or dev build.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
template="$root/deploy/launchd/io.aiwg.agentic-sandbox.host-runtime.plist"
output=""
daemon_binary="/usr/local/bin/agentic-host-runtime-daemon"
agent_binary="/usr/local/bin/agent-client"

usage() {
  cat <<'EOF'
Usage: scripts/render-macos-launch-agent.sh --output <plist> [options]

Options:
  --daemon-binary <path>  Absolute host-runtime daemon path
  --agent-binary <path>   Absolute agent-client path
  --output <path>         Destination plist

This command only renders a plist. It never installs, bootstraps, enables, or
starts the LaunchAgent. Host runtime grants the daemon user full host access.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --daemon-binary) daemon_binary="${2:-}"; shift 2 ;;
    --agent-binary) agent_binary="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$output" ]] || { echo "missing --output" >&2; exit 2; }
[[ "$daemon_binary" == /* ]] || { echo "--daemon-binary must be absolute" >&2; exit 2; }
[[ "$agent_binary" == /* ]] || { echo "--agent-binary must be absolute" >&2; exit 2; }
[[ -n "${HOME:-}" ]] || { echo "HOME is required to render per-user launchd paths" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "required command not found: jq" >&2; exit 1; }
command -v plutil >/dev/null 2>&1 || { echo "required command not found: plutil" >&2; exit 1; }

launchd_temp_dir="${TMPDIR:-/tmp}"
socket_base="${launchd_temp_dir%/}"
runtime_socket="$socket_base/io.aiwg.agentic-sandbox/host-runtime.sock"
application_support="$HOME/Library/Application Support/io.aiwg.agentic-sandbox"
(( ${#runtime_socket} < 104 )) || {
  echo "rendered socket path exceeds Darwin sockaddr_un.sun_path" >&2
  exit 1
}

mkdir -p "$(dirname "$output")"
install -m 0644 "$template" "$output"
program_arguments="$(
  jq -cn \
    --arg daemon "$daemon_binary" \
    --arg agent "$agent_binary" \
    '[$daemon, "--agent-client", $agent]'
)"
plutil -replace ProgramArguments -json "$program_arguments" "$output"
plutil -insert EnvironmentVariables.HOME -string "$HOME" "$output"
plutil -insert EnvironmentVariables.TMPDIR -string "$launchd_temp_dir" "$output"
plutil -insert EnvironmentVariables.AGENTIC_HOST_RUNTIME_DAEMON_SOCKET \
  -string "$runtime_socket" "$output"
plutil -insert EnvironmentVariables.AGENTIC_HOST_RUNTIME_ROOT \
  -string "$application_support/host-runtime" "$output"
plutil -insert EnvironmentVariables.AGENTIC_HOST_WORKSPACE_ROOT \
  -string "$application_support/workspace" "$output"
plutil -lint "$output" >/dev/null
printf 'rendered=%s\n' "$output"
