//! Verb implementations for the new noun-first taxonomy.
//!
//! One module per resource group. Verbs operate on
//! `client::http::HttpClient` and render via `output::{table,kv}`.
//! `--json` is honored at the verb level by passing through the raw
//! server response with `serde_json::to_string_pretty`.

pub mod agent;
pub mod event;
pub mod health;
pub mod loadout;
pub mod ops;
pub mod session;
pub mod task;
pub mod vm;

use anyhow::Result;
use std::time::Duration;

/// Print a JSON value pretty when `as_json` is true; otherwise call the
/// human renderer. Returns `Ok(())` so verbs can `?` it.
pub fn emit(value: &serde_json::Value, as_json: bool, human: impl FnOnce() -> String) -> Result<()> {
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

/// Apply duration parser for `--since` style flags. Accepts `30s`, `5m`,
/// `2h`, `1d`. Bare numbers are treated as seconds. Returns `Duration`.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }
    let (n_str, unit) = match s.chars().last().unwrap() {
        'a'..='z' => (&s[..s.len() - 1], &s[s.len() - 1..]),
        _ => (s, "s"),
    };
    let n: u64 = n_str.parse().map_err(|_| anyhow::anyhow!("invalid duration: {}", s))?;
    Ok(match unit {
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n * 60),
        "h" => Duration::from_secs(n * 3600),
        "d" => Duration::from_secs(n * 86400),
        u => anyhow::bail!("unknown duration unit: {}", u),
    })
}
