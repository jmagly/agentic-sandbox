//! `loadout` read-only verbs.
//!
//! Backing routes:
//! - `GET /api/v1/loadouts`             ← `loadout list`
//! - `GET /api/v1/loadouts/{name}`      ← `loadout get <name>`
//! - `GET /api/v1/loadout/registry`     ← `loadout registry`

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

pub async fn list(c: &HttpClient, as_json: bool) -> Result<()> {
    let v: Value = c.get_value("/api/v1/loadouts").await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("loadouts")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|l| {
                vec![
                    jstr(l, "name", "").to_string(),
                    jstr(l, "description", "-").to_string(),
                    jstr(l, "category", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "DESCRIPTION", "CATEGORY"], &rows)
    })
}

pub async fn get(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/loadouts/{}", name)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", "").to_string()),
            ("description", jstr(&v, "description", "-").to_string()),
            ("category", jstr(&v, "category", "-").to_string()),
            ("complexity", jstr(&v, "complexity", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

pub async fn registry(c: &HttpClient, as_json: bool) -> Result<()> {
    let v: Value = c.get_value("/api/v1/loadout/registry").await?;
    super::emit(&v, as_json, || {
        // The registry is structured; render as JSON in human mode too —
        // there's no compact tabular form that captures the tree.
        serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
    })
}
