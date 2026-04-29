//! Output renderers.
//!
//! Verbs declare a default human renderer (table or key:value). `--json`
//! and `--watch` are global flags applied at the dispatch boundary.
//! Streaming renderers (SSE / WS attach) live in their owning verb modules
//! since they need to drive the runtime, not just format a snapshot.

pub mod kv;
pub mod table;

/// Convenience: extract a string field from a JSON value, falling back to
/// `default` when the key is missing or non-string. Used by `cmd/*` to
/// render snapshot fields without redeclaring server response types.
pub fn jstr<'a>(v: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or(default)
}

/// Like `jstr` but for numeric fields rendered as a string.
pub fn jnum(v: &serde_json::Value, key: &str) -> String {
    match v.get(key) {
        Some(x) if x.is_i64() => x.as_i64().unwrap().to_string(),
        Some(x) if x.is_u64() => x.as_u64().unwrap().to_string(),
        Some(x) if x.is_f64() => format!("{:.2}", x.as_f64().unwrap()),
        Some(x) if x.is_string() => x.as_str().unwrap().to_string(),
        Some(x) if x.is_null() => "-".to_string(),
        Some(x) => x.to_string(),
        None => "-".to_string(),
    }
}
