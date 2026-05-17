# Test image for security hardening validation
# Digest pinned per ci/digests.txt (issue #262).
FROM ubuntu:22.04@sha256:4f838adc7181d9039ac795a7d0aba05a9bd9ecd480d294483169c5def983b64d

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    stress-ng \
    procps \
    iproute2 \
    && rm -rf /var/lib/apt/lists/*

# Non-root user
RUN useradd -m -u 1000 agent
USER agent
WORKDIR /home/agent

CMD ["bash"]
