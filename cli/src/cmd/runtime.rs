//! Runtime-kind and VM-provider discovery.

use anyhow::Result;

use crate::client::http::HttpClient;

pub async fn list(client: &HttpClient, as_json: bool) -> Result<()> {
    let value = client.get_value("/api/v2/admin/runtime/providers").await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let mut rows = Vec::new();
    for runtime in value["runtimes"].as_array().into_iter().flatten() {
        let capabilities = runtime["capabilities"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        rows.push(vec![
            runtime["id"].as_str().unwrap_or("unknown").to_string(),
            if runtime["available"].as_bool().unwrap_or(false) {
                "yes".to_string()
            } else {
                "no".to_string()
            },
            runtime["isolation_tier"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            runtime["architecture"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            capabilities,
            runtime["unavailable_code"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        ]);
    }
    print!(
        "{}",
        crate::output::table::render(
            &[
                "RUNTIME",
                "AVAILABLE",
                "ISOLATION",
                "ARCH",
                "CAPABILITIES",
                "DIAGNOSTIC"
            ],
            &rows,
        )
    );
    Ok(())
}
