//! Linux workload identity separation for managed containers.
//!
//! The long-lived agent control process may retain only `CAP_SETUID` and
//! `CAP_SETGID`. Every workload child changes to the configured numeric
//! identity and clears all inherited, permitted, effective, and ambient
//! capabilities before it executes user-controlled code.

use std::env;
use std::io;

use tokio::process::Command;

const WORKLOAD_UID_ENV: &str = "AGENT_WORKLOAD_UID";
const WORKLOAD_GID_ENV: &str = "AGENT_WORKLOAD_GID";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkloadIdentity {
    pub uid: u32,
    pub gid: u32,
}

impl WorkloadIdentity {
    pub fn from_values(uid: Option<&str>, gid: Option<&str>) -> io::Result<Option<Self>> {
        match (uid.map(str::trim), gid.map(str::trim)) {
            (None, None) | (Some(""), Some("")) => Ok(None),
            (Some(uid), Some(gid)) if !uid.is_empty() && !gid.is_empty() => {
                let uid = uid.parse::<u32>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "AGENT_WORKLOAD_UID must be numeric",
                    )
                })?;
                let gid = gid.parse::<u32>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "AGENT_WORKLOAD_GID must be numeric",
                    )
                })?;
                if uid == 0 || gid == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "managed workload uid/gid must be non-zero",
                    ));
                }
                Ok(Some(Self { uid, gid }))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AGENT_WORKLOAD_UID and AGENT_WORKLOAD_GID must be configured together",
            )),
        }
    }

    pub fn from_env() -> io::Result<Option<Self>> {
        let uid = env::var(WORKLOAD_UID_ENV).ok();
        let gid = env::var(WORKLOAD_GID_ENV).ok();
        Self::from_values(uid.as_deref(), gid.as_deref())
    }

    pub fn permits_run_as(self, run_as: &str) -> bool {
        run_as.trim().is_empty() || run_as.trim() == self.uid.to_string()
    }
}

/// Configure a Tokio command to enter the workload identity immediately
/// before exec. The closure uses only async-signal-safe libc calls.
pub fn configure_command(command: &mut Command) -> io::Result<Option<WorkloadIdentity>> {
    let Some(identity) = WorkloadIdentity::from_env()? else {
        return Ok(None);
    };
    unsafe {
        command.pre_exec(move || apply_in_child(identity));
    }
    Ok(Some(identity))
}

#[cfg(target_os = "linux")]
pub fn apply_in_child(identity: WorkloadIdentity) -> io::Result<()> {
    unsafe {
        if libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        if libc::setgroups(0, std::ptr::null()) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::setresgid(identity.gid, identity.gid, identity.gid) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::setresuid(identity.uid, identity.uid, identity.uid) != 0 {
            return Err(io::Error::last_os_error());
        }
        clear_capabilities()?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn apply_in_child(_identity: WorkloadIdentity) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "workload identity separation is supported only by Linux managed runtimes",
    ))
}

#[cfg(target_os = "linux")]
unsafe fn clear_capabilities() -> io::Result<()> {
    #[repr(C)]
    struct CapHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }

    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
    let mut header = CapHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [CapData {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    if libc::syscall(libc::SYS_capset, &mut header, data.as_mut_ptr()) != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_a_complete_non_root_numeric_identity() {
        assert_eq!(WorkloadIdentity::from_values(None, None).unwrap(), None);
        assert!(WorkloadIdentity::from_values(Some("10001"), None).is_err());
        assert!(WorkloadIdentity::from_values(Some("root"), Some("10001")).is_err());
        assert!(WorkloadIdentity::from_values(Some("0"), Some("10001")).is_err());
        assert_eq!(
            WorkloadIdentity::from_values(Some("10001"), Some("10001")).unwrap(),
            Some(WorkloadIdentity {
                uid: 10001,
                gid: 10001
            })
        );
    }

    #[test]
    fn separated_mode_rejects_run_as_bypass() {
        let identity = WorkloadIdentity {
            uid: 10001,
            gid: 10001,
        };
        assert!(identity.permits_run_as(""));
        assert!(identity.permits_run_as("10001"));
        assert!(!identity.permits_run_as("root"));
        assert!(!identity.permits_run_as("200001"));
    }
}
