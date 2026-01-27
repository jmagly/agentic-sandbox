#!/bin/bash
# Entrypoint script for agentic sandbox base image

set -e

# Function to log messages
log() {
    echo "[$(date +'%Y-%m-%d %H:%M:%S')] $*"
}

# Trap signals for graceful shutdown
trap 'log "Received SIGTERM, shutting down..."; exit 0' TERM
trap 'log "Received SIGINT, shutting down..."; exit 0' INT

log "Starting sandbox environment"

# Display environment info
log "User: $(whoami)"
log "UID: $(id -u)"
log "GID: $(id -g)"
log "Working directory: $(pwd)"

# Set up workspace
if [ ! -d "/workspace" ]; then
    log "Creating /workspace directory"
    mkdir -p /workspace
fi

# Initialize workspace if needed
if [ -f "/workspace/.sandbox-init" ]; then
    log "Running workspace initialization"
    /bin/bash /workspace/.sandbox-init
fi

# Execute the main command
if [ $# -eq 0 ]; then
    log "No command specified, starting interactive shell"
    exec /bin/bash
else
    log "Executing command: $*"
    exec "$@"
fi
