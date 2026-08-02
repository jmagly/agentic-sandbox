//! Neutral fleet-workload projection for external orchestrators.
//!
//! These commands intentionally speak only the shared
//! `agentic-orchestration/v1` contract. AIWG/Cockpit is the first consumer,
//! but no AIWG-specific fields or behavior are embedded here.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

use crate::client::http::HttpClient;
use crate::output::{jnum, jstr, kv, table};

const WORKLOADS_PATH: &str = "/api/v2/fleet/workloads";
const RECONCILE_PATH: &str = "/api/v2/fleet/reconcile";

pub async fn dispatch(client: &HttpClient, file: &Path, as_json: bool) -> Result<()> {
    let bytes = std::fs::read(file)
        .with_context(|| format!("read fleet workload record {}", file.display()))?;
    let record: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse fleet workload record {} as JSON", file.display()))?;
    let value: Value = client.post_json(WORKLOADS_PATH, Some(&record)).await?;
    render_dispatch(&value, as_json)
}

pub async fn inventory(client: &HttpClient, as_json: bool) -> Result<()> {
    let value = client.get_value(WORKLOADS_PATH).await?;
    render_inventory(&value, as_json)
}

pub async fn get(client: &HttpClient, child_id: &str, as_json: bool) -> Result<()> {
    let value = client
        .get_value(&format!(
            "{}/{}",
            WORKLOADS_PATH,
            super::urlencode(child_id)
        ))
        .await?;
    render_workload(&value, as_json)
}

pub async fn reconcile(
    client: &HttpClient,
    before_revision: u64,
    child_ids: &[String],
    as_json: bool,
) -> Result<()> {
    if child_ids.is_empty() {
        anyhow::bail!("reconcile requires at least one --child-id");
    }
    let request = json!({
        "before_revision": before_revision,
        "child_ids": child_ids,
    });
    let value: Value = client.post_json(RECONCILE_PATH, Some(&request)).await?;
    render_reconciliation(&value, as_json)
}

fn render_dispatch(value: &Value, as_json: bool) -> Result<()> {
    super::emit(value, as_json, || {
        let workload = &value["workload"];
        kv::render(&[
            (
                "replayed",
                value["replayed"].as_bool().unwrap_or(false).to_string(),
            ),
            (
                "child_id",
                workload
                    .pointer("/lineage/child_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            ("kind", jstr(workload, "kind", "-").to_string()),
            (
                "target",
                workload
                    .pointer("/lineage/target_id")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "state",
                workload
                    .pointer("/status/observed_state")
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "revision",
                workload
                    .pointer("/status/revision")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .to_string(),
            ),
        ])
    })
}

fn render_inventory(value: &Value, as_json: bool) -> Result<()> {
    super::emit(value, as_json, || {
        let mut rows = Vec::new();
        for workload in value["records"].as_array().into_iter().flatten() {
            rows.push(workload_row(workload));
        }
        let mut rendered = kv::render(&[
            ("inventory_revision", jnum(value, "inventory_revision")),
            ("generated_at", jstr(value, "generated_at", "-").to_string()),
        ]);
        rendered.push_str(&table::render(
            &["CHILD", "KIND", "TARGET", "EXECUTOR", "STATE", "REVISION"],
            &rows,
        ));
        rendered
    })
}

fn render_workload(value: &Value, as_json: bool) -> Result<()> {
    super::emit(value, as_json, || {
        let lineage = &value["lineage"];
        let status = &value["status"];
        kv::render(&[
            ("child_id", jstr(lineage, "child_id", "").to_string()),
            ("kind", jstr(value, "kind", "-").to_string()),
            ("mission_id", jstr(lineage, "mission_id", "-").to_string()),
            ("target_id", jstr(lineage, "target_id", "-").to_string()),
            ("executor_id", jstr(lineage, "executor_id", "-").to_string()),
            ("runtime_id", jstr(lineage, "runtime_id", "-").to_string()),
            ("session_id", jstr(lineage, "session_id", "-").to_string()),
            ("task_id", jstr(lineage, "task_id", "-").to_string()),
            ("command_id", jstr(lineage, "command_id", "-").to_string()),
            (
                "observed_state",
                jstr(status, "observed_state", "-").to_string(),
            ),
            ("revision", jnum(status, "revision")),
            ("last_seen", jstr(status, "last_seen", "-").to_string()),
        ])
    })
}

fn render_reconciliation(value: &Value, as_json: bool) -> Result<()> {
    super::emit(value, as_json, || {
        let mut rows = Vec::new();
        for row in value["rows"].as_array().into_iter().flatten() {
            rows.push(vec![
                jstr(row, "child_id", "").to_string(),
                jstr(row, "classification", "unknown").to_string(),
                jstr(row, "observed_state", "unknown").to_string(),
                jnum(row, "revision"),
                jstr(row, "reason", "-").to_string(),
            ]);
        }
        let mut rendered = kv::render(&[
            ("before_revision", jnum(value, "before_revision")),
            ("after_revision", jnum(value, "after_revision")),
            ("generated_at", jstr(value, "generated_at", "-").to_string()),
        ]);
        rendered.push_str(&table::render(
            &["CHILD", "CLASSIFICATION", "STATE", "REVISION", "REASON"],
            &rows,
        ));
        rendered
    })
}

fn workload_row(workload: &Value) -> Vec<String> {
    vec![
        workload
            .pointer("/lineage/child_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        jstr(workload, "kind", "-").to_string(),
        workload
            .pointer("/lineage/target_id")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        workload
            .pointer("/lineage/executor_id")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        workload
            .pointer("/status/observed_state")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        workload
            .pointer("/status/revision")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextEntry;
    use tempfile::NamedTempFile;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> HttpClient {
        HttpClient::new(&ContextEntry {
            server: server.uri(),
            token: "operator-token".into(),
            role: "admin".into(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn inventory_uses_neutral_v2_route_and_bearer_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(WORKLOADS_PATH))
            .and(header("authorization", "Bearer operator-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "inventory_revision": 0,
                "generated_at": "2026-08-02T12:00:00Z",
                "records": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        inventory(&client(&server), true).await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_preserves_contract_record_and_reconcile_is_explicit() {
        let server = MockServer::start().await;
        let record = json!({"document_type":"workload","api_version":"agentic-orchestration/v1"});
        Mock::given(method("POST"))
            .and(path(WORKLOADS_PATH))
            .and(body_json(record.clone()))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({
                "replayed": false,
                "workload": record
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(RECONCILE_PATH))
            .and(body_json(
                json!({"before_revision": 7, "child_ids": ["child-a", "child-b"]}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "before_revision": 7,
                "after_revision": 9,
                "generated_at": "2026-08-02T12:00:00Z",
                "rows": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut file = NamedTempFile::new().unwrap();
        serde_json::to_writer(&mut file, &record).unwrap();
        dispatch(&client(&server), file.path(), true).await.unwrap();
        reconcile(
            &client(&server),
            7,
            &["child-a".into(), "child-b".into()],
            true,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_percent_encodes_child_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/fleet/workloads/child%2Funsafe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "kind": "one-shot-command",
                "lineage": {"child_id": "child/unsafe"},
                "status": {"observed_state": "running", "revision": 2}
            })))
            .expect(1)
            .mount(&server)
            .await;

        get(&client(&server), "child/unsafe", true).await.unwrap();
    }
}
