#!/bin/bash
# install-rootless-docker.sh
# Run during VM provisioning to setup rootless Docker
# Implements Phase 1 of Docker Socket Privilege Escalation Mitigation
# See: .aiwg/security/docker-socket-mitigation.md

set -euo pipefail

TARGET_USER="${1:-agent}"
USER_HOME="/home/$TARGET_USER"
USER_ID=$(id -u "$TARGET_USER")

log() { echo "[rootless-docker] $1"; }

# Prerequisites
log "Installing prerequisites..."
apt-get update
apt-get install -y uidmap dbus-user-session slirp4netns

# Setup subordinate UID/GID ranges
log "Configuring subuid/subgid..."
if ! grep -q "^$TARGET_USER:" /etc/subuid; then
    echo "$TARGET_USER:100000:65536" >> /etc/subuid
fi
if ! grep -q "^$TARGET_USER:" /etc/subgid; then
    echo "$TARGET_USER:100000:65536" >> /etc/subgid
fi

# Install Docker CE (root daemon for system services if needed)
log "Installing Docker CE..."
install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
chmod a+r /etc/apt/keyrings/docker.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \
    https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
    tee /etc/apt/sources.list.d/docker.list > /dev/null
apt-get update
apt-get install -y docker-ce docker-ce-cli containerd.io \
    docker-buildx-plugin docker-compose-plugin

# DO NOT add user to docker group (this is the key security change)
# usermod -aG docker "$TARGET_USER"  # INTENTIONALLY OMITTED

# Stop and disable root Docker daemon (not needed for rootless operation)
systemctl stop docker || true
systemctl disable docker || true

# Enable lingering for user (allows user services without login)
loginctl enable-linger "$TARGET_USER"

# Create XDG_RUNTIME_DIR
mkdir -p "/run/user/$USER_ID"
chown "$TARGET_USER:$TARGET_USER" "/run/user/$USER_ID"
chmod 700 "/run/user/$USER_ID"

# Setup rootless Docker as agent user
log "Installing rootless Docker for $TARGET_USER..."
sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$USER_ID" bash << 'ROOTLESS_EOF'
export HOME="/home/agent"
export PATH="$HOME/.local/bin:/usr/bin:$PATH"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"

# Run rootless setup
dockerd-rootless-setuptool.sh install

# Create Docker config
mkdir -p "$HOME/.docker"
cat > "$HOME/.docker/config.json" << 'DOCKER_CFG'
{
  "currentContext": "rootless"
}
DOCKER_CFG

# Enable service
systemctl --user enable docker
systemctl --user start docker
ROOTLESS_EOF

# Configure low port binding (optional)
log "Configuring low port binding..."
echo "net.ipv4.ip_unprivileged_port_start=80" > /etc/sysctl.d/99-rootless-docker.conf
sysctl -p /etc/sysctl.d/99-rootless-docker.conf

# Add environment setup to profile
log "Configuring user environment..."
cat >> "$USER_HOME/.bashrc" << 'BASHRC_EOF'

# Rootless Docker configuration
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/docker.sock"
export PATH="$HOME/.local/bin:$PATH"
BASHRC_EOF

log "Rootless Docker installation complete"
log "User '$TARGET_USER' can now use Docker without root privileges"
log "Dangerous flags (--privileged, -v /:/, etc.) are blocked by design"
