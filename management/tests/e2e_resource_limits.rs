mod e2e_support;

use std::time::Duration;

use e2e_support::{require_rust_vm_e2e, VmManagementServer, VmTestTarget, WsTestClient};

#[test]
fn rust_vm_e2e_agent_service_has_cgroup_limits() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }

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

#[tokio::test]
async fn rust_vm_e2e_dispatch_pid_stress_hits_tasks_max() -> anyhow::Result<()> {
    if !require_rust_vm_e2e() {
        return Ok(());
    }

    let vm = VmTestTarget::from_env()?;
    let server = VmManagementServer::start(&vm)?;
    let mut ws = WsTestClient::connect(&server.ws_url()).await?;
    ws.subscribe(&vm.vm_name).await?;

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
    let output = frames
        .iter()
        .filter_map(|frame| frame.get("data").and_then(serde_json::Value::as_str))
        .collect::<String>();

    assert!(output.contains("PID_STRESS_HIT_LIMIT"), "{output}");
    assert!(
        output.contains("PID_STRESS_DONE hit_limit=True"),
        "{output}"
    );
    assert!(vm.is_alive(), "VM became unresponsive after PID stress");
    assert!(matches!(
        vm.agent_service()?.as_str(),
        "agent-client" | "agentic-agent"
    ));

    Ok(())
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
