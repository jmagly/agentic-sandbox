//! libvirt VM lifecycle event monitoring
//!
//! Monitors libvirt for VM lifecycle events (start, stop, crash, etc.)
//! and integrates with the management server's event system.

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use virt::connect::Connect;
use virt::sys;

/// VM lifecycle event types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VmEventType {
    Defined,
    Undefined,
    Started,
    Suspended,
    Resumed,
    Stopped,
    Shutdown,
    Crashed,
    Rebooted,
    PmSuspended,
    Unknown(i32),
}

impl std::fmt::Display for VmEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmEventType::Defined => write!(f, "vm.defined"),
            VmEventType::Undefined => write!(f, "vm.undefined"),
            VmEventType::Started => write!(f, "vm.started"),
            VmEventType::Suspended => write!(f, "vm.suspended"),
            VmEventType::Resumed => write!(f, "vm.resumed"),
            VmEventType::Stopped => write!(f, "vm.stopped"),
            VmEventType::Shutdown => write!(f, "vm.shutdown"),
            VmEventType::Crashed => write!(f, "vm.crashed"),
            VmEventType::Rebooted => write!(f, "vm.rebooted"),
            VmEventType::PmSuspended => write!(f, "vm.pmsuspended"),
            VmEventType::Unknown(code) => write!(f, "vm.unknown_{}", code),
        }
    }
}

impl VmEventType {
    fn from_raw(event: i32) -> Self {
        match event as u32 {
            sys::VIR_DOMAIN_EVENT_DEFINED => VmEventType::Defined,
            sys::VIR_DOMAIN_EVENT_UNDEFINED => VmEventType::Undefined,
            sys::VIR_DOMAIN_EVENT_STARTED => VmEventType::Started,
            sys::VIR_DOMAIN_EVENT_SUSPENDED => VmEventType::Suspended,
            sys::VIR_DOMAIN_EVENT_RESUMED => VmEventType::Resumed,
            sys::VIR_DOMAIN_EVENT_STOPPED => VmEventType::Stopped,
            sys::VIR_DOMAIN_EVENT_SHUTDOWN => VmEventType::Shutdown,
            sys::VIR_DOMAIN_EVENT_PMSUSPENDED => VmEventType::PmSuspended,
            sys::VIR_DOMAIN_EVENT_CRASHED => VmEventType::Crashed,
            _ => VmEventType::Unknown(event),
        }
    }
}

/// Stopped event detail
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoppedReason {
    Shutdown,
    Destroyed,
    Crashed,
    Migrated,
    Saved,
    Failed,
    FromSnapshot,
    Unknown(i32),
}

impl StoppedReason {
    fn from_raw(detail: i32) -> Self {
        match detail as u32 {
            sys::VIR_DOMAIN_EVENT_STOPPED_SHUTDOWN => StoppedReason::Shutdown,
            sys::VIR_DOMAIN_EVENT_STOPPED_DESTROYED => StoppedReason::Destroyed,
            sys::VIR_DOMAIN_EVENT_STOPPED_CRASHED => StoppedReason::Crashed,
            sys::VIR_DOMAIN_EVENT_STOPPED_MIGRATED => StoppedReason::Migrated,
            sys::VIR_DOMAIN_EVENT_STOPPED_SAVED => StoppedReason::Saved,
            sys::VIR_DOMAIN_EVENT_STOPPED_FAILED => StoppedReason::Failed,
            sys::VIR_DOMAIN_EVENT_STOPPED_FROM_SNAPSHOT => StoppedReason::FromSnapshot,
            _ => StoppedReason::Unknown(detail),
        }
    }
}

impl std::fmt::Display for StoppedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoppedReason::Shutdown => write!(f, "shutdown"),
            StoppedReason::Destroyed => write!(f, "destroyed"),
            StoppedReason::Crashed => write!(f, "crashed"),
            StoppedReason::Migrated => write!(f, "migrated"),
            StoppedReason::Saved => write!(f, "saved"),
            StoppedReason::Failed => write!(f, "failed"),
            StoppedReason::FromSnapshot => write!(f, "from_snapshot"),
            StoppedReason::Unknown(code) => write!(f, "unknown_{}", code),
        }
    }
}

/// Started event detail
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartedReason {
    Booted,
    Migrated,
    Restored,
    FromSnapshot,
    Wakeup,
    Unknown(i32),
}

impl StartedReason {
    fn from_raw(detail: i32) -> Self {
        match detail as u32 {
            sys::VIR_DOMAIN_EVENT_STARTED_BOOTED => StartedReason::Booted,
            sys::VIR_DOMAIN_EVENT_STARTED_MIGRATED => StartedReason::Migrated,
            sys::VIR_DOMAIN_EVENT_STARTED_RESTORED => StartedReason::Restored,
            sys::VIR_DOMAIN_EVENT_STARTED_FROM_SNAPSHOT => StartedReason::FromSnapshot,
            sys::VIR_DOMAIN_EVENT_STARTED_WAKEUP => StartedReason::Wakeup,
            _ => StartedReason::Unknown(detail),
        }
    }
}

impl std::fmt::Display for StartedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartedReason::Booted => write!(f, "booted"),
            StartedReason::Migrated => write!(f, "migrated"),
            StartedReason::Restored => write!(f, "restored"),
            StartedReason::FromSnapshot => write!(f, "from_snapshot"),
            StartedReason::Wakeup => write!(f, "wakeup"),
            StartedReason::Unknown(code) => write!(f, "unknown_{}", code),
        }
    }
}

/// VM lifecycle event
#[derive(Debug, Clone)]
pub struct VmEvent {
    pub event_type: VmEventType,
    pub vm_name: String,
    pub timestamp: DateTime<Utc>,
    pub reason: Option<String>,
    pub uptime_seconds: Option<i64>,
}

/// VM start time tracker for uptime calculation
struct VmStartTimes {
    times: RwLock<HashMap<String, Instant>>,
}

impl Default for VmStartTimes {
    fn default() -> Self {
        Self {
            times: RwLock::new(HashMap::new()),
        }
    }
}

impl VmStartTimes {
    fn record_start(&self, vm_name: &str) {
        self.times
            .write()
            .insert(vm_name.to_string(), Instant::now());
    }

    fn get_uptime(&self, vm_name: &str) -> Option<i64> {
        self.times
            .write()
            .remove(vm_name)
            .map(|start| start.elapsed().as_secs() as i64)
    }
}

/// libvirt event monitor configuration
#[derive(Debug, Clone)]
pub struct LibvirtMonitorConfig {
    /// libvirt connection URI
    pub uri: String,
    /// Reconnect delay on connection failure
    pub reconnect_delay: Duration,
    /// VM name filter (only monitor VMs matching this prefix)
    pub vm_prefix: Option<String>,
}

impl Default for LibvirtMonitorConfig {
    fn default() -> Self {
        Self {
            uri: "qemu:///system".to_string(),
            reconnect_delay: Duration::from_secs(5),
            vm_prefix: Some("agent-".to_string()),
        }
    }
}

/// Shared state for the callback
struct CallbackState {
    event_tx: mpsc::Sender<VmEvent>,
    start_times: Arc<VmStartTimes>,
    vm_prefix: Option<String>,
}

// Global state for the callback (libvirt callbacks don't support closures well)
static CALLBACK_STATE: std::sync::OnceLock<std::sync::Mutex<Option<CallbackState>>> =
    std::sync::OnceLock::new();

fn get_callback_state() -> &'static std::sync::Mutex<Option<CallbackState>> {
    CALLBACK_STATE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Lifecycle event callback (called by libvirt from C)
unsafe extern "C" fn lifecycle_callback(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    event: libc::c_int,
    detail: libc::c_int,
    _opaque: *mut libc::c_void,
) -> libc::c_int {
    // Get domain name
    let name_ptr = sys::virDomainGetName(dom);
    if name_ptr.is_null() {
        return 0;
    }

    let vm_name = match CStr::from_ptr(name_ptr).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };

    // Get callback state
    let state_guard = get_callback_state().lock().unwrap();
    let Some(state) = state_guard.as_ref() else {
        return 0;
    };

    // Filter by prefix if configured
    if let Some(ref prefix) = state.vm_prefix {
        if !vm_name.starts_with(prefix) {
            return 0;
        }
    }

    let event_type = VmEventType::from_raw(event);
    let mut reason: Option<String> = None;
    let mut uptime_seconds: Option<i64> = None;

    // Handle event-specific details
    match event_type {
        VmEventType::Started => {
            state.start_times.record_start(&vm_name);
            reason = Some(StartedReason::from_raw(detail).to_string());
        }
        VmEventType::Stopped => {
            uptime_seconds = state.start_times.get_uptime(&vm_name);
            reason = Some(StoppedReason::from_raw(detail).to_string());
        }
        VmEventType::Crashed => {
            uptime_seconds = state.start_times.get_uptime(&vm_name);
            reason = Some("crashed".to_string());
        }
        _ => {}
    }

    // Determine final event type (stopped+crashed -> crashed)
    let final_event_type = if event_type == VmEventType::Stopped {
        if StoppedReason::from_raw(detail) == StoppedReason::Crashed {
            VmEventType::Crashed
        } else {
            event_type
        }
    } else {
        event_type
    };

    let vm_event = VmEvent {
        event_type: final_event_type.clone(),
        vm_name: vm_name.clone(),
        timestamp: Utc::now(),
        reason,
        uptime_seconds,
    };

    // Log the event
    match &final_event_type {
        VmEventType::Crashed => {
            warn!(
                vm = %vm_name,
                event = %final_event_type,
                uptime = ?uptime_seconds,
                "VM crashed"
            );
        }
        _ => {
            info!(
                vm = %vm_name,
                event = %final_event_type,
                "VM lifecycle event"
            );
        }
    }

    // Send event (non-blocking)
    if let Err(e) = state.event_tx.try_send(vm_event) {
        warn!(error = %e, "Failed to send VM event");
    }

    0
}

/// libvirt event monitor
pub struct LibvirtMonitor {
    config: LibvirtMonitorConfig,
    event_tx: mpsc::Sender<VmEvent>,
    start_times: Arc<VmStartTimes>,
}

impl LibvirtMonitor {
    /// Create a new libvirt monitor
    pub fn new(config: LibvirtMonitorConfig, event_tx: mpsc::Sender<VmEvent>) -> Self {
        Self {
            config,
            event_tx,
            start_times: Arc::new(VmStartTimes::default()),
        }
    }

    /// Run the event monitor (blocking)
    pub fn run(&self) {
        info!(uri = %self.config.uri, "Starting libvirt event monitor");

        // Initialize libvirt event loop
        if let Err(e) = virt::event::event_register_default_impl() {
            error!(error = %e, "Failed to register libvirt event implementation");
            return;
        }

        loop {
            match self.run_event_loop() {
                Ok(()) => {
                    info!("libvirt event loop exited normally");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "libvirt event loop error, reconnecting...");
                    std::thread::sleep(self.config.reconnect_delay);
                }
            }
        }
    }

    fn run_event_loop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Connect to libvirt
        let conn = Connect::open(Some(&self.config.uri))?;
        info!("Connected to libvirt");

        // Set up callback state
        {
            let mut state = get_callback_state().lock().unwrap();
            *state = Some(CallbackState {
                event_tx: self.event_tx.clone(),
                start_times: self.start_times.clone(),
                vm_prefix: self.config.vm_prefix.clone(),
            });
        }

        // Register lifecycle callback using raw FFI
        // For lifecycle events, the callback signature is:
        //   fn(conn, dom, event, detail, opaque) -> c_int
        // We cast to the generic callback type that libvirt expects
        let callback: sys::virConnectDomainEventGenericCallback = unsafe {
            std::mem::transmute::<
                unsafe extern "C" fn(
                    sys::virConnectPtr,
                    sys::virDomainPtr,
                    libc::c_int,
                    libc::c_int,
                    *mut libc::c_void,
                ) -> libc::c_int,
                sys::virConnectDomainEventGenericCallback,
            >(lifecycle_callback)
        };

        let callback_id = unsafe {
            sys::virConnectDomainEventRegisterAny(
                conn.as_ptr(),
                std::ptr::null_mut(), // All domains
                sys::VIR_DOMAIN_EVENT_ID_LIFECYCLE as i32,
                callback,
                std::ptr::null_mut(),
                None,
            )
        };

        if callback_id < 0 {
            return Err("Failed to register domain event callback".into());
        }

        info!(callback_id, "Registered libvirt lifecycle callback");

        // Run event loop
        loop {
            if let Err(e) = virt::event::event_run_default_impl() {
                error!(error = %e, "Event loop iteration failed");
                break;
            }
        }

        // Cleanup
        unsafe {
            sys::virConnectDomainEventDeregisterAny(conn.as_ptr(), callback_id);
        }

        Ok(())
    }
}

/// Spawn the libvirt monitor as a background task
pub fn spawn_libvirt_monitor(
    config: LibvirtMonitorConfig,
) -> (mpsc::Receiver<VmEvent>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(256);
    let monitor = LibvirtMonitor::new(config, tx);

    // Run in a blocking task since libvirt uses blocking I/O
    let handle = tokio::task::spawn_blocking(move || {
        monitor.run();
    });

    (rx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_display() {
        assert_eq!(VmEventType::Started.to_string(), "vm.started");
        assert_eq!(VmEventType::Crashed.to_string(), "vm.crashed");
        assert_eq!(VmEventType::Unknown(99).to_string(), "vm.unknown_99");
    }

    #[test]
    fn test_stopped_reason_display() {
        assert_eq!(StoppedReason::Shutdown.to_string(), "shutdown");
        assert_eq!(StoppedReason::Crashed.to_string(), "crashed");
        assert_eq!(StoppedReason::Unknown(99).to_string(), "unknown_99");
    }

    #[test]
    fn test_started_reason_display() {
        assert_eq!(StartedReason::Booted.to_string(), "booted");
        assert_eq!(StartedReason::Restored.to_string(), "restored");
    }

    #[test]
    fn test_vm_start_times() {
        let tracker = VmStartTimes::default();

        tracker.record_start("test-vm");
        std::thread::sleep(Duration::from_millis(100));

        let uptime = tracker.get_uptime("test-vm");
        assert!(uptime.is_some());
        assert!(uptime.unwrap() >= 0);

        // Second call should return None (already removed)
        assert!(tracker.get_uptime("test-vm").is_none());
    }

    #[test]
    fn test_config_default() {
        let config = LibvirtMonitorConfig::default();
        assert_eq!(config.uri, "qemu:///system");
        assert_eq!(config.vm_prefix, Some("agent-".to_string()));
    }
}
