#!/usr/bin/env bash
# Credential-aware Claude launcher for managed observed sessions.
set -euo pipefail

export TERM="${AGENTIC_CLAUDE_TERM:-xterm}"
export NO_COLOR="${NO_COLOR:-1}"

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"
anthropic_key_file="${ANTHROPIC_API_KEY_FILE:-}"
if [[ -z "$anthropic_key_file" && -n "$credential_dir" && -f "$credential_dir/anthropic_api_key" ]]; then
  anthropic_key_file="$credential_dir/anthropic_api_key"
fi

if [[ -n "${AGENTIC_PROVIDER_HOME:-}" ]]; then
  mkdir -p "$AGENTIC_PROVIDER_HOME/home" "$AGENTIC_PROVIDER_HOME/config" "$AGENTIC_PROVIDER_HOME/cache"
  export HOME="$AGENTIC_PROVIDER_HOME/home"
  export XDG_CONFIG_HOME="$AGENTIC_PROVIDER_HOME/config"
  export XDG_CACHE_HOME="$AGENTIC_PROVIDER_HOME/cache"
fi

if [[ -n "${AGENTIC_CLAUDE_WORKDIR:-}" ]]; then
  cd "$AGENTIC_CLAUDE_WORKDIR"
fi

if [[ -n "$anthropic_key_file" ]]; then
  if [[ ! -f "$anthropic_key_file" ]]; then
    echo "agentic-claude-automation: ANTHROPIC_API_KEY_FILE not found" >&2
    exit 78
  fi
  export ANTHROPIC_API_KEY
  ANTHROPIC_API_KEY="$(<"$anthropic_key_file")"
fi

if [[ "${AGENTIC_CLAUDE_MODE:-tui}" == "print" ]]; then
  exec claude --print "$@"
fi

exec claude "$@"
