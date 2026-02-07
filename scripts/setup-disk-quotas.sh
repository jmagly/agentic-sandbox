#!/bin/bash
# setup-disk-quotas.sh - Initialize XFS project quotas for agentshare storage
#
# This script manages disk quotas for the agentshare filesystem to prevent
# any single agent from consuming all available storage.
#
# Prerequisites:
#   - /srv/agentshare mounted on XFS filesystem with prjquota option
#   - Root privileges
#
# Usage:
#   sudo ./setup-disk-quotas.sh setup              # Initial setup
#   sudo ./setup-disk-quotas.sh create <task-id> [quota-gb]
#   sudo ./setup-disk-quotas.sh remove <task-id>
#   sudo ./setup-disk-quotas.sh report
#
# Reference: docs/security/resource-quota-design.md

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Source shared logging library if available
LOGGING_LIB="$PROJECT_ROOT/scripts/lib/logging.sh"
if [[ -f "$LOGGING_LIB" && "${USE_SHARED_LOGGING:-true}" == "true" ]]; then
    # shellcheck source=lib/logging.sh
    source "$LOGGING_LIB"
    LOG_SCRIPT_NAME="setup-disk-quotas"
else
    # Fallback to inline logging
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
    log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
    log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
    log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
    log_error() { echo -e "${RED}[ERROR]${NC} $1" >&2; }
fi

# Configuration
AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
PROJID_FILE="/etc/projid"
PROJECTS_FILE="/etc/projects"

# Project ID allocation ranges
# 1000-1999: Reserved for system (global share, staging, etc.)
# 2000-9999: Task inboxes/outboxes
# 10000+: Future use (multi-tenant, etc.)
PROJECT_ID_SYSTEM_START=1000
PROJECT_ID_TASK_START=2000
PROJECT_ID_TASK_MAX=9999

# Default quotas (in GB)
DEFAULT_TASK_QUOTA_GB=50
GLOBAL_SHARE_QUOTA_GB=100
STAGING_QUOTA_GB=20

# ============================================================================
# Utility Functions
# ============================================================================

check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root"
        echo "Usage: sudo $0 <command> [args]"
        exit 1
    fi
}

check_xfs_support() {
    local mount_point="$1"

    # Check if mount point exists
    if [[ ! -d "$mount_point" ]]; then
        log_error "Mount point does not exist: $mount_point"
        return 1
    fi

    # Check if it's XFS
    local fstype
    fstype=$(df -T "$mount_point" | tail -1 | awk '{print $2}')

    if [[ "$fstype" != "xfs" ]]; then
        log_warn "$mount_point is not XFS (found: $fstype)"
        echo ""
        echo "XFS project quotas require an XFS filesystem."
        echo "Options:"
        echo "  1. Create XFS filesystem:"
        echo "     mkfs.xfs /dev/sdX"
        echo "     mount -o prjquota /dev/sdX $mount_point"
        echo ""
        echo "  2. Use ext4 with usrquota (alternative, less flexible):"
        echo "     See docs/security/resource-quota-design.md"
        echo ""
        return 1
    fi

    # Check if prjquota is enabled
    if ! mount | grep "$mount_point" | grep -q "prjquota"; then
        log_warn "$mount_point not mounted with prjquota option"
        echo ""
        echo "Add prjquota to mount options:"
        echo "  1. Edit /etc/fstab:"
        echo "     /dev/sdX  $mount_point  xfs  defaults,prjquota  0  0"
        echo ""
        echo "  2. Remount:"
        echo "     mount -o remount,prjquota $mount_point"
        echo ""
        return 1
    fi

    log_success "XFS with prjquota verified: $mount_point"
    return 0
}

init_quota_files() {
    touch "$PROJID_FILE" "$PROJECTS_FILE" 2>/dev/null || true
    chmod 644 "$PROJID_FILE" "$PROJECTS_FILE"
}

# Generate deterministic project ID from string (task ID, etc.)
# Uses hash to map to available range
generate_project_id() {
    local name="$1"
    local range_start="$2"
    local range_size="$3"

    local hash
    hash=$(echo -n "$name" | md5sum | cut -c1-8)
    local id=$((16#$hash % range_size + range_start))
    echo "$id"
}

# Add project to quota tracking files
register_project() {
    local project_name="$1"
    local project_id="$2"
    local path="$3"

    # Add to projid if not exists
    if ! grep -q "^${project_name}:" "$PROJID_FILE" 2>/dev/null; then
        echo "${project_name}:${project_id}" >> "$PROJID_FILE"
    fi

    # Add to projects if not exists
    if ! grep -q "^${project_id}:${path}$" "$PROJECTS_FILE" 2>/dev/null; then
        echo "${project_id}:${path}" >> "$PROJECTS_FILE"
    fi
}

# Set quota for a project
set_project_quota() {
    local project_name="$1"
    local quota_gb="$2"
    local mount_point="$3"

    # Initialize project directory ownership
    xfs_quota -x -c "project -s ${project_name}" "$mount_point" 2>/dev/null || true

    # Calculate limits (soft = 90% of hard)
    local hard_kb=$((quota_gb * 1024 * 1024))
    local soft_kb=$((hard_kb * 90 / 100))

    # Set quota
    xfs_quota -x -c "limit -p bsoft=${soft_kb}k bhard=${hard_kb}k ${project_name}" "$mount_point"
}

# ============================================================================
# Command Implementations
# ============================================================================

cmd_setup() {
    log_info "Setting up XFS project quotas for $AGENTSHARE_ROOT"

    # Verify XFS support
    if ! check_xfs_support "$AGENTSHARE_ROOT"; then
        exit 1
    fi

    # Initialize quota files
    init_quota_files

    # Create system projects
    log_info "Creating system quotas..."

    # Global share (read-only content)
    local global_path="$AGENTSHARE_ROOT/global"
    if [[ -d "$global_path" ]]; then
        register_project "agentshare_global" 1001 "$global_path"
        set_project_quota "agentshare_global" "$GLOBAL_SHARE_QUOTA_GB" "$AGENTSHARE_ROOT"
        log_success "Global share quota: ${GLOBAL_SHARE_QUOTA_GB}GB"
    fi

    # Staging area
    local staging_path="$AGENTSHARE_ROOT/staging"
    if [[ -d "$staging_path" ]]; then
        register_project "agentshare_staging" 1002 "$staging_path"
        set_project_quota "agentshare_staging" "$STAGING_QUOTA_GB" "$AGENTSHARE_ROOT"
        log_success "Staging quota: ${STAGING_QUOTA_GB}GB"
    fi

    # Tasks root (parent directory for task quotas)
    local tasks_path="$AGENTSHARE_ROOT/tasks"
    mkdir -p "$tasks_path"
    chmod 755 "$tasks_path"

    echo ""
    log_success "Quota setup complete"
    echo ""
    cmd_report
}

cmd_create() {
    local task_id="$1"
    local quota_gb="${2:-$DEFAULT_TASK_QUOTA_GB}"

    if [[ -z "$task_id" ]]; then
        log_error "Task ID required"
        echo "Usage: $0 create <task-id> [quota-gb]"
        exit 1
    fi

    log_info "Creating quota for task: $task_id (${quota_gb}GB)"

    # Generate project ID
    local range_size=$((PROJECT_ID_TASK_MAX - PROJECT_ID_TASK_START))
    local project_id
    project_id=$(generate_project_id "$task_id" "$PROJECT_ID_TASK_START" "$range_size")

    # Use short name for project (first 8 chars of task ID)
    local project_name="task_${task_id:0:8}"

    # Create task directories
    local task_root="$AGENTSHARE_ROOT/tasks/${task_id}"
    local inbox_path="${task_root}/inbox"
    local outbox_path="${task_root}/outbox"

    mkdir -p "$inbox_path" "$outbox_path"
    chmod 770 "$inbox_path" "$outbox_path"

    # Register project (inbox gets the quota - it's where agent writes)
    register_project "$project_name" "$project_id" "$inbox_path"

    # Set quota
    set_project_quota "$project_name" "$quota_gb" "$AGENTSHARE_ROOT"

    log_success "Created quota: project_id=$project_id, path=$inbox_path, limit=${quota_gb}GB"

    # Output project ID for caller
    echo "$project_id"
}

cmd_remove() {
    local task_id="$1"

    if [[ -z "$task_id" ]]; then
        log_error "Task ID required"
        echo "Usage: $0 remove <task-id>"
        exit 1
    fi

    log_info "Removing quota for task: $task_id"

    local project_name="task_${task_id:0:8}"

    # Remove quota limits (set to 0)
    xfs_quota -x -c "limit -p bsoft=0 bhard=0 ${project_name}" "$AGENTSHARE_ROOT" 2>/dev/null || true

    # Remove from tracking files
    sed -i "/^${project_name}:/d" "$PROJID_FILE" 2>/dev/null || true
    sed -i "/${task_id}/d" "$PROJECTS_FILE" 2>/dev/null || true

    log_success "Removed quota for: $project_name"
}

cmd_report() {
    echo "=============================================="
    echo "XFS Project Quota Report: $AGENTSHARE_ROOT"
    echo "=============================================="
    echo ""

    if ! command -v xfs_quota &>/dev/null; then
        log_error "xfs_quota not found. Install xfsprogs package."
        exit 1
    fi

    # Human-readable report
    xfs_quota -x -c "report -p -h" "$AGENTSHARE_ROOT" 2>/dev/null || {
        log_warn "Could not generate quota report (quotas may not be enabled)"
    }

    echo ""
    echo "Legend:"
    echo "  Used    = Current disk usage"
    echo "  Soft    = Warning threshold (90% of limit)"
    echo "  Hard    = Maximum allowed usage"
    echo ""
}

cmd_check() {
    local task_id="$1"

    if [[ -z "$task_id" ]]; then
        log_error "Task ID required"
        echo "Usage: $0 check <task-id>"
        exit 1
    fi

    local project_name="task_${task_id:0:8}"

    echo "Quota status for: $project_name"
    echo ""

    xfs_quota -x -c "quota -p -h ${project_name}" "$AGENTSHARE_ROOT" 2>/dev/null || {
        log_error "Project not found: $project_name"
        exit 1
    }
}

cmd_usage() {
    cat <<EOF
XFS Project Quota Manager for Agentic Sandbox

Usage: $0 <command> [arguments]

Commands:
  setup                     Initial quota setup (run once)
  create <task-id> [gb]     Create quota for task (default: ${DEFAULT_TASK_QUOTA_GB}GB)
  remove <task-id>          Remove quota for task
  check <task-id>           Check quota status for task
  report                    Show all quota usage

Examples:
  # Initial setup
  sudo $0 setup

  # Create 50GB quota for task
  sudo $0 create a1b2c3d4-e5f6-7890-abcd-ef1234567890

  # Create 100GB quota for large task
  sudo $0 create a1b2c3d4-e5f6-7890-abcd-ef1234567890 100

  # Check task quota usage
  sudo $0 check a1b2c3d4-e5f6-7890-abcd-ef1234567890

  # Remove quota after task completion
  sudo $0 remove a1b2c3d4-e5f6-7890-abcd-ef1234567890

Prerequisites:
  - XFS filesystem mounted at $AGENTSHARE_ROOT
  - Mount option 'prjquota' enabled
  - xfsprogs package installed

Configuration:
  AGENTSHARE_ROOT=$AGENTSHARE_ROOT
  DEFAULT_TASK_QUOTA_GB=$DEFAULT_TASK_QUOTA_GB

EOF
}

# ============================================================================
# Main
# ============================================================================

main() {
    local command="${1:-help}"
    shift || true

    check_root

    case "$command" in
        setup)
            cmd_setup
            ;;
        create)
            cmd_create "$@"
            ;;
        remove)
            cmd_remove "$@"
            ;;
        check)
            cmd_check "$@"
            ;;
        report)
            cmd_report
            ;;
        help|--help|-h)
            cmd_usage
            ;;
        *)
            log_error "Unknown command: $command"
            echo ""
            cmd_usage
            exit 1
            ;;
    esac
}

main "$@"
