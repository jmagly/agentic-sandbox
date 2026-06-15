#!/bin/bash
set -e

# Agent entrypoint script
# Handles initialization and task execution

echo "[sandbox] Initializing agent environment..."

credential_dir="${AGENTIC_CREDENTIAL_DIR:-}"

# Configure git from a session-scoped credential helper file if provided.
git_credentials_file="${GIT_CREDENTIALS_FILE:-}"
if [ -z "$git_credentials_file" ] && [ -n "$credential_dir" ] && [ -f "$credential_dir/git_credentials" ]; then
    git_credentials_file="$credential_dir/git_credentials"
fi
if [ -n "$git_credentials_file" ] && [ -f "$git_credentials_file" ]; then
    git config --global credential.helper "store --file=$git_credentials_file"
fi

# Configure SSH without copying leased keys into a durable home.
ssh_key_file="${SSH_PRIVATE_KEY_FILE:-}"
if [ -z "$ssh_key_file" ] && [ -n "$credential_dir" ] && [ -f "$credential_dir/ssh_private_key" ]; then
    ssh_key_file="$credential_dir/ssh_private_key"
fi
if [ -n "$ssh_key_file" ] && [ -f "$ssh_key_file" ]; then
    export GIT_SSH_COMMAND="ssh -i $ssh_key_file -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"
fi

# Set up Anthropic API key from a leased file. Raw ANTHROPIC_API_KEY remains
# supported only as an explicit compatibility path from the parent environment.
anthropic_key_file="${ANTHROPIC_API_KEY_FILE:-}"
if [ -z "$anthropic_key_file" ] && [ -n "$credential_dir" ] && [ -f "$credential_dir/anthropic_api_key" ]; then
    anthropic_key_file="$credential_dir/anthropic_api_key"
fi
if [ -n "$anthropic_key_file" ] && [ -f "$anthropic_key_file" ]; then
    export ANTHROPIC_API_KEY="$(cat "$anthropic_key_file")"
    echo "[sandbox] Anthropic credential lease configured"
elif [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    echo "[sandbox] Anthropic API key configured from parent env compatibility path"
fi

# Log agent start
echo "[sandbox] Agent starting at $(date -Iseconds)"
echo "[sandbox] Mode: ${AGENT_MODE:-interactive}"
echo "[sandbox] Timeout: ${AGENT_TIMEOUT:-3600}s"

# Execute task if provided
if [ -n "$AGENT_TASK" ]; then
    echo "[sandbox] Executing task"

    if [ "$AGENT_MODE" = "autonomous" ]; then
        # Run in autonomous mode with timeout
        exec timeout "${AGENT_TIMEOUT:-3600}" claude --dangerously-skip-permissions "$AGENT_TASK"
    else
        exec claude "$AGENT_TASK"
    fi
else
    # Interactive mode - run command or shell
    exec "$@"
fi
