#!/usr/bin/env bash
# Container image smoke test — verifies an agent container image has the
# toolchain we promised. CI runs this against every variant after build;
# operators can run it locally too.
#
# Usage:
#   tests/container/smoke.sh <variant>
#       variant: base | dev | claude | codex | opencode | automation-control
#
# Issue: #186 (Section E of #181)

set -Eeuo pipefail

VARIANT="${1:?usage: smoke.sh <base|dev|claude|codex|opencode|automation-control>}"
case "$VARIANT" in
    base)            IMAGE="agentic/agent:base" ;;
    dev)             IMAGE="agentic/agent:dev" ;;
    claude|codex|opencode|automation-control) IMAGE="agentic/${VARIANT}:latest" ;;
    *) echo "smoke.sh: unknown variant '$VARIANT'" >&2; exit 2 ;;
esac

echo "[smoke] $VARIANT — $IMAGE"

# 1. Toolchain (skipped for :base which is intentionally minimal).
#    Use `bash -lc` so we exercise the login PATH (the way operators
#    actually use the shell), not just the Dockerfile ENV PATH.
if [[ "$VARIANT" != "base" ]]; then
    docker run --rm --entrypoint /bin/bash "$IMAGE" -lc '
        set -e
        python3 -V
        node --version
        go version
        cargo --version
        rg --version | head -1
        fd --version
        bat --version | head -1
        jq --version
        delta --version | head -1
        xh --version
        grpcurl --version 2>&1 | head -1
        cmake --version | head -1
        ninja --version
        meson --version
        aider --version 2>&1 | head -1
        gh --version | head -1
        gh copilot --help | head -1
        echo "[smoke] toolchain ok"
    '
fi

# 2. Per-variant TUI presence.
case "$VARIANT" in
    claude)
        docker run --rm --entrypoint /bin/bash "$IMAGE" -lc 'set -o pipefail; claude --version | head -1; agentic-claude-automation --version | head -1; agentic-provider-inventory claude | grep -F "schema	agentic.provider_inventory.v1"; agentic-provider-readiness claude | grep -F "schema	agentic.provider_readiness.v1"'
        ;;
    codex)
        docker run --rm --entrypoint /bin/bash "$IMAGE" -lc 'set -o pipefail; codex --version | head -1'
        ;;
    opencode)
        docker run --rm --entrypoint /bin/bash "$IMAGE" -lc 'set -o pipefail; opencode --version | head -1'
        ;;
    automation-control)
        docker run --rm --entrypoint /bin/bash "$IMAGE" -lc 'set -o pipefail; codex --version | head -1; agentic-codex-automation --version | head -1; command -v agentic-claude-automation; agentic-provider-inventory | grep -F "schema	agentic.provider_inventory.v1"; agentic-provider-readiness codex | grep -F "schema	agentic.provider_readiness.v1"'
        ;;
esac

echo "[smoke] $VARIANT ok"
