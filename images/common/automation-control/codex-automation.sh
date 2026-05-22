#!/usr/bin/env bash
# Low-churn Codex TUI launcher for orchestrator-observed PTY sessions.
set -euo pipefail

export TERM="${AGENTIC_CODEX_TERM:-xterm}"
export NO_COLOR="${NO_COLOR:-1}"

if [[ -n "${AGENTIC_CODEX_WORKDIR:-}" ]]; then
  cd "$AGENTIC_CODEX_WORKDIR"
fi

exec codex --no-alt-screen "$@"
