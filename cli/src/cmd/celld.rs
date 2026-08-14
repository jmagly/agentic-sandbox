use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

use crate::client::http::HttpClient;

const ROOT: &str = "/api/v2/celld";

pub async fn status(client: &HttpClient, as_json: bool) -> Result<()> {
    render(&client.get_value(&format!("{ROOT}/status")).await?, as_json)
}
pub async fn cell(client: &HttpClient, id: &str, generation: u64, as_json: bool) -> Result<()> {
    render(
        &client
            .get_value(&format!(
                "{ROOT}/cells/{}?generation={generation}",
                super::urlencode(id)
            ))
            .await?,
        as_json,
    )
}
pub async fn command(client: &HttpClient, file: &Path, as_json: bool) -> Result<()> {
    let value = read_json(file)?;
    let id = value
        .get("instance_id")
        .and_then(Value::as_str)
        .context("command requires instance_id")?;
    render(
        &client
            .post_json(
                &format!("{ROOT}/cells/{}/commands", super::urlencode(id)),
                Some(&value),
            )
            .await?,
        as_json,
    )
}
pub async fn reconcile(
    client: &HttpClient,
    id: &str,
    generation: u64,
    as_json: bool,
) -> Result<()> {
    render(
        &client
            .post_json(
                &format!("{ROOT}/cells/{}/reconcile", super::urlencode(id)),
                Some(&json!({"management_generation":generation})),
            )
            .await?,
        as_json,
    )
}
pub async fn validate_bundle(client: &HttpClient, file: &Path, as_json: bool) -> Result<()> {
    post_file(client, &format!("{ROOT}/bundles/validate"), file, as_json).await
}
pub async fn validate_fleet(client: &HttpClient, file: &Path, as_json: bool) -> Result<()> {
    post_file(client, &format!("{ROOT}/fleets/validate"), file, as_json).await
}
pub async fn preflight(client: &HttpClient, file: &Path, as_json: bool) -> Result<()> {
    post_file(client, &format!("{ROOT}/fleets/preflight"), file, as_json).await
}
pub async fn plan_upgrade(
    client: &HttpClient,
    file: &Path,
    from: &str,
    to: &str,
    as_json: bool,
) -> Result<()> {
    let manifest = read_json(file)?;
    render(
        &client
            .post_json(
                &format!("{ROOT}/fleets/plan-upgrade"),
                Some(&json!({"manifest":manifest,"from":from,"to":to})),
            )
            .await?,
        as_json,
    )
}
async fn post_file(client: &HttpClient, path: &str, file: &Path, as_json: bool) -> Result<()> {
    let value = read_json(file)?;
    render(&client.post_json(path, Some(&value)).await?, as_json)
}
fn read_json(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {} as JSON", path.display()))
}
fn render(value: &Value, _as_json: bool) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
