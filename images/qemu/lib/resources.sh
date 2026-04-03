#!/bin/bash
# lib/resources.sh - VM resource sizing, limits, and disk quota management
#
# Provides functions for:
#   - Size string parsing (8G, 512M) to bytes and MiB
#   - Memory string parsing to MB
#   - Resource limit calculation from user options or defaults
#   - XFS project quota setup for inbox directories
#
# No required globals — all functions are self-contained.

# Parse memory string to MB
parse_memory_mb() {
    local mem="$1"
    local value="${mem%[GgMm]}"
    local unit="${mem: -1}"

    case "$unit" in
        G|g) echo $((value * 1024)) ;;
        M|m) echo "$value" ;;
        *)   echo "$mem" ;;  # Assume MB if no unit
    esac
}

# Parse size string (8G, 512M, etc.) to bytes
parse_size_to_bytes() {
    local size_str="$1"
    local number="${size_str%[GMKgmk]*}"
    local unit="${size_str##*[0-9]}"

    case "${unit^^}" in
        G|GB) echo $((number * 1024 * 1024 * 1024)) ;;
        M|MB) echo $((number * 1024 * 1024)) ;;
        K|KB) echo $((number * 1024)) ;;
        *) echo "$number" ;;
    esac
}

# Parse size string to MiB
parse_size_to_mib() {
    local bytes
    bytes=$(parse_size_to_bytes "$1")
    echo $((bytes / 1024 / 1024))
}

# Calculate resource limits from user options or defaults
calculate_resource_limits() {
    local vm_cpus="$1"
    local vm_memory_mb="$2"
    local user_mem_limit="$3"
    local user_cpu_quota="$4"
    local user_io_read="$5"
    local user_io_write="$6"

    # Memory hard limit: default to 94% of VM memory
    local mem_limit_mb
    if [[ -z "$user_mem_limit" ]]; then
        mem_limit_mb=$((vm_memory_mb * 94 / 100))
    else
        mem_limit_mb=$(parse_size_to_mib "$user_mem_limit")
    fi

    # CPU quota: default to cpus * 100%
    local cpu_quota_pct
    if [[ -z "$user_cpu_quota" ]]; then
        cpu_quota_pct=$((vm_cpus * 100))
    else
        cpu_quota_pct="$user_cpu_quota"
    fi

    # I/O limits: convert to bytes/sec, defaults are 500M read, 200M write
    local io_read_bps io_write_bps
    if [[ -z "$user_io_read" ]]; then
        io_read_bps=524288000  # 500M
    else
        io_read_bps=$(parse_size_to_bytes "$user_io_read")
    fi

    if [[ -z "$user_io_write" ]]; then
        io_write_bps=209715200  # 200M
    else
        io_write_bps=$(parse_size_to_bytes "$user_io_write")
    fi

    echo "$mem_limit_mb $cpu_quota_pct $io_read_bps $io_write_bps"
}

# Setup XFS project quota on a directory
# Uses XFS project quotas if available, otherwise skips gracefully
setup_inbox_quota() {
    local path="$1"
    local quota_str="$2"  # e.g., "50G"
    local project_name="$3"

    # Convert quota string to GB
    local quota_gb
    local number="${quota_str%[GMKgmk]*}"
    local unit="${quota_str##*[0-9]}"
    case "${unit^^}" in
        G|GB) quota_gb="$number" ;;
        M|MB) quota_gb=$((number / 1024)); [[ $quota_gb -eq 0 ]] && quota_gb=1 ;;
        K|KB) quota_gb=1 ;;  # Minimum 1GB
        *) quota_gb="$number" ;;  # Assume GB if no unit
    esac

    # Check if xfs_quota is available
    if ! command -v xfs_quota &>/dev/null; then
        log_warn "xfs_quota not found - disk quota not enforced"
        return 0
    fi

    # Check if path is on XFS with prjquota
    local mount_point
    mount_point=$(df "$path" 2>/dev/null | tail -1 | awk '{print $NF}')
    local fstype
    fstype=$(df -T "$path" 2>/dev/null | tail -1 | awk '{print $2}')

    if [[ "$fstype" != "xfs" ]]; then
        log_warn "Disk quota requires XFS filesystem (found: $fstype)"
        return 0
    fi

    if ! mount | grep "$mount_point" | grep -q "prjquota" 2>/dev/null; then
        log_warn "XFS not mounted with prjquota - disk quota not enforced"
        return 0
    fi

    # Setup quota files if they don't exist
    local projid_file="/etc/projid"
    local projects_file="/etc/projects"
    sudo touch "$projid_file" "$projects_file" 2>/dev/null || true

    # Generate deterministic project ID from name (range 10000-19999)
    local hash
    hash=$(echo -n "$project_name" | md5sum | cut -c1-8)
    local project_id=$((16#$hash % 10000 + 10000))

    # Register project
    if ! grep -q "^${project_name}:" "$projid_file" 2>/dev/null; then
        echo "${project_name}:${project_id}" | sudo tee -a "$projid_file" >/dev/null
    fi
    if ! grep -q "^${project_id}:${path}$" "$projects_file" 2>/dev/null; then
        echo "${project_id}:${path}" | sudo tee -a "$projects_file" >/dev/null
    fi

    # Initialize and set quota
    sudo xfs_quota -x -c "project -s ${project_name}" "$mount_point" 2>/dev/null || true

    local hard_kb=$((quota_gb * 1024 * 1024))
    local soft_kb=$((hard_kb * 90 / 100))

    if sudo xfs_quota -x -c "limit -p bsoft=${soft_kb}k bhard=${hard_kb}k ${project_name}" "$mount_point" 2>/dev/null; then
        log_success "Disk quota set: ${quota_gb}GB for $path"
    else
        log_warn "Failed to set disk quota (non-fatal)"
    fi
}
