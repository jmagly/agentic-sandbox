use std::collections::HashMap;
use std::ffi::CStr;
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

    /// Enable debug logging.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Clone)]
struct BridgeConfig {
    management_url: String,
    libvirt_uri: String,
}

impl BridgeConfig {
    fn from_args(args: Args) -> Self {
        Self {
            management_url: args
                .management_url
                .or_else(|| std::env::var("MANAGEMENT_URL").ok())
                .unwrap_or_else(|| "http://localhost:8122".to_string()),
            libvirt_uri: args
                .libvirt_uri
                .or_else(|| std::env::var("LIBVIRT_URI").ok())
                .unwrap_or_else(|| "qemu:///system".to_string()),
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
}
