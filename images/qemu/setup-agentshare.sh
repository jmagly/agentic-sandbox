#!/bin/bash
# setup-agentshare.sh - Initialize agentshare file system on host
#
# Usage: sudo ./setup-agentshare.sh

set -euo pipefail

AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Check root
if [[ $EUID -ne 0 ]]; then
    error "This script must be run as root"
    echo "Usage: sudo $0"
    exit 1
fi

info "Initializing agentshare at $AGENTSHARE_ROOT"

# Create directory structure
mkdir -p "$AGENTSHARE_ROOT"/{global,staging}
mkdir -p "$AGENTSHARE_ROOT/global"/{tools,prompts,configs,content,scripts}

# Create RO symlink for VM mounts
ln -sfn global "$AGENTSHARE_ROOT/global-ro"

# Set permissions
chmod 755 "$AGENTSHARE_ROOT"
chmod 755 "$AGENTSHARE_ROOT/global"
chmod -R 755 "$AGENTSHARE_ROOT/global"/*
chmod 770 "$AGENTSHARE_ROOT/staging"

# Create README in global
cat > "$AGENTSHARE_ROOT/global/README.md" << 'EOF'
# Agent Global Share

This directory is mounted read-only inside agent VMs at `/mnt/global` and `~/global`.

## Structure

- `tools/` - Shared utilities and executables
- `prompts/` - System prompts and instructions
- `configs/` - Configuration templates
- `content/` - Reference documents and data
- `scripts/` - Automation scripts

## Adding Content

Files must be promoted via the staging workflow:

```bash
# 1. Place file in staging
cp myfile /srv/agentshare/staging/

# 2. Review and promote
sudo cp /srv/agentshare/staging/myfile /srv/agentshare/global/tools/
sudo chmod 444 /srv/agentshare/global/tools/myfile
```

Do NOT write directly to global from inside an agent VM.
EOF

# Create default prompt template
cat > "$AGENTSHARE_ROOT/global/prompts/default-system.md" << 'EOF'
# System Prompt

You are an AI agent running in an isolated sandbox environment.

## Environment

- OS: Ubuntu 24.04 LTS
- User: `agent` (sudo access)
- Work directory: `~/workspace`
- Output directory: `~/outputs` (synced to host)
- Global tools: `~/global`

## Output Guidelines

1. Write results to `~/outputs/` for collection by the host
2. Log progress to stdout for monitoring
3. Save artifacts with descriptive names

## Resource Limits

- CPU: 4 cores
- Memory: 8GB
- Disk: 50GB (ephemeral)

Your outputs will be collected and reviewed by the orchestrator.
EOF

info "Created directory structure:"
tree -L 2 "$AGENTSHARE_ROOT" 2>/dev/null || ls -laR "$AGENTSHARE_ROOT"

# Verify libvirt can access
if command -v virsh &>/dev/null; then
    info "Checking libvirt access..."
    # Ensure qemu user can read global
    if getent group libvirt-qemu &>/dev/null; then
        chgrp -R libvirt-qemu "$AGENTSHARE_ROOT/global" 2>/dev/null || true
    fi
fi

echo ""
info "Agentshare initialized successfully"
echo ""
echo "Next steps:"
echo "  1. Add tools to: $AGENTSHARE_ROOT/global/tools/"
echo "  2. Add prompts to: $AGENTSHARE_ROOT/global/prompts/"
echo "  3. Provision an agent VM - inbox will be auto-created"
echo ""
echo "Usage in provision-vm.sh:"
echo "  ./provision-vm.sh --agentshare agent-01"
