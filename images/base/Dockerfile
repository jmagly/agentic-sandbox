# Base image for agentic sandboxes
# Provides minimal runtime with common tools

# Digest pinned per ci/digests.txt (issue #262).
FROM ubuntu:24.04@sha256:c4a8d5503dfb2a3eb8ab5f807da5bc69a85730fb49b5cfca2330194ebcc41c7b

LABEL maintainer="integrolabs"
LABEL description="Base image for agentic sandbox environments"

# Prevent interactive prompts
ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=UTC

# Core system packages
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    gnupg \
    jq \
    openssh-client \
    sudo \
    unzip \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Create agent user with sudo access
RUN useradd -m -s /bin/bash agent \
    && echo "agent ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers.d/agent

# Create workspace directory
RUN mkdir -p /workspace && chown agent:agent /workspace

# Agent runtime directory
RUN mkdir -p /opt/sandbox && chown agent:agent /opt/sandbox

# Set working directory
WORKDIR /workspace

# Switch to agent user
USER agent

# Default command
CMD ["/bin/bash"]
