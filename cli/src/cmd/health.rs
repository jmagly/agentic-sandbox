//! `health` — diagnostic surface rollup.
//!
//! Backing routes:
//! - `GET /healthz`         (liveness)
//! - `GET /healthz/http`    (HTTP-only liveness, watchdog probe target)
//! - `GET /readyz`          (readiness)
//! - `GET /healthz/deep`    (detailed status)
//!
//! `health status` rolls all four into a unified table; non-zero exit
//! when any probe is failing. `health watchdog` is a thin alias for the
//! HTTP-only probe — server-side watchdog state (consecutive failures,
//! last probe time) is internal to the management process; surfacing it
//! over HTTP would need a new endpoint, deferred to a follow-up.

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::table;

pub async fn status(c: &HttpClient, as_json: bool) -> Result<()> {
    let probes = [
        ("liveness", "/healthz"),
        ("http", "/healthz/http"),
        ("readiness", "/readyz"),
        ("deep", "/healthz/deep"),
    ];
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut json_results = serde_json::Map::new();
    let mut all_healthy = true;
    for (name, path) in probes {
        let r = c.get_value(path).await;
        let (ok, body) = match r {
            Ok(v) => (true, v),
            Err(e) => (false, serde_json::json!({"error": e.to_string()})),
        };
        if !ok {
            all_healthy = false;
        }
        json_results.insert(
            name.into(),
            serde_json::json!({"healthy": ok, "body": body}),
        );
        rows.push(vec![
            name.into(),
            path.into(),
            if ok { "OK".into() } else { "FAIL".into() },
        ]);
    }
    let payload = Value::Object(json_results);
    super::emit(&payload, as_json, || {
        table::render(&["PROBE", "PATH", "STATUS"], &rows)
    })?;
    if !all_healthy {
        // Documented: non-zero exit when any probe is failing.
        return Err(anyhow::anyhow!("one or more probes are not healthy"));
    }
    Ok(())
}

pub async fn watchdog(c: &HttpClient, as_json: bool) -> Result<()> {
    let v: Value = c.get_value("/healthz/http").await?;
    super::emit(&v, as_json, || {
        format!(
            "watchdog probe target: /healthz/http\nstatus: {}\nnote: server-side counters \
             (consecutive failures, last probe time) are not yet exposed via REST; \
             see follow-up issue if you need them.\n",
            v.get("http").and_then(|x| x.as_str()).unwrap_or("?"),
        )
    })
}
