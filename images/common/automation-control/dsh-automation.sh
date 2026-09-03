#!/usr/bin/env bash
# Credential-aware DSH launcher for orchestrator-observed PTY sessions.
set -euo pipefail

export TERM="${AGENTIC_DSH_TERM:-xterm}"
export NO_COLOR="${NO_COLOR:-1}"

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"
openrouter_key_file="${OPENROUTER_API_KEY_FILE:-}"
if [[ -z "$openrouter_key_file" && -n "$credential_dir" && -f "$credential_dir/openrouter_api_key" ]]; then
  openrouter_key_file="$credential_dir/openrouter_api_key"
fi

if [[ -n "${AGENTIC_PROVIDER_HOME:-}" ]]; then
  mkdir -p "${AGENTIC_PROVIDER_HOME}/home" "${AGENTIC_PROVIDER_HOME}/config" "${AGENTIC_PROVIDER_HOME}/cache"
  export HOME="${AGENTIC_PROVIDER_HOME}/home"
  export XDG_CONFIG_HOME="${AGENTIC_PROVIDER_HOME}/config"
  export XDG_CACHE_HOME="${AGENTIC_PROVIDER_HOME}/cache"
fi

if [[ -n "${AGENTIC_DSH_WORKDIR:-}" ]]; then
  cd "${AGENTIC_DSH_WORKDIR}"
fi

if [[ -n "$openrouter_key_file" ]]; then
  if [[ ! -f "$openrouter_key_file" ]]; then
    echo "agentic-dsh-automation: OPENROUTER_API_KEY_FILE not found" >&2
    exit 78
  fi
  export OPENROUTER_API_KEY
  OPENROUTER_API_KEY="$(<"$openrouter_key_file")"
fi

# Seed a writable provider home without ever copying credentials into it.
# DSH auto-initializes profiles and persists sessions below DSH_HOME, so the
# read-only image seed itself must never be used as the live home.
dsh_home="${AGENTIC_DSH_HOME:-${HOME}/.dsh}"
seed_home="${AGENTIC_DSH_SEED_HOME:-/usr/share/agentic-sandbox/dsh-seed}"
mkdir -p "$dsh_home"
if [[ ! -e "$dsh_home/settings.yaml" && -f "$seed_home/settings.yaml" ]]; then
  install -m 0600 "$seed_home/settings.yaml" "$dsh_home/settings.yaml"
fi
export DSH_HOME="$dsh_home"

# The dsh launcher hands everything after its own flags to the booted profile
# tree verbatim (apps/cli/src/args.ts): interactive attach is
#   dsh --profile <name>
# and headless/one-shot invocations pass the profile's inner flags through.
exec dsh "$@"
