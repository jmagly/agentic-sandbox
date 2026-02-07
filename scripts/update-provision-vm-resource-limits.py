#!/usr/bin/env python3
"""
Update provision-vm.sh to add resource limit support

This script adds:
1. CLI options for resource limits
2. libvirt XML tuning elements (memtune, cputune, blkiotune)
3. Parameter passing through the provisioning flow
4. Updated usage documentation

Reference: docs/security/resource-quota-design.md
"""

import re
import sys
from pathlib import Path

def update_usage_function(content: str) -> str:
    """Add resource limit options to usage documentation"""

    # Find the usage function and insert resource limit documentation
    usage_pattern = r'(  --management HOST.*\n  -n, --dry-run.*\n  -h, --help.*\n)'

    resource_limits_doc = '''  --management HOST     Management server address (default: $MANAGEMENT_SERVER)
  -n, --dry-run         Show what would be done
  -h, --help            Show this help

Resource Limits (cgroup v2 and libvirt tuning):
  --mem-limit SIZE      Memory hard limit for agent service (default: auto from --memory)
                        Examples: 7680M, 7.5G (should be ~94% of --memory)
  --mem-soft SIZE       Memory soft limit (default: 75% of --mem-limit)
  --cpu-quota PERCENT   CPU quota as percentage (default: cpus * 100%)
                        Examples: 400 for 4 cores, 200 for 2 cores
  --tasks-max NUM       Maximum PIDs/tasks (default: 2048)
  --io-weight NUM       I/O scheduling weight 100-10000 (default: 500)
  --io-read-limit SIZE  Read bandwidth limit (default: 500M)
  --io-write-limit SIZE Write bandwidth limit (default: 200M)
  --nofile-limit NUM    File descriptor limit (default: 65536)
  --disk-quota SIZE     Disk quota for inbox (default: 50G, requires XFS quotas)

'''

    content = re.sub(usage_pattern, resource_limits_doc, content)

    # Add resource limit profile documentation
    examples_pattern = r'(Resource Guidelines.*?\n.*?\n.*?\n)'

    resource_profile_doc = '''Resource Guidelines (for concurrent VMs):
  Single VM:    --cpus 8 --memory 16G
  2 concurrent: --cpus 4 --memory 8G  (default)
  4 concurrent: --cpus 2 --memory 4G

Resource Limit Defaults (standard profile):
  Memory:  7.5GB hard / 6GB soft (for 8GB VM - auto-calculated)
  PIDs:    2048 tasks
  CPU:     400% quota (4 cores - matches --cpus)
  I/O:     500M read / 200M write
  FDs:     65536
  Disk:    50GB (if XFS quotas enabled)

'''

    content = re.sub(examples_pattern, resource_profile_doc, content)

    return content


def update_define_vm_function(content: str) -> str:
    """Add libvirt XML resource tuning elements to define_vm()"""

    # Find the define_vm function parameters
    param_pattern = r'(define_vm\(\) \{.*?local outbox_path="\$\{10:-\}")'

    new_params = '''define_vm() {
    local vm_name="$1"
    local disk_path="$2"
    local cloud_init_iso="$3"
    local cpus="$4"
    local memory_mb="$5"
    local network="$6"
    local mac_address="${7:-}"
    local use_agentshare="${8:-false}"
    local inbox_path="${9:-}"
    local outbox_path="${10:-}"
    # Resource limit parameters (NEW)
    local mem_limit_mb="${11:-$memory_mb}"
    local cpu_quota_pct="${12:-$((cpus * 100))}"
    local io_weight="${13:-500}"
    local io_read_bps="${14:-524288000}"   # 500MB/s in bytes
    local io_write_bps="${15:-209715200}"  # 200MB/s in bytes'''

    content = re.sub(param_pattern, new_params, content, flags=re.DOTALL)

    # Add resource tuning XML elements after <vcpu> and before <os>
    xml_pattern = r'(<domain type=.kvm.>.*?<vcpu placement=.static.>\$cpus</vcpu>)\$memoryBacking'

    resource_xml = r'''\1$memoryBacking

  <!-- Resource limits: prevent VM from exhausting host resources -->
  <memtune>
    <!-- Hard limit: OOM if exceeded (add 256MB buffer for kernel overhead) -->
    <hard_limit unit='MiB'>$(($mem_limit_mb + 256))</hard_limit>
    <!-- Soft limit: trigger memory reclaim before OOM -->
    <soft_limit unit='MiB'>$mem_limit_mb</soft_limit>
  </memtune>

  <!-- CPU tuning: fair scheduling and quota enforcement -->
  <cputune>
    <!-- CPU shares for fair scheduler (1024 per vCPU) -->
    <shares>$(($cpus * 1024))</shares>
    <!-- CPU quota: limit total CPU time (period=100ms, quota=period*cpus) -->
    <period>100000</period>
    <quota>$(($cpu_quota_pct * 1000))</quota>
  </cputune>

  <!-- Block I/O tuning: prevent storage abuse -->
  <blkiotune>
    <!-- I/O weight for proportional sharing (100-1000 range) -->
    <weight>$io_weight</weight>
    <!-- Per-device I/O bandwidth limits -->
    <device>
      <path>/dev/vda</path>
      <read_bytes_sec>$io_read_bps</read_bytes_sec>
      <write_bytes_sec>$io_write_bps</write_bytes_sec>
    </device>
  </blkiotune>'''

    content = re.sub(xml_pattern, resource_xml, content)

    return content


def update_main_function(content: str) -> str:
    """Add resource limit variables and argument parsing"""

    # Add default resource limit variables after existing defaults
    defaults_pattern = r'(local use_agentshare=false\n    local task_id="")'

    new_defaults = '''local use_agentshare=false
    local task_id=""

    # Resource limit options (NEW - defaults calculated later)
    local mem_limit=""
    local mem_soft=""
    local cpu_quota=""
    local tasks_max="2048"
    local io_weight="500"
    local io_read_limit="500M"
    local io_write_limit="200M"
    local nofile_limit="65536"
    local disk_quota="50G"'''

    content = re.sub(defaults_pattern, new_defaults, content)

    # Add argument parsing cases before the -h|--help case
    help_pattern = r'(            --management\)\n                MANAGEMENT_SERVER="\$2"\n                shift 2\n                ;;)'

    new_cases = '''            --management)
                MANAGEMENT_SERVER="$2"
                shift 2
                ;;
            --mem-limit)
                mem_limit="$2"
                shift 2
                ;;
            --mem-soft)
                mem_soft="$2"
                shift 2
                ;;
            --cpu-quota)
                cpu_quota="$2"
                shift 2
                ;;
            --tasks-max)
                tasks_max="$2"
                shift 2
                ;;
            --io-weight)
                io_weight="$2"
                shift 2
                ;;
            --io-read-limit)
                io_read_limit="$2"
                shift 2
                ;;
            --io-write-limit)
                io_write_limit="$2"
                shift 2
                ;;
            --nofile-limit)
                nofile_limit="$2"
                shift 2
                ;;
            --disk-quota)
                disk_quota="$2"
                shift 2
                ;;'''

    content = re.sub(help_pattern, new_cases, content)

    return content


def update_define_vm_call(content: str) -> str:
    """Update the define_vm() call to pass resource limit parameters"""

    # Find the define_vm call and add new parameters
    call_pattern = r'(xml_path=\$\(define_vm "\$vm_name" "\$disk_path" "\$cloud_init_iso" "\$cpus" "\$memory_mb" "\$network" "\$mac_address" "\$use_agentshare" "\$inbox_path" "\$outbox_path")\)'

    new_call = r'''\1 "$mem_limit_mb" "$cpu_quota_pct" "$io_weight" "$io_read_bps" "$io_write_bps")'''

    content = re.sub(call_pattern, new_call, content)

    return content


def add_resource_limit_calculation(content: str) -> str:
    """Add function to calculate resource limits from parsed options"""

    # Insert before the provision_vm function
    provision_pattern = r'(# Provision a VM\nprovision_vm\(\) \{)'

    calc_function = '''# Calculate resource limits in bytes/numbers for libvirt XML
# Takes human-readable inputs (8G, 500M, etc.) and VM parameters
calculate_resource_limits() {
    local vm_cpus="$1"
    local vm_memory_mb="$2"
    local user_mem_limit="$3"
    local user_mem_soft="$4"
    local user_cpu_quota="$5"
    local user_io_read="$6"
    local user_io_write="$7"

    # Memory limit: default to 94% of VM memory (leave 6% for kernel/systemd)
    if [[ -z "$user_mem_limit" ]]; then
        mem_limit_mb=$((vm_memory_mb * 94 / 100))
    else
        mem_limit_mb=$(parse_size_to_mb "$user_mem_limit")
    fi

    # Memory soft limit: default to 75% of hard limit
    if [[ -z "$user_mem_soft" ]]; then
        mem_soft_mb=$((mem_limit_mb * 75 / 100))
    else
        mem_soft_mb=$(parse_size_to_mb "$user_mem_soft")
    fi

    # CPU quota: default to cpus * 100% (400 for 4 cores)
    if [[ -z "$user_cpu_quota" ]]; then
        cpu_quota_pct=$((vm_cpus * 100))
    else
        cpu_quota_pct="$user_cpu_quota"
    fi

    # I/O limits: convert human-readable to bytes/sec
    if [[ -z "$user_io_read" ]]; then
        io_read_bps=524288000  # 500MB/s
    else
        io_read_bps=$(parse_size_to_bytes "$user_io_read")
    fi

    if [[ -z "$user_io_write" ]]; then
        io_write_bps=209715200  # 200MB/s
    else
        io_write_bps=$(parse_size_to_bytes "$user_io_write")
    fi

    # Export for caller
    echo "$mem_limit_mb $mem_soft_mb $cpu_quota_pct $io_read_bps $io_write_bps"
}

# Parse size string (8G, 512M, etc.) to bytes
parse_size_to_bytes() {
    local size_str="$1"
    local number="${size_str%[GMK]*}"
    local unit="${size_str##*[0-9]}"

    case "${unit^^}" in
        G|GB)
            echo $((number * 1024 * 1024 * 1024))
            ;;
        M|MB)
            echo $((number * 1024 * 1024))
            ;;
        K|KB)
            echo $((number * 1024))
            ;;
        *)
            echo "$number"  # Assume bytes
            ;;
    esac
}

# Parse size string to MiB
parse_size_to_mb() {
    local bytes=$(parse_size_to_bytes "$1")
    echo $((bytes / 1024 / 1024))
}

# Provision a VM
provision_vm() {'''

    content = re.sub(provision_pattern, calc_function, content)

    return content


def add_resource_limit_invocation(content: str) -> str:
    """Add call to calculate_resource_limits before define_vm"""

    # Find where we call define_vm and add calculation before it
    define_pattern = r'(    # Define VM with static MAC\n    log_info "Defining VM in libvirt\.\.\.")'

    calculation = '''    # Calculate resource limits for libvirt XML
    local limits
    limits=$(calculate_resource_limits "$cpus" "$memory_mb" "$mem_limit" "$mem_soft" "$cpu_quota" "$io_read_limit" "$io_write_limit")
    read -r mem_limit_mb mem_soft_mb cpu_quota_pct io_read_bps io_write_bps <<< "$limits"

    log_info "Resource limits: mem=${mem_limit_mb}M, cpu=${cpu_quota_pct}%, io=${io_read_bps}/${io_write_bps} bps"

    # Define VM with static MAC
    log_info "Defining VM in libvirt..."'''

    content = re.sub(define_pattern, calculation, content)

    return content


def main():
    script_path = Path(__file__).parent.parent / "images" / "qemu" / "provision-vm.sh"

    if not script_path.exists():
        print(f"Error: {script_path} not found", file=sys.stderr)
        sys.exit(1)

    print(f"Updating {script_path} with resource limit support...")

    # Read original content
    content = script_path.read_text()

    # Apply transformations
    print("  - Updating usage documentation...")
    content = update_usage_function(content)

    print("  - Adding resource limit calculation functions...")
    content = add_resource_limit_calculation(content)

    print("  - Updating main() function variables...")
    content = update_main_function(content)

    print("  - Updating define_vm() function...")
    content = update_define_vm_function(content)

    print("  - Adding resource limit calculation invocation...")
    content = add_resource_limit_invocation(content)

    print("  - Updating define_vm() call...")
    content = update_define_vm_call(content)

    # Write updated content
    output_path = script_path.with_suffix(".sh.updated")
    output_path.write_text(content)

    print(f"\nUpdated script written to: {output_path}")
    print("\nTo apply changes:")
    print(f"  diff -u {script_path} {output_path}")
    print(f"  mv {output_path} {script_path}")
    print("\nOr review and apply manually")

    return 0


if __name__ == "__main__":
    sys.exit(main())
