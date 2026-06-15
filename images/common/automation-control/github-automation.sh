#!/usr/bin/env bash
# Credential-aware GitHub CLI launcher for managed observed sessions.
set -euo pipefail

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"
github_token_file="${GITHUB_TOKEN_FILE:-${GH_TOKEN_FILE:-}}"
if [[ -z "$github_token_file" && -n "$credential_dir" && -f "$credential_dir/github_token" ]]; then
  github_token_file="$credential_dir/github_token"
fi

if [[ -n "${AGENTIC_PROVIDER_HOME:-}" ]]; then
  mkdir -p "$AGENTIC_PROVIDER_HOME/home" "$AGENTIC_PROVIDER_HOME/config" "$AGENTIC_PROVIDER_HOME/cache"
  export HOME="$AGENTIC_PROVIDER_HOME/home"
  export XDG_CONFIG_HOME="$AGENTIC_PROVIDER_HOME/config"
  export XDG_CACHE_HOME="$AGENTIC_PROVIDER_HOME/cache"
fi

if [[ -n "${AGENTIC_GITHUB_WORKDIR:-}" ]]; then
  cd "$AGENTIC_GITHUB_WORKDIR"
fi

if [[ -n "$github_token_file" ]]; then
  if [[ ! -f "$github_token_file" ]]; then
    echo "agentic-github-automation: GITHUB_TOKEN_FILE not found" >&2
    exit 78
  fi
  export GH_TOKEN GITHUB_TOKEN
  GH_TOKEN="$(<"$github_token_file")"
  GITHUB_TOKEN="$GH_TOKEN"
  export GIT_TERMINAL_PROMPT=0
  if [[ -n "${AGENTIC_PROVIDER_HOME:-}" ]]; then
    askpass="$AGENTIC_PROVIDER_HOME/git-askpass.sh"
    cat >"$askpass" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:-${GITHUB_TOKEN:-}}" ;;
  *) printf '\n' ;;
esac
EOF
    chmod 0700 "$askpass"
    export GIT_ASKPASS="$askpass"
  fi
fi

exec gh "$@"
