//! Client-side audit log — JSON Lines under `$XDG_STATE_HOME/sandboxctl/audit.log`.
//!
//! Two records per dispatched command: `intent` (before) and `outcome`
//! (after). The pair lets a forensic reader reconstruct what the operator
//! tried to do AND what happened, even if the outcome is missing because
//! the process was killed mid-call.
//!
//! Server-side audit is a separate concern (out of scope for #153). The
//! local log gives operators a record before that exists.

use anyhow::Result;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
struct Record<'a> {
    /// Wall-clock millis since epoch.
    ts_ms: u128,
    /// "intent" or "outcome".
    kind: &'a str,
    /// Active context (or "<none>").
    context: &'a str,
    /// Verb path, e.g. "vm list" or "config set-context".
    verb: &'a str,
    /// Free-form target identifier (e.g. VM name, agent id). May be empty.
    target: &'a str,
    /// `outcome` only: "ok" | "err" | "skipped".
    outcome: Option<&'a str>,
    /// `outcome` only: total wall-clock duration in milliseconds.
    duration_ms: Option<u128>,
    /// `outcome` only: short error string when applicable.
    err: Option<&'a str>,
}

/// Resolve the audit-log path. Errors are intentionally non-fatal in the
/// caller — never block real work because we couldn't write a log line.
pub fn audit_log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(base.join("sandboxctl").join("audit.log"))
}

fn append_record(rec: &Record<'_>) {
    let Some(path) = audit_log_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let line = match serde_json::to_string(rec) {
        Ok(s) => s,
        Err(_) => return,
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", line);
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Wrap a verb invocation with intent/outcome audit records.
///
/// Use:
/// ```ignore
/// let rec = audit::Span::new("vm list", "", "lab");
/// let r = run_verb().await;
/// rec.finish(&r);
/// ```
pub struct Span<'a> {
    verb: &'a str,
    target: &'a str,
    context: &'a str,
    started_ms: u128,
}

impl<'a> Span<'a> {
    pub fn new(verb: &'a str, target: &'a str, context: &'a str) -> Self {
        let started_ms = now_ms();
        append_record(&Record {
            ts_ms: started_ms,
            kind: "intent",
            context,
            verb,
            target,
            outcome: None,
            duration_ms: None,
            err: None,
        });
        Self {
            verb,
            target,
            context,
            started_ms,
        }
    }

    pub fn finish<T, E: std::fmt::Display>(self, result: &Result<T, E>) {
        let duration_ms = now_ms().saturating_sub(self.started_ms);
        let (outcome, err_str): (&str, Option<String>) = match result {
            Ok(_) => ("ok", None),
            Err(e) => ("err", Some(e.to_string())),
        };
        append_record(&Record {
            ts_ms: now_ms(),
            kind: "outcome",
            context: self.context,
            verb: self.verb,
            target: self.target,
            outcome: Some(outcome),
            duration_ms: Some(duration_ms),
            err: err_str.as_deref(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn span_writes_intent_and_outcome_records() {
        // Direct the audit log into a tempdir.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_STATE_HOME", dir.path());
        let path = audit_log_path().unwrap();
        // Record + finish on success.
        let span = Span::new("test verb", "tgt", "ctx");
        let r: Result<(), anyhow::Error> = Ok(());
        span.finish(&r);
        // Record + finish on err.
        let span = Span::new("test verb", "tgt2", "ctx");
        let r: Result<(), anyhow::Error> = Err(anyhow!("boom"));
        span.finish(&r);
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 4, "two pairs of intent+outcome");
        assert!(lines[0].contains("\"kind\":\"intent\""));
        assert!(lines[1].contains("\"kind\":\"outcome\""));
        assert!(lines[1].contains("\"outcome\":\"ok\""));
        assert!(lines[3].contains("\"outcome\":\"err\""));
        assert!(lines[3].contains("boom"));
    }
}
