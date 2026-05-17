# Claude Code Agent Image
# Full development environment with Claude Code CLI

# Internal base image (issue #262). Version kept in sync with Makefile's
# BASE_VERSION variable. When this base image is published to a registry,
# this reference should pick up an @sha256:<digest> pin per ci/digests.txt.
FROM agentic-sandbox-base:v2026.5.17

LABEL description="Claude Code agent sandbox with full development tools"

USER root

# Development tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    python3 \
    python3-pip \
    python3-venv \
    ripgrep \
    fd-find \
    tree \
    tmux \
    vim \
    && rm -rf /var/lib/apt/lists/*

# Node.js (for Claude Code CLI)
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI
# Pin tracked in ci/npm-pins.txt (issue #266).
# --ignore-scripts blocks lifecycle-script supply-chain attacks.
RUN npm install -g --ignore-scripts @anthropic-ai/claude-code@2.1.143

# Agent entrypoint script
COPY entrypoint.sh /opt/sandbox/entrypoint.sh
RUN chmod +x /opt/sandbox/entrypoint.sh

USER agent

# Claude Code configuration directory
RUN mkdir -p ~/.config/claude-code

# Environment variables
ENV AGENT_MODE=autonomous
ENV AGENT_TIMEOUT=86400
ENV CLAUDE_CODE_ACCEPT_TOS=1

ENTRYPOINT ["/opt/sandbox/entrypoint.sh"]
CMD ["claude"]
