# Test image for security hardening validation
FROM ubuntu:22.04

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
