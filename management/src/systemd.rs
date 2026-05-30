//! Minimal systemd notify/watchdog integration for the management server.

use std::time::Duration;

use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy)]
pub struct SystemdWatchdog {
    enabled: bool,
    interval: Duration,
}

impl SystemdWatchdog {
    pub fn new() -> Self {
        let mut usec = 0;
        let enabled = sd_notify::watchdog_enabled(false, &mut usec);

        if enabled && usec > 0 {
            let configured = Duration::from_micros(usec);
            let interval = (configured / 2).max(Duration::from_secs(1));
            info!(
                watchdog_usec = usec,
                ping_interval_secs = interval.as_secs_f64(),
                "systemd watchdog enabled"
            );
            Self {
                enabled: true,
                interval,
            }
        } else if enabled {
            warn!("systemd watchdog enabled without WATCHDOG_USEC; using 15s ping interval");
            Self {
                enabled: true,
                interval: Duration::from_secs(15),
            }
        } else {
            debug!("systemd watchdog not enabled");
            Self {
                enabled: false,
                interval: Duration::from_secs(15),
            }
        }
    }

    pub fn notify_ready(&self) -> Result<(), String> {
        if std::env::var_os("NOTIFY_SOCKET").is_none() {
            debug!("NOTIFY_SOCKET absent; skipping systemd READY notification");
            return Ok(());
        }

        sd_notify::notify(
            true,
            &[
                sd_notify::NotifyState::Ready,
                sd_notify::NotifyState::Status("agentic-mgmt gRPC listener is ready"),
            ],
        )
        .map_err(|e| format!("failed to notify systemd READY: {e}"))?;
        info!("sent systemd READY notification");
        Ok(())
    }

    fn ping(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog])
            .map_err(|e| format!("failed to notify systemd watchdog: {e}"))?;
        debug!("sent systemd WATCHDOG notification");
        Ok(())
    }

    pub fn spawn_ping_loop(self) {
        if !self.enabled {
            return;
        }

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                if let Err(e) = self.ping() {
                    warn!(error = %e, "systemd watchdog ping failed");
                }
            }
        });
    }

    #[cfg(test)]
    fn from_parts(enabled: bool, interval: Duration) -> Self {
        Self { enabled, interval }
    }
}

impl Default for SystemdWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_watchdog_is_noop() {
        let watchdog = SystemdWatchdog::from_parts(false, Duration::from_secs(15));
        assert!(watchdog.ping().is_ok());
    }

    #[test]
    fn interval_has_minimum_floor() {
        let configured = Duration::from_micros(1);
        let interval = (configured / 2).max(Duration::from_secs(1));
        assert_eq!(interval, Duration::from_secs(1));
    }

    #[test]
    fn packaged_unit_enables_notify_watchdog_and_limits() {
        let unit = include_str!("../systemd/agentic-mgmt.service");
        for expected in [
            "Type=notify",
            "NotifyAccess=main",
            "WatchdogSec=30",
            "KillMode=mixed",
            "LimitNOFILE=1048576",
        ] {
            assert!(
                unit.contains(expected),
                "agentic-mgmt.service missing {expected}"
            );
        }
    }
}
