# Base image for agentic sandboxes
# Provides minimal runtime with common tools

FROM ubuntu:24.04

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
