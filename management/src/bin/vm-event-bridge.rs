use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::Parser;
use parking_lot::Mutex;
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use virt::connect::Connect;
use virt::sys;

#[derive(Debug, Parser)]
#[command(
    name = "vm-event-bridge",
    about = "Forward libvirt VM lifecycle events to agentic-mgmt"
)]
struct Args {
    /// Management server base URL.
    #[arg(long)]
    management_url: Option<String>,

    /// libvirt connection URI.
    #[arg(long)]
    libvirt_uri: Option<String>,

    /// VM backend to observe: libvirt or cloud-hypervisor.
    #[arg(long)]
    backend: Option<String>,

    /// Cloud Hypervisor VM storage root.
    #[arg(long)]
    ch_vm_root: Option<PathBuf>,

    /// Cloud Hypervisor polling interval in seconds.
    #[arg(long, default_value_t = 2)]
    ch_poll_seconds: u64,

    /// Enable debug logging.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Clone)]
struct BridgeConfig {
    management_url: String,
    libvirt_uri: String,
    backend: VmBackend,
    ch_vm_root: PathBuf,
    ch_poll_interval: Duration,
}

impl BridgeConfig {
    fn from_args(args: Args) -> Self {
        let backend = args
            .backend
            .or_else(|| std::env::var("AGENTIC_BACKEND").ok())
            .unwrap_or_else(|| "libvirt".to_string())
            .parse()
            .expect("invalid VM event backend");

        Self {
            management_url: args
                .management_url
                .or_else(|| std::env::var("MANAGEMENT_URL").ok())
                .unwrap_or_else(|| "http://localhost:8122".to_string()),
            libvirt_uri: args
                .libvirt_uri
                .or_else(|| std::env::var("LIBVIRT_URI").ok())
                .unwrap_or_else(|| "qemu:///system".to_string()),
            backend,
            ch_vm_root: args
                .ch_vm_root
                .or_else(|| std::env::var_os("VM_STORAGE_DIR").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("/var/lib/agentic-sandbox/vms")),
            ch_poll_interval: Duration::from_secs(args.ch_poll_seconds.max(1)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VmBackend {
    Libvirt,
    CloudHypervisor,
}

impl std::str::FromStr for VmBackend {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "libvirt" => Ok(Self::Libvirt),
            "cloud-hypervisor" | "cloud_hypervisor" | "ch" => Ok(Self::CloudHypervisor),
            other => Err(anyhow!("unsupported VM event backend: {other}")),
        }
    }
}

#[derive(Debug, Serialize, PartialEq)]
struct BridgeEvent {
    event_type: String,
    vm_name: String,
    timestamp: String,
    details: HashMap<String, Value>,
    agent_id: String,
}

#[derive(Debug, Default)]
struct VmStartTimes {
    times: Mutex<HashMap<String, Instant>>,
}

impl VmStartTimes {
    fn record_start(&self, vm_name: &str) {
        self.times
            .lock()
            .insert(vm_name.to_string(), Instant::now());
    }

    fn get_uptime(&self, vm_name: &str) -> Option<i64> {
        self.times
            .lock()
            .remove(vm_name)
            .map(|start| start.elapsed().as_secs() as i64)
    }
}

#[derive(Debug)]
struct CallbackState {
    client: Client,
    events_url: String,
    start_times: VmStartTimes,
}

static CALLBACK_STATE: OnceLock<CallbackState> = OnceLock::new();

fn main() -> Result<()> {
    let args = Args::parse();
    init_logging(args.verbose)?;
    let config = BridgeConfig::from_args(args);

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("failed to create HTTP client")?;
    let events_url = format!(
        "{}/api/v1/events",
        config.management_url.trim_end_matches('/')
    );

    CALLBACK_STATE
        .set(CallbackState {
            client,
            events_url,
            start_times: VmStartTimes::default(),
        })
        .map_err(|_| anyhow!("callback state initialized more than once"))?;

    run(config)
}

fn init_logging(verbose: bool) -> Result<()> {
    let default_filter = if verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .try_init()
        .map_err(|error| anyhow!("failed to initialize logging: {error}"))
}

fn run(config: BridgeConfig) -> Result<()> {
    match config.backend {
        VmBackend::Libvirt => run_libvirt(config),
        VmBackend::CloudHypervisor => run_cloud_hypervisor(config),
    }
}

fn run_libvirt(config: BridgeConfig) -> Result<()> {
    virt::event::event_register_default_impl()
        .context("failed to register libvirt event implementation")?;

    let conn = Connect::open(Some(&config.libvirt_uri)).with_context(|| {
        format!(
            "failed to connect to libvirt at {}",
            config.libvirt_uri.as_str()
        )
    })?;
    info!(uri = %config.libvirt_uri, "connected to libvirt");

    let lifecycle_id = register_lifecycle_callback(&conn)?;
    let reboot_id = register_reboot_callback(&conn)?;
    info!(lifecycle_id, reboot_id, "registered libvirt callbacks");

    loop {
        if let Err(error) = virt::event::event_run_default_impl() {
            error!(%error, "libvirt event loop iteration failed");
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn run_cloud_hypervisor(config: BridgeConfig) -> Result<()> {
    let Some(state) = CALLBACK_STATE.get() else {
        return Err(anyhow!("callback state is not initialized"));
    };

    info!(
        vm_root = %config.ch_vm_root.display(),
        interval_secs = config.ch_poll_interval.as_secs(),
        "polling Cloud Hypervisor VM state"
    );

    let mut observed: HashMap<String, bool> = HashMap::new();
    loop {
        let current = scan_cloud_hypervisor_vms(&config.ch_vm_root);
        for event in reconcile_cloud_hypervisor_events(&mut observed, &state.start_times, current) {
            post_event(state, event);
        }

        std::thread::sleep(config.ch_poll_interval);
    }
}

fn reconcile_cloud_hypervisor_events(
    observed: &mut HashMap<String, bool>,
    start_times: &VmStartTimes,
    current: Vec<CloudHypervisorVm>,
) -> Vec<BridgeEvent> {
    let current_names: HashSet<String> = current.iter().map(|vm| vm.name.clone()).collect();
    let mut events = Vec::new();

    for vm in current {
        match observed.get(&vm.name).copied() {
            None => {
                events.push(build_event("vm.defined", &vm.name, vm.details()));
                if vm.running {
                    start_times.record_start(&vm.name);
                    let mut details = vm.details();
                    details.insert("reason".to_string(), json!("booted"));
                    events.push(build_event("vm.started", &vm.name, details));
                }
            }
            Some(false) if vm.running => {
                start_times.record_start(&vm.name);
                let mut details = vm.details();
                details.insert("reason".to_string(), json!("booted"));
                events.push(build_event("vm.started", &vm.name, details));
            }
            Some(true) if !vm.running => {
                let mut details = vm.details();
                details.insert("reason".to_string(), json!("shutdown"));
                if let Some(uptime_seconds) = start_times.get_uptime(&vm.name) {
                    details.insert("uptime_seconds".to_string(), json!(uptime_seconds));
                }
                events.push(build_event("vm.stopped", &vm.name, details));
            }
            _ => {}
        }
        observed.insert(vm.name, vm.running);
    }

    let removed: Vec<String> = observed
        .keys()
        .filter(|name| !current_names.contains(*name))
        .cloned()
        .collect();
    for name in removed {
        if observed.remove(&name).unwrap_or(false) {
            if let Some(uptime_seconds) = start_times.get_uptime(&name) {
                events.push(build_event(
                    "vm.stopped",
                    &name,
                    HashMap::from([
                        ("reason".to_string(), json!("destroyed")),
                        ("uptime_seconds".to_string(), json!(uptime_seconds)),
                    ]),
                ));
            }
        }
        events.push(build_event("vm.undefined", &name, HashMap::new()));
    }

    events
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloudHypervisorVm {
    name: String,
    running: bool,
    api_socket: Option<PathBuf>,
    pid: Option<u32>,
}

impl CloudHypervisorVm {
    fn details(&self) -> HashMap<String, Value> {
        let mut details = HashMap::new();
        details.insert("backend".to_string(), json!("cloud-hypervisor"));
        if let Some(pid) = self.pid {
            details.insert("pid".to_string(), json!(pid));
        }
        if let Some(api_socket) = &self.api_socket {
            details.insert(
                "api_socket".to_string(),
                json!(api_socket.to_string_lossy().to_string()),
            );
        }
        details
    }
}

fn scan_cloud_hypervisor_vms(vm_root: &Path) -> Vec<CloudHypervisorVm> {
    let Ok(entries) = fs::read_dir(vm_root) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| read_cloud_hypervisor_vm(&entry.path()))
        .collect()
}

fn read_cloud_hypervisor_vm(vm_dir: &Path) -> Option<CloudHypervisorVm> {
    let name = vm_dir.file_name()?.to_string_lossy().to_string();
    let ch_dir = vm_dir.join("cloud-hypervisor");
    let state_file = ch_dir.join("vm.env");
    if !state_file.is_file() {
        return None;
    }

    let env = parse_shell_env_file(&state_file);
    let pid_file = env
        .get("PID_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| ch_dir.join("pid"));
    let api_socket = env
        .get("API_SOCKET")
        .map(PathBuf::from)
        .or_else(|| Some(ch_dir.join("api.sock")));
    let pid = fs::read_to_string(pid_file)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let running = pid.is_some_and(process_is_running);

    Some(CloudHypervisorVm {
        name,
        running,
        api_socket,
        pid,
    })
}

fn parse_shell_env_file(path: &Path) -> HashMap<String, String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return HashMap::new();
    };

    contents
        .lines()
        .filter_map(|line| {
            let (key, raw_value) = line.split_once('=')?;
            Some((key.to_string(), unquote_shell_value(raw_value)))
        })
        .collect()
}

fn unquote_shell_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else {
        value.trim_matches('"').to_string()
    }
}

fn process_is_running(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

fn register_lifecycle_callback(conn: &Connect) -> Result<i32> {
    let callback: sys::virConnectDomainEventGenericCallback = unsafe {
        std::mem::transmute::<sys::virConnectDomainEventCallback, _>(Some(lifecycle_callback))
    };

    let id = unsafe {
        sys::virConnectDomainEventRegisterAny(
            conn.as_ptr(),
            std::ptr::null_mut(),
            sys::VIR_DOMAIN_EVENT_ID_LIFECYCLE as i32,
            callback,
            std::ptr::null_mut(),
            None,
        )
    };
    if id < 0 {
        return Err(anyhow!("failed to register libvirt lifecycle callback"));
    }
    Ok(id)
}

fn register_reboot_callback(conn: &Connect) -> Result<i32> {
    let id = unsafe {
        sys::virConnectDomainEventRegisterAny(
            conn.as_ptr(),
            std::ptr::null_mut(),
            sys::VIR_DOMAIN_EVENT_ID_REBOOT as i32,
            Some(reboot_callback),
            std::ptr::null_mut(),
            None,
        )
    };
    if id < 0 {
        return Err(anyhow!("failed to register libvirt reboot callback"));
    }
    Ok(id)
}

unsafe extern "C" fn lifecycle_callback(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    event: libc::c_int,
    detail: libc::c_int,
    _opaque: *mut libc::c_void,
) -> libc::c_int {
    let Some(vm_name) = domain_name(dom) else {
        return 0;
    };

    let Some(state) = CALLBACK_STATE.get() else {
        return 0;
    };

    let bridge_event = lifecycle_event_payload(state, &vm_name, event, detail);
    post_event(state, bridge_event);
    0
}

unsafe extern "C" fn reboot_callback(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _opaque: *mut libc::c_void,
) {
    let Some(vm_name) = domain_name(dom) else {
        return;
    };

    let Some(state) = CALLBACK_STATE.get() else {
        return;
    };

    let mut details = HashMap::new();
    details.insert("reason".to_string(), json!("reboot"));
    post_event(state, build_event("vm.rebooted", &vm_name, details));
}

unsafe fn domain_name(dom: sys::virDomainPtr) -> Option<String> {
    let name_ptr = sys::virDomainGetName(dom);
    if name_ptr.is_null() {
        return None;
    }
    CStr::from_ptr(name_ptr).to_str().ok().map(str::to_string)
}

fn lifecycle_event_payload(
    state: &CallbackState,
    vm_name: &str,
    event: libc::c_int,
    detail: libc::c_int,
) -> BridgeEvent {
    let mut event_type = lifecycle_event_type(event);
    let mut details = HashMap::new();

    match event as u32 {
        sys::VIR_DOMAIN_EVENT_STARTED => {
            state.start_times.record_start(vm_name);
            details.insert("reason".to_string(), json!(started_reason(detail)));
        }
        sys::VIR_DOMAIN_EVENT_STOPPED => {
            let reason = stopped_reason(detail);
            if reason == "crashed" {
                event_type = "vm.crashed";
            }
            details.insert("reason".to_string(), json!(reason));
            if let Some(uptime_seconds) = state.start_times.get_uptime(vm_name) {
                details.insert("uptime_seconds".to_string(), json!(uptime_seconds));
            }
        }
        sys::VIR_DOMAIN_EVENT_CRASHED => {
            details.insert("reason".to_string(), json!("crashed"));
            if let Some(uptime_seconds) = state.start_times.get_uptime(vm_name) {
                details.insert("uptime_seconds".to_string(), json!(uptime_seconds));
            }
        }
        _ => {}
    }

    build_event(event_type, vm_name, details)
}

fn build_event(event_type: &str, vm_name: &str, details: HashMap<String, Value>) -> BridgeEvent {
    BridgeEvent {
        event_type: event_type.to_string(),
        vm_name: vm_name.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        details,
        agent_id: vm_name.to_string(),
    }
}

fn post_event(state: &CallbackState, event: BridgeEvent) {
    debug!(event_type = %event.event_type, vm = %event.vm_name, "posting VM event");

    match state.client.post(&state.events_url).json(&event).send() {
        Ok(response) if response.status().is_success() => {
            info!(event_type = %event.event_type, vm = %event.vm_name, "posted VM event");
        }
        Ok(response) => {
            warn!(
                status = %response.status(),
                event_type = %event.event_type,
                vm = %event.vm_name,
                "management server rejected VM event"
            );
        }
        Err(error) => {
            error!(
                %error,
                event_type = %event.event_type,
                vm = %event.vm_name,
                "failed to post VM event"
            );
        }
    }
}

fn lifecycle_event_type(event: libc::c_int) -> &'static str {
    match event as u32 {
        sys::VIR_DOMAIN_EVENT_DEFINED => "vm.defined",
        sys::VIR_DOMAIN_EVENT_UNDEFINED => "vm.undefined",
        sys::VIR_DOMAIN_EVENT_STARTED => "vm.started",
        sys::VIR_DOMAIN_EVENT_SUSPENDED => "vm.suspended",
        sys::VIR_DOMAIN_EVENT_RESUMED => "vm.resumed",
        sys::VIR_DOMAIN_EVENT_STOPPED => "vm.stopped",
        sys::VIR_DOMAIN_EVENT_SHUTDOWN => "vm.shutdown",
        sys::VIR_DOMAIN_EVENT_PMSUSPENDED => "vm.pmsuspended",
        sys::VIR_DOMAIN_EVENT_CRASHED => "vm.crashed",
        _ => "vm.unknown",
    }
}

fn stopped_reason(detail: libc::c_int) -> &'static str {
    match detail as u32 {
        sys::VIR_DOMAIN_EVENT_STOPPED_SHUTDOWN => "shutdown",
        sys::VIR_DOMAIN_EVENT_STOPPED_DESTROYED => "destroyed",
        sys::VIR_DOMAIN_EVENT_STOPPED_CRASHED => "crashed",
        sys::VIR_DOMAIN_EVENT_STOPPED_MIGRATED => "migrated",
        sys::VIR_DOMAIN_EVENT_STOPPED_SAVED => "saved",
        sys::VIR_DOMAIN_EVENT_STOPPED_FAILED => "failed",
        sys::VIR_DOMAIN_EVENT_STOPPED_FROM_SNAPSHOT => "from_snapshot",
        _ => "unknown",
    }
}

fn started_reason(detail: libc::c_int) -> &'static str {
    match detail as u32 {
        sys::VIR_DOMAIN_EVENT_STARTED_BOOTED => "booted",
        sys::VIR_DOMAIN_EVENT_STARTED_MIGRATED => "migrated",
        sys::VIR_DOMAIN_EVENT_STARTED_RESTORED => "restored",
        sys::VIR_DOMAIN_EVENT_STARTED_FROM_SNAPSHOT => "from_snapshot",
        sys::VIR_DOMAIN_EVENT_STARTED_WAKEUP => "wakeup",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callback_state() -> CallbackState {
        CallbackState {
            client: Client::builder().build().unwrap(),
            events_url: "http://localhost:8122/api/v1/events".to_string(),
            start_times: VmStartTimes::default(),
        }
    }

    #[test]
    fn maps_lifecycle_event_types() {
        assert_eq!(
            lifecycle_event_type(sys::VIR_DOMAIN_EVENT_STARTED as i32),
            "vm.started"
        );
        assert_eq!(
            lifecycle_event_type(sys::VIR_DOMAIN_EVENT_PMSUSPENDED as i32),
            "vm.pmsuspended"
        );
        assert_eq!(lifecycle_event_type(999), "vm.unknown");
    }

    #[test]
    fn maps_event_detail_reasons() {
        assert_eq!(
            started_reason(sys::VIR_DOMAIN_EVENT_STARTED_BOOTED as i32),
            "booted"
        );
        assert_eq!(
            stopped_reason(sys::VIR_DOMAIN_EVENT_STOPPED_FROM_SNAPSHOT as i32),
            "from_snapshot"
        );
        assert_eq!(stopped_reason(999), "unknown");
    }

    #[test]
    fn preserves_started_payload_contract() {
        let state = callback_state();
        let payload = lifecycle_event_payload(
            &state,
            "agent-01",
            sys::VIR_DOMAIN_EVENT_STARTED as i32,
            sys::VIR_DOMAIN_EVENT_STARTED_BOOTED as i32,
        );

        assert_eq!(payload.event_type, "vm.started");
        assert_eq!(payload.vm_name, "agent-01");
        assert_eq!(payload.agent_id, "agent-01");
        assert_eq!(payload.details.get("reason"), Some(&json!("booted")));
    }

    #[test]
    fn converts_stopped_crashed_detail_to_crashed_event() {
        let state = callback_state();
        state.start_times.record_start("agent-01");

        let payload = lifecycle_event_payload(
            &state,
            "agent-01",
            sys::VIR_DOMAIN_EVENT_STOPPED as i32,
            sys::VIR_DOMAIN_EVENT_STOPPED_CRASHED as i32,
        );

        assert_eq!(payload.event_type, "vm.crashed");
        assert_eq!(payload.details.get("reason"), Some(&json!("crashed")));
        assert!(payload.details.contains_key("uptime_seconds"));
    }

    #[test]
    fn parses_cloud_hypervisor_shell_state() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("vm.env");
        std::fs::write(
            &state_file,
            "VM_NAME=agent-ch\nAPI_SOCKET=/tmp/api.sock\nPID_FILE='/tmp/pid file'\n",
        )
        .unwrap();

        let env = parse_shell_env_file(&state_file);
        assert_eq!(env.get("VM_NAME").map(String::as_str), Some("agent-ch"));
        assert_eq!(
            env.get("PID_FILE").map(String::as_str),
            Some("/tmp/pid file")
        );
    }

    #[test]
    fn reads_cloud_hypervisor_vm_state_as_stopped_without_live_pid() {
        let root = tempfile::tempdir().unwrap();
        let vm_dir = root.path().join("agentic-e2e-123");
        let ch_dir = vm_dir.join("cloud-hypervisor");
        std::fs::create_dir_all(&ch_dir).unwrap();
        let pid_file = ch_dir.join("pid");
        std::fs::write(&pid_file, "999999999").unwrap();
        std::fs::write(
            ch_dir.join("vm.env"),
            format!(
                "VM_NAME=agentic-e2e-123\nPID_FILE={}\nAPI_SOCKET={}\n",
                pid_file.display(),
                ch_dir.join("api.sock").display()
            ),
        )
        .unwrap();

        let vm = read_cloud_hypervisor_vm(&vm_dir).unwrap();
        assert_eq!(vm.name, "agentic-e2e-123");
        assert_eq!(vm.pid, Some(999999999));
        assert!(!vm.running);
        assert_eq!(
            vm.details().get("backend"),
            Some(&json!("cloud-hypervisor"))
        );
    }

    #[test]
    fn cloud_hypervisor_reconcile_emits_lifecycle_events() {
        let start_times = VmStartTimes::default();
        let mut observed = HashMap::new();

        let events = reconcile_cloud_hypervisor_events(
            &mut observed,
            &start_times,
            vec![CloudHypervisorVm {
                name: "agentic-e2e-123".to_string(),
                running: true,
                api_socket: Some(PathBuf::from("/tmp/ch.sock")),
                pid: Some(std::process::id()),
            }],
        );
        let event_types: Vec<&str> = events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect();
        assert_eq!(event_types, vec!["vm.defined", "vm.started"]);
        assert_eq!(
            events[0].details.get("backend"),
            Some(&json!("cloud-hypervisor"))
        );
        assert_eq!(events[1].details.get("reason"), Some(&json!("booted")));

        let events = reconcile_cloud_hypervisor_events(
            &mut observed,
            &start_times,
            vec![CloudHypervisorVm {
                name: "agentic-e2e-123".to_string(),
                running: false,
                api_socket: Some(PathBuf::from("/tmp/ch.sock")),
                pid: Some(999999999),
            }],
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "vm.stopped");
        assert_eq!(events[0].details.get("reason"), Some(&json!("shutdown")));

        let events = reconcile_cloud_hypervisor_events(&mut observed, &start_times, Vec::new());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "vm.undefined");
    }
}
