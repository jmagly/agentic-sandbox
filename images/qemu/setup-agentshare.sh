#!/bin/bash
# setup-agentshare.sh - Initialize agentshare file system on host
#
# Usage: sudo ./setup-agentshare.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
AGENTSHARE_STORAGE_MODE="${AGENTSHARE_STORAGE_MODE:-loopback}"
AGENTSHARE_BACKING_FILE="${AGENTSHARE_BACKING_FILE:-/var/lib/agentic-sandbox/agentshare.xfs}"
AGENTSHARE_SIZE_GIB="${AGENTSHARE_SIZE_GIB:-64}"
AGENTSHARE_PERSIST="${AGENTSHARE_PERSIST:-1}"

# Source shared logging library if available
LOGGING_LIB="$PROJECT_ROOT/scripts/lib/logging.sh"
if [[ -f "$LOGGING_LIB" && "${USE_SHARED_LOGGING:-true}" == "true" ]]; then
    # shellcheck source=../../scripts/lib/logging.sh
    source "$LOGGING_LIB"
    LOG_SCRIPT_NAME="setup-agentshare"
    # Alias for backward compatibility
    info() { log_info "$*"; }
    warn() { log_warn "$*"; }
    error() { log_error "$*"; }
else
    # Fallback to inline logging
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m'
    info() { echo -e "${GREEN}[INFO]${NC} $*"; }
    warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
    error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }
fi

# Check root
if [[ $EUID -ne 0 ]]; then
    error "This script must be run as root"
    echo "Usage: sudo $0"
    exit 1
fi

validate_storage_path() {
    local path="$1" label="$2"
    if [[ "$path" != /* || "$path" == "/" || "$path" == "/srv" || "$path" == "/var" ]]; then
        error "$label must be a specific absolute path, got: $path"
        exit 1
    fi
}

verify_dedicated_mount() {
    local root_device share_device options fstype
    root_device="$(stat -c %d /)"
    share_device="$(stat -c %d "$AGENTSHARE_ROOT")"
    fstype="$(findmnt -n -o FSTYPE --target "$AGENTSHARE_ROOT")"
    options="$(findmnt -n -o OPTIONS --target "$AGENTSHARE_ROOT")"
    [[ "$root_device" != "$share_device" ]] || {
        error "Agentshare shares the host-root device; use AGENTSHARE_STORAGE_MODE=loopback or mount a dedicated XFS device"
        exit 1
    }
    [[ "$fstype" == "xfs" ]] || {
        error "Agentshare must use XFS for project quotas (found: $fstype)"
        exit 1
    }
    [[ ",$options," == *,prjquota,* || ",$options," == *,pquota,* ]] || {
        error "Agentshare XFS mount is missing prjquota"
        exit 1
    }
}

prepare_agentshare_storage() {
    validate_storage_path "$AGENTSHARE_ROOT" "AGENTSHARE_ROOT"
    case "$AGENTSHARE_STORAGE_MODE" in
        loopback)
            validate_storage_path "$AGENTSHARE_BACKING_FILE" "AGENTSHARE_BACKING_FILE"
            command -v mkfs.xfs >/dev/null 2>&1 || {
                error "mkfs.xfs is required for secure loopback agentshare storage"
                exit 1
            }
            command -v findmnt >/dev/null 2>&1 || {
                error "findmnt is required for agentshare mount verification"
                exit 1
            }
            if mountpoint -q "$AGENTSHARE_ROOT"; then
                verify_dedicated_mount
                return
            fi
            if [[ -d "$AGENTSHARE_ROOT" ]] && find "$AGENTSHARE_ROOT" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
                error "Refusing to mount over populated agentshare directory: $AGENTSHARE_ROOT"
                error "Migrate its contents to dedicated storage before retrying"
                exit 1
            fi
            mkdir -p "$(dirname "$AGENTSHARE_BACKING_FILE")" "$AGENTSHARE_ROOT"
            if [[ ! -e "$AGENTSHARE_BACKING_FILE" ]]; then
                [[ "$AGENTSHARE_SIZE_GIB" =~ ^[1-9][0-9]*$ ]] || {
                    error "AGENTSHARE_SIZE_GIB must be a positive integer"
                    exit 1
                }
                info "Creating size-capped ${AGENTSHARE_SIZE_GIB}GiB XFS backing file"
                truncate -s "${AGENTSHARE_SIZE_GIB}G" "$AGENTSHARE_BACKING_FILE"
                mkfs.xfs -q "$AGENTSHARE_BACKING_FILE"
                chmod 600 "$AGENTSHARE_BACKING_FILE"
            elif [[ "$(blkid -o value -s TYPE "$AGENTSHARE_BACKING_FILE" 2>/dev/null || true)" != "xfs" ]]; then
                error "Existing agentshare backing file is not XFS; refusing to format it"
                exit 1
            fi
            mount -o loop,prjquota,nosuid,nodev "$AGENTSHARE_BACKING_FILE" "$AGENTSHARE_ROOT"
            if [[ "$AGENTSHARE_PERSIST" == "1" ]]; then
                fstab_entry="$AGENTSHARE_BACKING_FILE $AGENTSHARE_ROOT xfs loop,prjquota,nosuid,nodev,nofail 0 0"
                grep -Fqx "$fstab_entry" /etc/fstab || printf '%s\n' "$fstab_entry" >> /etc/fstab
            elif [[ "$AGENTSHARE_PERSIST" != "0" ]]; then
                error "AGENTSHARE_PERSIST must be 0 or 1"
                exit 1
            fi
            verify_dedicated_mount
            ;;
        existing)
            mountpoint -q "$AGENTSHARE_ROOT" || {
                error "AGENTSHARE_STORAGE_MODE=existing requires an existing dedicated mount"
                exit 1
            }
            verify_dedicated_mount
            ;;
        allow-host-root)
            warn "Using host-root agentshare storage by explicit request; unsuitable for T2+ isolation"
            mkdir -p "$AGENTSHARE_ROOT"
            ;;
        *)
            error "AGENTSHARE_STORAGE_MODE must be loopback, existing, or allow-host-root"
            exit 1
            ;;
    esac
}

prepare_agentshare_storage

info "Initializing agentshare at $AGENTSHARE_ROOT"

# Create directory structure
mkdir -p "$AGENTSHARE_ROOT"/{global,staging,tasks}
mkdir -p "$AGENTSHARE_ROOT/global"/{tools,prompts,configs,content,scripts}

# Create RO symlink for VM mounts
ln -sfn global "$AGENTSHARE_ROOT/global-ro"

# Set permissions
chmod 755 "$AGENTSHARE_ROOT"
chmod 755 "$AGENTSHARE_ROOT/global"
chmod -R 755 "$AGENTSHARE_ROOT/global"/*
chmod 770 "$AGENTSHARE_ROOT/staging"
chmod 755 "$AGENTSHARE_ROOT/tasks"

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
echo "  3. Provision an agent VM - inbox/outbox will be auto-created"
echo ""
echo "Usage in provision-vm.sh:"
echo "  ./provision-vm.sh --agentshare agent-01           # Agent mode"
echo "  ./provision-vm.sh --task-id <uuid> agent-01       # Task orchestration mode"
echo ""
echo "Task storage structure:"
echo "  $AGENTSHARE_ROOT/tasks/<task-id>/"
echo "    ├── inbox/           # Cloned repo + working files"
echo "    └── outbox/"
echo "        ├── progress/    # Real-time streaming"
echo "        │   ├── stdout.log"
echo "        │   ├── stderr.log"
echo "        │   └── events.jsonl"
echo "        ├── artifacts/   # Final deliverables"
echo "        └── metadata.json"
