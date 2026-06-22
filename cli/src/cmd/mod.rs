//! Verb implementations for the new noun-first taxonomy.
//!
//! One module per resource group. Verbs operate on
//! `client::http::HttpClient` and render via `output::{table,kv}`.
//! `--json` is honored at the verb level by passing through the raw
//! server response with `serde_json::to_string_pretty`.

pub mod agent;
pub mod container;
pub mod event;
pub mod health;
pub mod hitl;
pub mod loadout;
pub mod ops;
pub mod session;
pub mod ssh;
pub mod storage;
pub mod task;
pub mod tui;
pub mod vm;

use anyhow::Result;
use std::time::Duration;

/// Print a JSON value pretty when `as_json` is true; otherwise call the
/// human renderer. Returns `Ok(())` so verbs can `?` it.
pub fn emit(
    value: &serde_json::Value,
    as_json: bool,
    human: impl FnOnce() -> String,
) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        print!("{}", human());
    }
    Ok(())
}

/// Compose a path + query string from `(key, value)` pairs. Empty `q`
/// returns `path` unchanged. Used by every list verb that takes filters.
pub fn with_query(path: &str, q: &[(String, String)]) -> String {
    if q.is_empty() {
        return path.to_string();
    }
    let qs: String = q
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", path, qs)
}

/// Minimal application/x-www-form-urlencoded encoder (no extra dep).
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Gate destructive verbs.
///
/// - `--yes` (`yes == true`) → proceed unconditionally.
/// - stdin is a TTY → prompt the operator; only `y`/`Y`/`yes` proceeds.
/// - non-TTY without `--yes` → error. This is intentional: scripts
///   that destroy state must opt in explicitly to avoid a stale `cron`
///   job nuking a VM after a typo upstream.
pub fn confirm_destructive(verb: &str, target: &str, yes: bool) -> Result<()> {
    use std::io::IsTerminal;
    confirm_destructive_inner(verb, target, yes, std::io::stdin().is_terminal())
}

/// TTY-injected core. Split out so unit tests can exercise both
/// branches without depending on the runner's stdin state — `act`
/// attaches a pty to job containers, which used to make the non-TTY
/// branch hang on `read_line`.
fn confirm_destructive_inner(
    verb: &str,
    target: &str,
    yes: bool,
    stdin_is_tty: bool,
) -> Result<()> {
    use std::io::Write;
    if yes {
        return Ok(());
    }
    if !stdin_is_tty {
        anyhow::bail!(
            "refusing destructive verb `{verb}` on `{target}` in non-interactive \
             mode without --yes; pass --yes to proceed"
        );
    }
    print!("About to {verb} `{target}`. Continue? [y/N] ");
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let answer = buf.trim().to_ascii_lowercase();
    if matches!(answer.as_str(), "y" | "yes") {
        Ok(())
    } else {
        anyhow::bail!("aborted by operator");
    }
}

/// Apply duration parser for `--since` and `--watch` style flags.
/// Accepts `500ms`, `30s`, `5m`, `2h`, `1d`. Bare numbers are treated as
/// seconds. Returns `Duration`.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }
    // Try the two-letter suffix `ms` first; otherwise fall back to the
    // single-letter table.
    if let Some(num) = s.strip_suffix("ms") {
        let n: u64 = num
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration: {}", s))?;
        return Ok(Duration::from_millis(n));
    }
    let (n_str, unit) = match s.chars().last().unwrap() {
        'a'..='z' => (&s[..s.len() - 1], &s[s.len() - 1..]),
        _ => (s, "s"),
    };
    let n: u64 = n_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration: {}", s))?;
    Ok(match unit {
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n * 60),
        "h" => Duration::from_secs(n * 3600),
        "d" => Duration::from_secs(n * 86400),
        u => anyhow::bail!("unknown duration unit: {}", u),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_destructive_short_circuits_on_yes() {
        assert!(confirm_destructive("destroy", "test-vm", true).is_ok());
    }

    #[test]
    fn confirm_destructive_refuses_non_tty_without_yes() {
        // Calls the inner helper directly so the result doesn't depend on
        // the test runner's stdin state. (Gitea's act runner attaches a
        // pty to job containers, which would otherwise make this hang.)
        let r =
            confirm_destructive_inner("destroy", "test-vm", false, /*stdin_is_tty=*/ false);
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("non-interactive"));
        assert!(msg.contains("--yes"));
    }

    #[test]
    fn parse_duration_accepts_units() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    }

    #[test]
    fn parse_duration_rejects_bad_input() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("xyz").is_err());
        assert!(parse_duration("5w").is_err());
    }
}
