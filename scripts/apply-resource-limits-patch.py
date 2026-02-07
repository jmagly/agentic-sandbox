#!/usr/bin/env python3
"""
Apply resource limit patch to provision-vm.sh

This script makes surgical modifications to provision-vm.sh to add:
1. CLI options for resource limits (--mem-limit, --cpu-quota, etc.)
2. Helper functions for size parsing and limit calculation
3. libvirt XML resource tuning elements (memtune, cputune, blkiotune)
4. Parameter passing through provisioning flow

Reference: docs/security/resource-quota-design.md
Gitea Issue: #86
"""

import re
import sys
from pathlib import Path
from datetime import datetime


def create_backup(file_path: Path) -> Path:
    """Create timestamped backup of file"""
    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    backup_path = file_path.with_suffix(f".sh.backup-{timestamp}")
    backup_path.write_text(file_path.read_text())
    return backup_path


def add_helper_functions(lines: list[str]) -> list[str]:
    """Add resource limit helper functions before provision_vm()"""

    # Find line with "# Provision a VM"
    for i, line in enumerate(lines):
        if line.strip() == "# Provision a VM":
            # Insert helper functions before this comment
            helper_funcs = '''# Parse size string (8G, 512M, etc.) to bytes
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
            echo "$number"
            ;;
    esac
}

# Parse size string to MiB
parse_size_to_mb() {
    local bytes=$(parse_size_to_bytes "$1")
    echo $((bytes / 1024 / 1024))
}

# Calculate resource limits from user options or defaults
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

    # I/O limits: convert to bytes/sec
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

    echo "$mem_limit_mb $mem_soft_mb $cpu_quota_pct $io_read_bps $io_write_bps"
}

'''
            lines.insert(i, helper_funcs)
            break

    return lines


def update_usage_docs(lines: list[str]) -> list[str]:
    """Add resource limit documentation to usage() function"""

    for i, line in enumerate(lines):
        if line.strip() == "-h, --help            Show this help":
            # Insert resource limits documentation after this line
            docs = '''  -h, --help            Show this help

Resource Limits (cgroup v2 and libvirt tuning):
  --mem-limit SIZE      Memory hard limit for agent service (default: auto from --memory)
  --mem-soft SIZE       Memory soft limit (default: 75% of --mem-limit)
  --cpu-quota PERCENT   CPU quota as percentage (default: cpus * 100%)
  --tasks-max NUM       Maximum PIDs/tasks (default: 2048)
  --io-weight NUM       I/O scheduling weight 100-10000 (default: 500)
  --io-read-limit SIZE  Read bandwidth limit (default: 500M)
  --io-write-limit SIZE Write bandwidth limit (default: 200M)
  --nofile-limit NUM    File descriptor limit (default: 65536)
  --disk-quota SIZE     Disk quota for inbox (default: 50G, requires XFS quotas)'''

            lines[i] = docs
            break

    return lines


def add_main_variables(lines: list[str]) -> list[str]:
    """Add resource limit variables in main() function"""

    for i, line in enumerate(lines):
        if line.strip() == 'local task_id=""':
            # Insert resource limit variables after this line
            vars_block = '''    local task_id=""

    # Resource limit options (defaults calculated later)
    local mem_limit=""
    local mem_soft=""
    local cpu_quota=""
    local tasks_max="2048"
    local io_weight="500"
    local io_read_limit="500M"
    local io_write_limit="200M"
    local nofile_limit="65536"
    local disk_quota="50G"'''

            lines[i] = vars_block
            break

    return lines


def add_argument_parsing(lines: list[str]) -> list[str]:
    """Add resource limit argument parsing in main()"""

    for i, line in enumerate(lines):
        if "--management)" in line:
            # Find the end of this case block (shift 2)
            j = i
            while j < len(lines) and "shift 2" not in lines[j]:
                j += 1

            # Insert new cases after this block
            new_cases = '''                shift 2
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
                disk_quota="$2"'''

            if j < len(lines):
                lines[j] = new_cases
            break

    return lines


def update_define_vm_params(lines: list[str]) -> list[str]:
    """Add resource limit parameters to define_vm() function"""

    for i, line in enumerate(lines):
        if 'local outbox_path="${10:-}"' in line:
            # Add new parameters after outbox_path
            new_params = '''    local outbox_path="${10:-}"
    # Resource limit parameters (NEW)
    local mem_limit_mb="${11:-$memory_mb}"
    local cpu_quota_pct="${12:-$((cpus * 100))}"
    local io_weight="${13:-500}"
    local io_read_bps="${14:-524288000}"
    local io_write_bps="${15:-209715200}"'''

            lines[i] = new_params
            break

    return lines


def add_libvirt_tuning_xml(lines: list[str]) -> list[str]:
    """Add libvirt XML resource tuning elements"""

    for i, line in enumerate(lines):
        if "<vcpu placement='static'>$cpus</vcpu>$memoryBacking" in line:
            # Add tuning elements after this line
            tuning_xml = '''  <vcpu placement='static'>$cpus</vcpu>$memoryBacking

  <!-- Resource limits: prevent VM from exhausting host resources -->
  <memtune>
    <hard_limit unit='MiB'>$(($mem_limit_mb + 256))</hard_limit>
    <soft_limit unit='MiB'>$mem_limit_mb</soft_limit>
  </memtune>

  <!-- CPU tuning: fair scheduling and quota enforcement -->
  <cputune>
    <shares>$(($cpus * 1024))</shares>
    <period>100000</period>
    <quota>$(($cpu_quota_pct * 1000))</quota>
  </cputune>

  <!-- Block I/O tuning: prevent storage abuse -->
  <blkiotune>
    <weight>$io_weight</weight>
    <device>
      <path>/dev/vda</path>
      <read_bytes_sec>$io_read_bps</read_bytes_sec>
      <write_bytes_sec>$io_write_bps</write_bytes_sec>
    </device>
  </blkiotune>'''

            lines[i] = tuning_xml
            break

    return lines


def add_resource_calculation(lines: list[str]) -> list[str]:
    """Add resource calculation before define_vm call"""

    for i, line in enumerate(lines):
        if 'log_info "Defining VM in libvirt..."' in line:
            # Insert calculation before this line
            calc_code = '''    # Calculate resource limits for libvirt XML
    local limits
    limits=$(calculate_resource_limits "$cpus" "$memory_mb" "$mem_limit" "$mem_soft" "$cpu_quota" "$io_read_limit" "$io_write_limit")
    read -r mem_limit_mb mem_soft_mb cpu_quota_pct io_read_bps io_write_bps <<< "$limits"

    log_info "Defining VM in libvirt..."'''

            lines[i] = calc_code
            break

    return lines


def update_define_vm_call(lines: list[str]) -> list[str]:
    """Update define_vm() call to pass resource parameters"""

    for i, line in enumerate(lines):
        if 'xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path")' in line:
            # Replace with new call
            new_call = '''    xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path" "$mem_limit_mb" "$cpu_quota_pct" "$io_weight" "$io_read_bps" "$io_write_bps")'''

            lines[i] = new_call
            break

    return lines


def main():
    script_dir = Path(__file__).parent
    provision_script = script_dir.parent / "images" / "qemu" / "provision-vm.sh"

    if not provision_script.exists():
        print(f"Error: {provision_script} not found", file=sys.stderr)
        return 1

    print(f"Applying resource limit patch to {provision_script}")
    print("=" * 60)

    # Create backup
    print("\n[1/9] Creating backup...")
    backup_path = create_backup(provision_script)
    print(f"      Backup: {backup_path}")

    # Read file
    lines = provision_script.read_text().splitlines(keepends=True)

    # Apply transformations
    print("\n[2/9] Adding helper functions...")
    lines = add_helper_functions(lines)

    print("[3/9] Updating usage documentation...")
    lines = update_usage_docs(lines)

    print("[4/9] Adding main() variables...")
    lines = add_main_variables(lines)

    print("[5/9] Adding argument parsing...")
    lines = add_argument_parsing(lines)

    print("[6/9] Updating define_vm() parameters...")
    lines = update_define_vm_params(lines)

    print("[7/9] Adding libvirt XML tuning...")
    lines = add_libvirt_tuning_xml(lines)

    print("[8/9] Adding resource calculation...")
    lines = add_resource_calculation(lines)

    print("[9/9] Updating define_vm() call...")
    lines = update_define_vm_call(lines)

    # Write modified file
    provision_script.write_text("".join(lines))

    print("\n" + "=" * 60)
    print("✓ Patch applied successfully!")
    print("\nTest the changes:")
    print(f"  {provision_script} --help")
    print(f"  {provision_script} --cpus 4 --memory 8G --dry-run test-vm")
    print("\nTo revert:")
    print(f"  mv {backup_path} {provision_script}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
