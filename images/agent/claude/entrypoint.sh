#!/bin/bash
set -e

# Agent entrypoint script
# Handles initialization and task execution

echo "[sandbox] Initializing agent environment..."

# Configure git if credentials are mounted
if [ -f /run/secrets/git-credentials ]; then
    git config --global credential.helper 'store --file=/run/secrets/git-credentials'
fi

# Configure SSH if key is mounted
if [ -f /run/secrets/ssh-key ]; then
    mkdir -p ~/.ssh
    cp /run/secrets/ssh-key ~/.ssh/id_ed25519
    chmod 600 ~/.ssh/id_ed25519
    ssh-keyscan github.com gitlab.com >> ~/.ssh/known_hosts 2>/dev/null || true
fi

# Set up Anthropic API key if provided
if [ -n "$ANTHROPIC_API_KEY" ]; then
    echo "[sandbox] API key configured"
fi

# Log agent start
echo "[sandbox] Agent starting at $(date -Iseconds)"
echo "[sandbox] Mode: ${AGENT_MODE:-interactive}"
echo "[sandbox] Timeout: ${AGENT_TIMEOUT:-3600}s"

# Execute task if provided
if [ -n "$AGENT_TASK" ]; then
    echo "[sandbox] Executing task: $AGENT_TASK"

    if [ "$AGENT_MODE" = "autonomous" ]; then
        # Run in autonomous mode with timeout
        timeout "${AGENT_TIMEOUT:-3600}" claude --dangerously-skip-permissions "$AGENT_TASK"
    else
        claude "$AGENT_TASK"
    fi
else
    # Interactive mode - run command or shell
    exec "$@"
fi
