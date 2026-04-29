//! `vm` read-only verbs.
//!
//! Backing routes:
//! - `GET /api/v1/vms`             ← `vm list` (`--state`, `--prefix`)
//! - `GET /api/v1/vms/{name}`      ← `vm get <name>`

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

pub async fn list(c: &HttpClient, state: Option<&str>, prefix: Option<&str>, as_json: bool) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = state {
        q.push(("state".into(), s.into()));
    }
    if let Some(p) = prefix {
        q.push(("prefix".into(), p.into()));
    }
    let path = super::with_query("/api/v1/vms", &q);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("vms")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|vm| {
                vec![
                    jstr(vm, "name", "").to_string(),
                    jstr(vm, "state", "").to_string(),
                    jstr(vm, "ip_address", "-").to_string(),
                    jstr(vm, "profile", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "STATE", "IP", "PROFILE"], &rows)
    })
}

pub async fn get(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/vms/{}", name)).await?;
    super::emit(&v, as_json, || {
        let pairs = vec![
            ("name", jstr(&v, "name", "").to_string()),
            ("state", jstr(&v, "state", "").to_string()),
            ("ip_address", jstr(&v, "ip_address", "-").to_string()),
            ("profile", jstr(&v, "profile", "-").to_string()),
            ("loadout", jstr(&v, "loadout", "-").to_string()),
            ("memory_mb", crate::output::jnum(&v, "memory_mb")),
            ("vcpus", crate::output::jnum(&v, "vcpus")),
        ];
        kv::render(&pairs.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_list_table_from_array_response() {
        let v: Value = serde_json::json!([
            {"name": "a", "state": "running", "ip_address": "1.1.1.1", "profile": "basic"},
            {"name": "bb", "state": "stopped", "ip_address": null, "profile": "agentic-dev"}
        ]);
        let arr = v.as_array().cloned().unwrap();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|vm| {
                vec![
                    jstr(vm, "name", "").to_string(),
                    jstr(vm, "state", "").to_string(),
                    jstr(vm, "ip_address", "-").to_string(),
                    jstr(vm, "profile", "-").to_string(),
                ]
            })
            .collect();
        let out = table::render(&["NAME", "STATE", "IP", "PROFILE"], &rows);
        assert!(out.contains("running"));
        assert!(out.contains("stopped"));
        assert!(out.contains("1.1.1.1"));
        // Null IP renders as "-".
        assert!(out.contains("-"));
    }

}
