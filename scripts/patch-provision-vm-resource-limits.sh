#!/bin/bash
# patch-provision-vm-resource-limits.sh
#
# Apply resource limit enhancements to provision-vm.sh
# Implements: Gitea issue #86 - cgroup v2 resource limits and disk quotas
#
# Usage: ./patch-provision-vm-resource-limits.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROVISION_SCRIPT="$PROJECT_ROOT/images/qemu/provision-vm.sh"
BACKUP_FILE="$PROVISION_SCRIPT.backup-$(date +%Y%m%d-%H%M%S)"

echo "Resource Limit Patch for provision-vm.sh"
echo "========================================="
echo ""

# Check if provision script exists
if [[ ! -f "$PROVISION_SCRIPT" ]]; then
    echo "ERROR: $PROVISION_SCRIPT not found"
    exit 1
fi

# Create backup
echo "[1/9] Creating backup: $BACKUP_FILE"
cp "$PROVISION_SCRIPT" "$BACKUP_FILE"

# Step 1: Add helper functions after IP allocation functions (around line 1690)
echo "[2/9] Adding resource limit helper functions..."
sed -i '/^# Provision a VM$/i\
# Parse size string (8G, 512M, etc.) to bytes\
parse_size_to_bytes() {\
    local size_str="$1"\
    local number="${size_str%[GMK]*}"\
    local unit="${size_str##*[0-9]}"\
\
    case "${unit^^}" in\
        G|GB)\
            echo $((number * 1024 * 1024 * 1024))\
            ;;\
        M|MB)\
            echo $((number * 1024 * 1024))\
            ;;\
        K|KB)\
            echo $((number * 1024))\
            ;;\
        *)\
            echo "$number"\
            ;;\
    esac\
}\
\
# Parse size string to MiB\
parse_size_to_mb() {\
    local bytes=$(parse_size_to_bytes "$1")\
    echo $((bytes / 1024 / 1024))\
}\
\
# Calculate resource limits from user options or defaults\
calculate_resource_limits() {\
    local vm_cpus="$1"\
    local vm_memory_mb="$2"\
    local user_mem_limit="$3"\
    local user_mem_soft="$4"\
    local user_cpu_quota="$5"\
    local user_io_read="$6"\
    local user_io_write="$7"\
\
    # Memory limit: default to 94% of VM memory (leave 6% for kernel/systemd)\
    if [[ -z "$user_mem_limit" ]]; then\
        mem_limit_mb=$((vm_memory_mb * 94 / 100))\
    else\
        mem_limit_mb=$(parse_size_to_mb "$user_mem_limit")\
    fi\
\
    # Memory soft limit: default to 75% of hard limit\
    if [[ -z "$user_mem_soft" ]]; then\
        mem_soft_mb=$((mem_limit_mb * 75 / 100))\
    else\
        mem_soft_mb=$(parse_size_to_mb "$user_mem_soft")\
    fi\
\
    # CPU quota: default to cpus * 100% (400 for 4 cores)\
    if [[ -z "$user_cpu_quota" ]]; then\
        cpu_quota_pct=$((vm_cpus * 100))\
    else\
        cpu_quota_pct="$user_cpu_quota"\
    fi\
\
    # I/O limits: convert to bytes/sec\
    if [[ -z "$user_io_read" ]]; then\
        io_read_bps=524288000  # 500MB/s\
    else\
        io_read_bps=$(parse_size_to_bytes "$user_io_read")\
    fi\
\
    if [[ -z "$user_io_write" ]]; then\
        io_write_bps=209715200  # 200MB/s\
    else\
        io_write_bps=$(parse_size_to_bytes "$user_io_write")\
    fi\
\
    echo "$mem_limit_mb $mem_soft_mb $cpu_quota_pct $io_read_bps $io_write_bps"\
}\
' "$PROVISION_SCRIPT"

# Step 2: Update usage() function to add resource limit documentation
echo "[3/9] Updating usage() documentation..."
sed -i '/^  -h, --help/a\
\
Resource Limits (cgroup v2 and libvirt tuning):\
  --mem-limit SIZE      Memory hard limit for agent service (default: auto from --memory)\
  --mem-soft SIZE       Memory soft limit (default: 75% of --mem-limit)\
  --cpu-quota PERCENT   CPU quota as percentage (default: cpus * 100%)\
  --tasks-max NUM       Maximum PIDs/tasks (default: 2048)\
  --io-weight NUM       I/O scheduling weight 100-10000 (default: 500)\
  --io-read-limit SIZE  Read bandwidth limit (default: 500M)\
  --io-write-limit SIZE Write bandwidth limit (default: 200M)\
  --nofile-limit NUM    File descriptor limit (default: 65536)\
  --disk-quota SIZE     Disk quota for inbox (default: 50G, requires XFS quotas)' "$PROVISION_SCRIPT"

# Step 3: Add resource limit variables in main() function
echo "[4/9] Adding resource limit variables to main()..."
sed -i '/local task_id=""/a\
\
    # Resource limit options (NEW - defaults calculated later)\
    local mem_limit=""\
    local mem_soft=""\
    local cpu_quota=""\
    local tasks_max="2048"\
    local io_weight="500"\
    local io_read_limit="500M"\
    local io_write_limit="200M"\
    local nofile_limit="65536"\
    local disk_quota="50G"' "$PROVISION_SCRIPT"

# Step 4: Add argument parsing for resource limit options
echo "[5/9] Adding resource limit argument parsing..."
sed -i '/--management)/,/shift 2/a\
            ;;\
            --mem-limit)\
                mem_limit="$2"\
                shift 2\
            ;;\
            --mem-soft)\
                mem_soft="$2"\
                shift 2\
            ;;\
            --cpu-quota)\
                cpu_quota="$2"\
                shift 2\
            ;;\
            --tasks-max)\
                tasks_max="$2"\
                shift 2\
            ;;\
            --io-weight)\
                io_weight="$2"\
                shift 2\
            ;;\
            --io-read-limit)\
                io_read_limit="$2"\
                shift 2\
            ;;\
            --io-write-limit)\
                io_write_limit="$2"\
                shift 2\
            ;;\
            --nofile-limit)\
                nofile_limit="$2"\
                shift 2\
            ;;\
            --disk-quota)\
                disk_quota="$2"\
                shift 2' "$PROVISION_SCRIPT"

# Step 5: Update define_vm() function parameters
echo "[6/9] Updating define_vm() function signature..."
sed -i '/^define_vm() {$/,/local outbox_path=/ {
    /local outbox_path=/a\
    # Resource limit parameters (NEW)\
    local mem_limit_mb="${11:-$memory_mb}"\
    local cpu_quota_pct="${12:-$((cpus * 100))}"\
    local io_weight="${13:-500}"\
    local io_read_bps="${14:-524288000}"\
    local io_write_bps="${15:-209715200}"
}' "$PROVISION_SCRIPT"

# Step 6: Add libvirt XML resource tuning elements in define_vm()
echo "[7/9] Adding libvirt XML resource tuning elements..."
sed -i '/<vcpu placement=.static.>\$cpus<\/vcpu>\$memoryBacking/a\
\
  <!-- Resource limits: prevent VM from exhausting host resources -->\
  <memtune>\
    <hard_limit unit='"'"'MiB'"'"'>$(($mem_limit_mb + 256))</hard_limit>\
    <soft_limit unit='"'"'MiB'"'"'>$mem_limit_mb</soft_limit>\
  </memtune>\
\
  <!-- CPU tuning: fair scheduling and quota enforcement -->\
  <cputune>\
    <shares>$(($cpus * 1024))</shares>\
    <period>100000</period>\
    <quota>$(($cpu_quota_pct * 1000))</quota>\
  </cputune>\
\
  <!-- Block I/O tuning: prevent storage abuse -->\
  <blkiotune>\
    <weight>$io_weight</weight>\
    <device>\
      <path>/dev/vda</path>\
      <read_bytes_sec>$io_read_bps</read_bytes_sec>\
      <write_bytes_sec>$io_write_bps</write_bytes_sec>\
    </device>\
  </blkiotune>' "$PROVISION_SCRIPT"

# Step 7: Add resource calculation before define_vm call
echo "[8/9] Adding resource limit calculation invocation..."
sed -i '/log_info "Defining VM in libvirt..."/i\
    # Calculate resource limits for libvirt XML\
    local limits\
    limits=$(calculate_resource_limits "$cpus" "$memory_mb" "$mem_limit" "$mem_soft" "$cpu_quota" "$io_read_limit" "$io_write_limit")\
    read -r mem_limit_mb mem_soft_mb cpu_quota_pct io_read_bps io_write_bps <<< "$limits"\
' "$PROVISION_SCRIPT"

# Step 8: Update define_vm call to pass resource parameters
echo "[9/9] Updating define_vm() call with resource parameters..."
sed -i 's/xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path")/xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path" "$mem_limit_mb" "$cpu_quota_pct" "$io_weight" "$io_read_bps" "$io_write_bps")/' "$PROVISION_SCRIPT"

echo ""
echo "✓ Patch applied successfully!"
echo ""
echo "Backup saved to: $BACKUP_FILE"
echo ""
echo "Test the changes:"
echo "  ./images/qemu/provision-vm.sh --help  # Check new options"
echo "  ./images/qemu/provision-vm.sh --cpus 4 --memory 8G --dry-run test-vm"
echo ""
echo "To revert:"
echo "  mv $BACKUP_FILE $PROVISION_SCRIPT"
echo ""
