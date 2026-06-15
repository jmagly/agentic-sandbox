#!/usr/bin/env bash
# Credential-aware Codex TUI launcher for orchestrator-observed PTY sessions.
set -euo pipefail

export TERM="${AGENTIC_CODEX_TERM:-xterm}"
export NO_COLOR="${NO_COLOR:-1}"

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"
openai_key_file="${OPENAI_API_KEY_FILE:-}"
if [[ -z "$openai_key_file" && -n "$credential_dir" && -f "$credential_dir/openai_api_key" ]]; then
  openai_key_file="$credential_dir/openai_api_key"
fi

if [[ -n "${AGENTIC_PROVIDER_HOME:-}" ]]; then
  mkdir -p "$AGENTIC_PROVIDER_HOME/home" "$AGENTIC_PROVIDER_HOME/config" "$AGENTIC_PROVIDER_HOME/cache"
  export HOME="$AGENTIC_PROVIDER_HOME/home"
  export XDG_CONFIG_HOME="$AGENTIC_PROVIDER_HOME/config"
  export XDG_CACHE_HOME="$AGENTIC_PROVIDER_HOME/cache"
fi

if [[ -n "${AGENTIC_CODEX_WORKDIR:-}" ]]; then
  cd "$AGENTIC_CODEX_WORKDIR"
fi

if [[ -n "$openai_key_file" ]]; then
  if [[ ! -f "$openai_key_file" ]]; then
    echo "agentic-codex-automation: OPENAI_API_KEY_FILE not found" >&2
    exit 78
  fi
  export OPENAI_API_KEY
  OPENAI_API_KEY="$(<"$openai_key_file")"
fi

exec codex --no-alt-screen "$@"
