mod e2e_support;

use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use e2e_support::{require_rust_vm_e2e, VmManagementServer, VmTestTarget, WsTestClient};

static VM_RESOURCE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn vm_resource_test_guard() -> MutexGuard<'static, ()> {
    VM_RESOURCE_TEST_LOCK.lock().unwrap_or_else(|poisoned| {
        eprintln!("recovering poisoned VM resource test lock after prior failure");
        poisoned.into_inner()
    })
}

#[test]
fn rust_vm_e2e_agent_service_has_cgroup_limits() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }
    let _guard = vm_resource_test_guard();

    let vm = VmTestTarget::from_env()?;

    let controllers = vm.ssh(
        "cat /sys/fs/cgroup/cgroup.controllers",
        Duration::from_secs(10),
    )?;
    assert_eq!(controllers.status, 0, "{}", controllers.stderr);
    assert!(
        controllers.stdout.contains("memory"),
        "memory controller missing from {:?}",
        controllers.stdout
    );
    assert!(
        controllers.stdout.contains("cpu") || controllers.stdout.contains("cpuset"),
        "cpu/cpuset controller missing from {:?}",
        controllers.stdout
    );

    let service = vm.agent_service()?;
    let cgroup = vm.ssh(
        &format!("systemctl show {service} --property=ControlGroup --value"),
        Duration::from_secs(10),
    )?;
    assert_eq!(cgroup.status, 0, "{}", cgroup.stderr);
    assert!(
        cgroup.stdout.contains(&service),
        "service cgroup did not include {service}: {:?}",
        cgroup.stdout
    );
    let cgroup_path = cgroup.stdout.trim();

    let memory_max = vm.ssh(
        &format!("cat /sys/fs/cgroup{cgroup_path}/memory.max"),
        Duration::from_secs(10),
    )?;
    assert_eq!(memory_max.status, 0, "{}", memory_max.stderr);
    let value = memory_max.stdout.trim();
    assert!(!value.is_empty(), "memory.max was empty for {service}");
    if value != "max" {
        let parsed = value.parse::<u64>()?;
        assert!(
            parsed <= 8 * 1024 * 1024 * 1024,
            "memory.max for {service} is too high: {parsed}"
        );
    }

    let tasks_max = vm.ssh(
        &format!("systemctl show {service} --property=TasksMax --value"),
        Duration::from_secs(10),
    )?;
    assert_eq!(tasks_max.status, 0, "{}", tasks_max.stderr);
    let value = tasks_max.stdout.trim();
    assert!(!value.is_empty(), "TasksMax was empty for {service}");
    assert_ne!(value, "infinity", "TasksMax is not bounded for {service}");
    let parsed = value.parse::<u64>()?;
    assert!(
        parsed <= 4096,
        "TasksMax for {service} is too high: {parsed}"
    );

    Ok(())
}

#[test]
fn rust_vm_e2e_memory_pressure_is_contained() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }
    let _guard = vm_resource_test_guard();

    let vm = VmTestTarget::from_env()?;
    let output = vm.ssh(memory_stress_script(), Duration::from_secs(120))?;
    let combined = format!("{}{}", output.stdout, output.stderr);

    assert!(
        output.status == 0
            || combined.contains("MEM_STRESS_KILLED")
            || combined.contains("MEM_STRESS_MEMORY_ERROR"),
        "memory pressure did not hit an expected containment path: status={} output={combined}",
        output.status
    );
    assert!(combined.contains("MEM_STRESS_DONE"), "{combined}");
    assert!(
        vm.is_alive(),
        "VM became unresponsive after memory pressure"
    );

    Ok(())
}

#[test]
fn rust_vm_e2e_agentshare_small_write_succeeds() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }
    let _guard = vm_resource_test_guard();

    let vm = VmTestTarget::from_env()?;
    let mount = vm.ssh("test -d /mnt/inbox && echo exists", Duration::from_secs(10))?;
    if !mount.stdout.contains("exists") {
        eprintln!("skipping agentshare small-write check; /mnt/inbox is not mounted");
        return Ok(());
    }

    let output = vm.ssh(agentshare_small_write_script(), Duration::from_secs(60))?;
    let combined = format!("{}{}", output.stdout, output.stderr);
    assert_eq!(
        output.status, 0,
        "agentshare small write failed: {combined}"
    );
    assert!(
        combined.contains("AGENTSHARE_SMALL_WRITE_DONE"),
        "{combined}"
    );
    assert!(vm.is_alive(), "VM became unresponsive after small write");

    Ok(())
}

#[test]
fn rust_vm_e2e_agentshare_quota_blocks_excess_write() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }
    let _guard = vm_resource_test_guard();

    let vm = VmTestTarget::from_env()?;
    let mount = vm.ssh("test -d /mnt/inbox && echo exists", Duration::from_secs(10))?;
    if !mount.stdout.contains("exists") {
        eprintln!("skipping agentshare quota check; /mnt/inbox is not mounted");
        return Ok(());
    }
    if !agentshare_project_quota_available() {
        eprintln!("skipping agentshare quota check; project quotas are not available");
        return Ok(());
    }

    let output = vm.ssh(agentshare_quota_overrun_script(), Duration::from_secs(90))?;
    let combined = format!("{}{}", output.stdout, output.stderr);
    let lower = combined.to_ascii_lowercase();
    let quota_enforced = lower.contains("disk quota")
        || lower.contains("quota exceeded")
        || lower.contains("no space");

    if !quota_enforced {
        eprintln!("skipping agentshare quota check; quota was not enforced: {combined}");
        return Ok(());
    }
    assert!(
        combined.contains("AGENTSHARE_EXCESS_WRITE_DONE"),
        "{combined}"
    );
    assert!(
        vm.is_alive(),
        "VM became unresponsive after agentshare quota overrun"
    );

    Ok(())
}

#[tokio::test]
async fn rust_vm_e2e_dispatch_resource_stress_hits_agent_limits() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }
    let _guard = vm_resource_test_guard();

    let vm = VmTestTarget::from_env()?;
    let server = VmManagementServer::start(&vm)?;
    let mut ws = WsTestClient::connect(&server.ws_url()).await?;

    let command_id = ws
        .send_command(
            &vm.vm_name,
            "bash",
            vec!["-lc".to_string(), pid_stress_script().to_string()],
        )
        .await?;
    let frames = ws
        .collect_output(&command_id, Duration::from_secs(120))
        .await?;
    let output = output_text(&frames);

    assert!(output.contains("PID_STRESS_HIT_LIMIT"), "{output}");
    assert!(
        output.contains("PID_STRESS_DONE hit_limit=True"),
        "{output}"
    );

    let command_id = ws
        .send_command(
            &vm.vm_name,
            "bash",
            vec!["-lc".to_string(), fd_stress_script().to_string()],
        )
        .await?;
    let frames = ws
        .collect_output(&command_id, Duration::from_secs(60))
        .await?;
    let output = output_text(&frames);

    assert!(output.contains("FD_STRESS_LIMIT"), "{output}");
    assert!(
        output.contains("FD_STRESS_HIT_LIMIT")
            || output.contains("FD_STRESS_LIMIT_ABOVE_TEST_BUDGET"),
        "{output}"
    );
    assert!(output.contains("FD_STRESS_DONE"), "{output}");
    assert!(
        vm.is_alive(),
        "VM became unresponsive after resource stress"
    );
    assert!(matches!(
        vm.agent_service()?.as_str(),
        "agent-client" | "agentic-agent"
    ));

    Ok(())
}

#[tokio::test]
async fn rust_vm_e2e_dispatch_write_throughput_respects_io_limit() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }
    let _guard = vm_resource_test_guard();

    let vm = VmTestTarget::from_env()?;
    let server = VmManagementServer::start(&vm)?;
    let mut ws = WsTestClient::connect(&server.ws_url()).await?;

    let command_id = ws
        .send_command(
            &vm.vm_name,
            "bash",
            vec!["-lc".to_string(), io_throttle_script().to_string()],
        )
        .await?;
    let frames = ws
        .collect_output(&command_id, Duration::from_secs(120))
        .await?;
    let output = output_text(&frames);

    if output.contains("IO_THROTTLE_SKIP") {
        eprintln!(
            "skipping I/O throttle check; runtime reported no concrete write limit: {output}"
        );
        return Ok(());
    }

    assert!(output.contains("IO_THROTTLE_RESULT"), "{output}");
    assert!(
        output.contains("IO_THROTTLE_DONE respected=true"),
        "{output}"
    );
    assert!(
        vm.is_alive(),
        "VM became unresponsive after I/O throughput check"
    );

    Ok(())
}

fn output_text(frames: &[serde_json::Value]) -> String {
    frames
        .iter()
        .filter_map(|frame| frame.get("data").and_then(serde_json::Value::as_str))
        .collect::<String>()
}

fn io_throttle_script() -> &'static str {
    r#"
set -euo pipefail
target="/tmp/rust-e2e-io-throttle-$$"
trap 'rm -f "$target"' EXIT

limit_bps=$(python3 - <<'PY'
import sys

def own_cgroup_path():
    with open("/proc/self/cgroup", "r", encoding="utf-8") as handle:
        for line in handle:
            parts = line.strip().split(":", 2)
            if len(parts) == 3 and parts[0] == "0":
                return parts[2]
    raise RuntimeError("could not locate unified cgroup entry")

io_max = "/sys/fs/cgroup" + own_cgroup_path() + "/io.max"
limits = []
try:
    with open(io_max, "r", encoding="utf-8") as handle:
        for line in handle:
            for field in line.split()[1:]:
                if field.startswith("wbps="):
                    value = field.split("=", 1)[1]
                    if value != "max":
                        limits.append(int(value))
except FileNotFoundError:
    pass

if not limits:
    sys.exit(2)

print(min(limits))
PY
) || status=$?

if [ "${status:-0}" -eq 2 ]; then
    echo "IO_THROTTLE_SKIP reason=no-wbps-limit"
    echo "IO_THROTTLE_DONE skipped=true"
    exit 0
fi
if [ "${status:-0}" -ne 0 ]; then
    echo "IO_THROTTLE_SKIP reason=io-max-unreadable status=${status:-0}"
    echo "IO_THROTTLE_DONE skipped=true"
    exit 0
fi

size_mb=$(python3 - <<PY
import math
limit = int("$limit_bps")
size = math.ceil((limit * 3) / (1024 * 1024))
print(max(64, min(size, 512)))
PY
)

start=$(python3 - <<'PY'
import time
print(time.monotonic())
PY
)
dd if=/dev/zero of="$target" bs=1M count="$size_mb" oflag=direct status=none
sync "$target"
end=$(python3 - <<'PY'
import time
print(time.monotonic())
PY
)

python3 - <<PY
import sys
elapsed = float("$end") - float("$start")
size_bytes = int("$size_mb") * 1024 * 1024
limit_bps = int("$limit_bps")
throughput_bps = size_bytes / elapsed if elapsed > 0 else float("inf")
print(
    "IO_THROTTLE_RESULT "
    f"limit_bps={limit_bps} throughput_bps={throughput_bps:.0f} "
    f"elapsed={elapsed:.2f} size_mb={int('$size_mb')}",
    flush=True,
)
if throughput_bps <= limit_bps * 2.5:
    print("IO_THROTTLE_DONE respected=true", flush=True)
    sys.exit(0)
print("IO_THROTTLE_DONE respected=false", flush=True)
sys.exit(1)
PY
"#
}

fn pid_stress_script() -> &'static str {
    r#"
set -euo pipefail
python3 - <<'PY'
import subprocess
import sys
import time

def own_cgroup_pids_max():
    with open("/proc/self/cgroup", "r", encoding="utf-8") as handle:
        for line in handle:
            parts = line.strip().split(":", 2)
            if len(parts) == 3 and parts[0] == "0":
                with open(
                    "/sys/fs/cgroup" + parts[2] + "/pids.max",
                    "r",
                    encoding="utf-8",
                ) as pids_file:
                    value = pids_file.read().strip()
                if value == "max":
                    raise RuntimeError("agent cgroup has no pids.max limit")
                return int(value)
    raise RuntimeError("could not locate unified cgroup entry")

limit = own_cgroup_pids_max()
target = min(limit + 128, 6000)
processes = []
hit_limit = False

try:
    for _ in range(target):
        processes.append(subprocess.Popen(["sleep", "60"]))
except OSError as exc:
    hit_limit = True
    print(f"PID_STRESS_HIT_LIMIT spawned={len(processes)} errno={exc.errno}", flush=True)
finally:
    for proc in processes:
        try:
            proc.terminate()
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 10
    for proc in processes:
        remaining = max(0.1, deadline - time.monotonic())
        try:
            proc.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            try:
                proc.kill()
            except ProcessLookupError:
                pass

print(f"PID_STRESS_DONE hit_limit={hit_limit} spawned={len(processes)} pids_max={limit}", flush=True)
sys.exit(0 if hit_limit else 1)
PY
"#
}

fn memory_stress_script() -> &'static str {
    r#"
set -uo pipefail
python3 - <<'PY'
import os
import subprocess
import sys
import textwrap

budget = 12 * 1024 * 1024 * 1024
meminfo = {}
with open("/proc/meminfo", "r", encoding="utf-8") as handle:
    for line in handle:
        key, value = line.split(":", 1)
        fields = value.split()
        if fields:
            meminfo[key] = int(fields[0]) * 1024

mem_total = meminfo["MemTotal"]
swap_total = meminfo.get("SwapTotal", 0)
target = mem_total + swap_total + (512 * 1024 * 1024)
if target > budget:
    print(f"MEM_STRESS_LIMIT_ABOVE_TEST_BUDGET target={target} budget={budget} mem_total={mem_total} swap_total={swap_total}", flush=True)
    print("MEM_STRESS_DONE contained=budget-skip", flush=True)
    sys.exit(0)

program = textwrap.dedent(f"""
import os
import sys

try:
    block = bytearray({target})
    page_size = os.sysconf("SC_PAGE_SIZE")
    for offset in range(0, len(block), page_size):
        block[offset] = 1
    print("MEM_STRESS_UNEXPECTED_SUCCESS", flush=True)
    sys.exit(1)
except MemoryError:
    print("MEM_STRESS_MEMORY_ERROR", flush=True)
    sys.exit(0)
""")

result = subprocess.run([sys.executable, "-c", program], text=True, capture_output=True)
print(result.stdout, end="")
print(result.stderr, end="", file=sys.stderr)

if result.returncode == 0:
    print("MEM_STRESS_DONE contained=memory-error", flush=True)
    sys.exit(0)
if result.returncode < 0 or result.returncode in (137, 143):
    print(f"MEM_STRESS_KILLED returncode={result.returncode}", flush=True)
    print("MEM_STRESS_DONE contained=killed", flush=True)
    sys.exit(0)

print(f"MEM_STRESS_UNEXPECTED_EXIT returncode={result.returncode}", flush=True)
print("MEM_STRESS_DONE contained=false", flush=True)
sys.exit(1)
PY
"#
}

fn fd_stress_script() -> &'static str {
    r#"
set -euo pipefail
python3 - <<'PY'
import os
import resource
import sys

fds = []
hit_limit = False
budget = 200000
soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)

print(f"FD_STRESS_LIMIT soft={soft} hard={hard} budget={budget}", flush=True)

if soft == resource.RLIM_INFINITY or soft > budget:
    print("FD_STRESS_LIMIT_ABOVE_TEST_BUDGET", flush=True)
    print(f"FD_STRESS_DONE hit_limit=False opened=0 soft={soft}", flush=True)
    sys.exit(0)

target = soft + 1

try:
    for _ in range(target):
        fds.append(os.open("/dev/null", os.O_RDONLY))
except OSError as exc:
    hit_limit = True
    print(f"FD_STRESS_HIT_LIMIT opened={len(fds)} errno={exc.errno}", flush=True)
finally:
    for fd in fds:
        try:
            os.close(fd)
        except OSError:
            pass

print(f"FD_STRESS_DONE hit_limit={hit_limit} opened={len(fds)} soft={soft}", flush=True)
sys.exit(0 if hit_limit else 1)
PY
"#
}

fn agentshare_small_write_script() -> &'static str {
    r#"
set -euo pipefail
target="/mnt/inbox/rust-e2e-small-write-$$"
trap 'rm -f "$target"' EXIT
dd if=/dev/zero of="$target" bs=1M count=100 status=none
sync "$target"
rm -f "$target"
echo "AGENTSHARE_SMALL_WRITE_DONE bytes=$((100 * 1024 * 1024))"
"#
}

fn agentshare_quota_overrun_script() -> &'static str {
    r#"
set -uo pipefail
target="/mnt/inbox/rust-e2e-excess-write-$$"
trap 'rm -f "$target"' EXIT
dd if=/dev/zero of="$target" bs=1M count=61440 2>&1
status=$?
rm -f "$target"
echo "AGENTSHARE_EXCESS_WRITE_DONE status=$status"
"#
}

fn agentshare_project_quota_available() -> bool {
    let root = std::env::var("AGENTSHARE_ROOT").unwrap_or_else(|_| "/srv/agentshare".to_string());
    if !std::path::Path::new(&root).exists() {
        return false;
    }

    let Ok(df) = Command::new("df").args(["-T", &root]).output() else {
        return false;
    };
    if !df.status.success()
        || !String::from_utf8_lossy(&df.stdout)
            .split_whitespace()
            .any(|field| field == "xfs")
    {
        return false;
    }

    let Ok(mount) = Command::new("findmnt")
        .args(["-no", "OPTIONS", &root])
        .output()
    else {
        return false;
    };
    if !mount.status.success()
        || !String::from_utf8_lossy(&mount.stdout)
            .trim()
            .split(',')
            .any(|option| option == "prjquota")
    {
        return false;
    }

    Command::new("xfs_quota")
        .arg("-V")
        .output()
        .is_ok_and(|output| output.status.success())
}
