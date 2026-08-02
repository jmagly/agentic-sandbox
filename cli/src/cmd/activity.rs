//! Correlated `activity.event/v1` timeline, coverage, and signed export verbs.

use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

use crate::client::http::HttpClient;
use crate::output::{jstr, table};

#[derive(Debug)]
pub struct Scope<'a> {
    pub tenant: &'a str,
    pub host: &'a str,
    pub instance: &'a str,
    pub agent: &'a str,
    pub collector: Option<&'a str>,
}

impl Scope<'_> {
    fn headers(&self) -> Vec<(&'static str, &str)> {
        let mut headers = vec![
            ("x-agentic-tenant-id", self.tenant),
            ("x-agentic-host-id", self.host),
            ("x-agentic-instance-id", self.instance),
            ("x-agentic-agent-id", self.agent),
        ];
        if let Some(collector) = self.collector {
            headers.push(("x-agentic-collector-id", collector));
        }
        headers
    }
}

pub async fn timeline(
    client: &HttpClient,
    scope: &Scope<'_>,
    query: &[(String, String)],
    as_json: bool,
) -> Result<()> {
    let path = super::with_query("/api/v2/activity/timeline", query);
    let value = client
        .get_value_with_headers(&path, &scope.headers())
        .await?;
    super::emit(&value, as_json, || render_timeline(&value))
}

pub async fn coverage(client: &HttpClient, scope: &Scope<'_>, as_json: bool) -> Result<()> {
    let value = client
        .get_value_with_headers("/api/v2/activity/coverage", &scope.headers())
        .await?;
    super::emit(&value, as_json, || render_coverage(&value))
}

pub async fn export(
    client: &HttpClient,
    scope: &Scope<'_>,
    query: Value,
    output: &Path,
) -> Result<()> {
    let headers = scope.headers();
    let value: Value = client
        .post_json_with_headers("/api/v2/activity/export", &query, &headers)
        .await?;
    let bytes = serde_json::to_vec_pretty(&value)?;
    std::fs::write(output, bytes)?;
    println!("Wrote signed activity export to {}", output.display());
    Ok(())
}

fn render_timeline(value: &Value) -> String {
    let mut out = render_coverage(value);
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows = events
        .iter()
        .map(|event| {
            vec![
                jstr(event, "occurred_at", "-").to_owned(),
                jstr(event, "event_name", "-").to_owned(),
                event
                    .pointer("/source/layer")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_owned(),
                event
                    .pointer("/source/trust")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_owned(),
                jstr(event, "sensitivity", "-").to_owned(),
                event
                    .pointer("/outcome/status")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_owned(),
                event
                    .pointer("/correlation/session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_owned(),
            ]
        })
        .collect::<Vec<_>>();
    out.push_str(&table::render(
        &[
            "OCCURRED",
            "EVENT",
            "SOURCE",
            "TRUST",
            "SENSITIVITY",
            "OUTCOME",
            "SESSION",
        ],
        &rows,
    ));
    out
}

fn render_coverage(value: &Value) -> String {
    let summary = value
        .get("completeness")
        .cloned()
        .unwrap_or_else(|| json!({}));
    format!(
        "coverage={} collectors={} gaps={} durable_loss={} dropped={} restarts={} stale={} clock_uncertainty_ms={} unsupported={}\n",
        jstr(&summary, "label", "incomplete-or-unknown"),
        summary.get("collector_count").and_then(Value::as_u64).unwrap_or(0),
        summary.get("sequence_gap_count").and_then(Value::as_u64).unwrap_or(0),
        summary.get("durable_loss_count").and_then(Value::as_u64).unwrap_or(0),
        summary.get("dropped_event_count").and_then(Value::as_u64).unwrap_or(0),
        summary.get("restart_count").and_then(Value::as_u64).unwrap_or(0),
        summary.get("stale_collector_count").and_then(Value::as_u64).unwrap_or(0),
        summary.get("maximum_clock_error_ms").and_then(Value::as_f64).unwrap_or(0.0),
        summary.get("unsupported_event_classes").and_then(Value::as_array).map(|v| v.len()).unwrap_or(0),
    )
}
