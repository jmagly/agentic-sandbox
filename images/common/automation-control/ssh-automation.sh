#!/usr/bin/env bash
# Credential-aware SSH launcher for managed observed sessions.
set -euo pipefail

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"
ssh_key_file="${SSH_PRIVATE_KEY_FILE:-}"
if [[ -z "$ssh_key_file" && -n "$credential_dir" && -f "$credential_dir/ssh_private_key" ]]; then
  ssh_key_file="$credential_dir/ssh_private_key"
fi

known_hosts_file="${SSH_KNOWN_HOSTS_FILE:-}"
if [[ -z "$known_hosts_file" && -n "$credential_dir" && -f "$credential_dir/ssh_known_hosts" ]]; then
  known_hosts_file="$credential_dir/ssh_known_hosts"
fi

if [[ -n "${AGENTIC_PROVIDER_HOME:-}" ]]; then
  mkdir -p "$AGENTIC_PROVIDER_HOME/home" "$AGENTIC_PROVIDER_HOME/config" "$AGENTIC_PROVIDER_HOME/cache"
  export HOME="$AGENTIC_PROVIDER_HOME/home"
  export XDG_CONFIG_HOME="$AGENTIC_PROVIDER_HOME/config"
  export XDG_CACHE_HOME="$AGENTIC_PROVIDER_HOME/cache"
fi

if [[ -n "${AGENTIC_SSH_WORKDIR:-}" ]]; then
  cd "$AGENTIC_SSH_WORKDIR"
fi

if [[ -n "$ssh_key_file" ]]; then
  if [[ ! -f "$ssh_key_file" ]]; then
    echo "agentic-ssh-automation: SSH_PRIVATE_KEY_FILE not found" >&2
    exit 78
  fi
  chmod 0600 "$ssh_key_file" 2>/dev/null || true
  git_ssh_command=(ssh -i "$ssh_key_file" -o IdentitiesOnly=yes)
  if [[ -n "$known_hosts_file" ]]; then
    if [[ ! -f "$known_hosts_file" ]]; then
      echo "agentic-ssh-automation: SSH_KNOWN_HOSTS_FILE not found" >&2
      exit 78
    fi
    git_ssh_command+=(-o UserKnownHostsFile="$known_hosts_file" -o StrictHostKeyChecking=yes)
  else
    git_ssh_command+=(-o StrictHostKeyChecking=accept-new)
  fi
  printf -v GIT_SSH_COMMAND '%q ' "${git_ssh_command[@]}"
  export GIT_SSH_COMMAND="${GIT_SSH_COMMAND% }"
fi

exec ssh "$@"
