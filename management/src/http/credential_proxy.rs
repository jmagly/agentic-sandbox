//! HTTP/API credential proxy backend for ADR-028.
//!
//! Workloads send a lease reference and target metadata to this endpoint. The
//! proxy authorizes the request against the lease's proxy policy, injects the
//! upstream credential only for the outbound hop, and returns a redacted proxy
//! response. The upstream secret is never returned in API responses.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

use super::server::AppState;
use crate::credentials::{
    CredentialError, CredentialProxyInjectedHeader, CredentialProxyMaterial, CredentialProxyPolicy,
};

const DEFAULT_PROXY_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PROXY_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct ProxyHttpRequest {
    pub lease_id: String,
    pub agent_id: String,
    pub instance_id: String,
    pub session_id: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProxyHttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Debug, Serialize)]
struct ProxyErrorResponse {
    error: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/http", post(proxy_http))
}

async fn proxy_http(
    State(state): State<AppState>,
    Json(request): Json<ProxyHttpRequest>,
) -> Response {
    match proxy_http_inner(state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => proxy_error(err),
    }
}

async fn proxy_http_inner(
    state: AppState,
    request: ProxyHttpRequest,
) -> Result<ProxyHttpResponse, CredentialError> {
    let material = state.credential_broker.proxy_material_for_active_lease(
        &request.lease_id,
        &request.agent_id,
        &request.instance_id,
        &request.session_id,
    )?;
    let url = Url::parse(&request.url)
        .map_err(|_| CredentialError::ProxyDenied("target URL is invalid".to_string()))?;
    authorize_proxy_request(&material.policy, &request, &url)?;

    let method = Method::from_bytes(request.method.as_bytes())
        .map_err(|_| CredentialError::ProxyDenied("HTTP method is invalid".to_string()))?;
    let mut outbound_headers = filtered_headers(&material.policy, &request.headers)?;
    inject_header(
        &mut outbound_headers,
        material.policy.injected_header.as_ref(),
        &material,
    )?;

    let client = reqwest::Client::builder()
        .timeout(DEFAULT_PROXY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| CredentialError::ProxyDenied(format!("proxy client unavailable: {err}")))?;
    let mut builder = client.request(method, url).headers(outbound_headers);
    if let Some(body) = request.body {
        builder = builder.body(body);
    }
    let response = builder
        .send()
        .await
        .map_err(|err| CredentialError::ProxyDenied(format!("upstream request failed: {err}")))?;
    let status = response.status().as_u16();
    let headers = response_headers(response.headers(), &material.secret);
    let bytes = response
        .bytes()
        .await
        .map_err(|err| CredentialError::ProxyDenied(format!("upstream response failed: {err}")))?;
    if bytes.len() > MAX_PROXY_BODY_BYTES {
        return Err(CredentialError::ProxyDenied(
            "upstream response body exceeded proxy limit".to_string(),
        ));
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();

    Ok(ProxyHttpResponse {
        status,
        headers,
        body: redact_secret(&body, &material.secret),
    })
}

fn authorize_proxy_request(
    policy: &CredentialProxyPolicy,
    request: &ProxyHttpRequest,
    url: &Url,
) -> Result<(), CredentialError> {
    let host = url
        .host_str()
        .ok_or_else(|| CredentialError::ProxyDenied("target URL host is required".to_string()))?;
    let host_with_port = url
        .port()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or_else(|| host.to_string());
    if !is_host_allowed(host, &host_with_port, &policy.allowed_hosts) {
        return Err(CredentialError::ProxyDenied(format!(
            "target host {host_with_port} is not allowed"
        )));
    }
    if !policy.allowed_path_prefixes.is_empty()
        && !policy
            .allowed_path_prefixes
            .iter()
            .any(|prefix| url.path().starts_with(prefix))
    {
        return Err(CredentialError::ProxyDenied(format!(
            "target path {} is not allowed",
            url.path()
        )));
    }
    if !policy.allowed_methods.is_empty()
        && !policy
            .allowed_methods
            .iter()
            .any(|method| method.eq_ignore_ascii_case(&request.method))
    {
        return Err(CredentialError::ProxyDenied(format!(
            "HTTP method {} is not allowed",
            request.method
        )));
    }
    Ok(())
}

fn is_host_allowed(host: &str, host_with_port: &str, allowed_hosts: &[String]) -> bool {
    allowed_hosts.iter().any(|allowed| {
        let allowed = allowed.trim();
        if allowed.is_empty() {
            return false;
        }
        if let Some(suffix) = allowed.strip_prefix("*.") {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host.eq_ignore_ascii_case(allowed) || host_with_port.eq_ignore_ascii_case(allowed)
        }
    })
}

fn filtered_headers(
    policy: &CredentialProxyPolicy,
    headers: &BTreeMap<String, String>,
) -> Result<HeaderMap, CredentialError> {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        let allowed = policy
            .allowed_headers
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name));
        if !allowed {
            return Err(CredentialError::ProxyDenied(format!(
                "header {name} is not allowed"
            )));
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| CredentialError::ProxyDenied(format!("header {name} is invalid")))?;
        let header_value = HeaderValue::from_str(value).map_err(|_| {
            CredentialError::ProxyDenied(format!("header {name} contains invalid characters"))
        })?;
        out.insert(header_name, header_value);
    }
    Ok(out)
}

fn inject_header(
    headers: &mut HeaderMap,
    configured: Option<&CredentialProxyInjectedHeader>,
    material: &CredentialProxyMaterial,
) -> Result<(), CredentialError> {
    let configured = configured
        .cloned()
        .unwrap_or(CredentialProxyInjectedHeader {
            name: "authorization".to_string(),
            value_prefix: "Bearer ".to_string(),
        });
    let header_name = HeaderName::from_bytes(configured.name.as_bytes()).map_err(|_| {
        CredentialError::ProxyDenied("proxy injected header name is invalid".to_string())
    })?;
    headers.remove(&header_name);
    let value = format!("{}{}", configured.value_prefix, material.secret);
    let header_value = HeaderValue::from_str(&value).map_err(|_| {
        CredentialError::ProxyDenied(
            "proxy credential value is not valid for header injection".to_string(),
        )
    })?;
    headers.insert(header_name, header_value);
    Ok(())
}

fn response_headers(headers: &HeaderMap, secret: &str) -> BTreeMap<String, String> {
    let mut redacted = BTreeMap::new();
    for (name, value) in headers {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let value = redact_secret(value, secret);
        if !value.is_empty() {
            redacted.insert(name.as_str().to_string(), value);
        }
    }
    redacted
}

fn redact_secret(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        return value.to_string();
    }
    value.replace(secret, "[REDACTED_CREDENTIAL]")
}

fn proxy_error(err: CredentialError) -> Response {
    let status = match err {
        CredentialError::LeaseNotFound(_) => StatusCode::NOT_FOUND,
        CredentialError::NotFound(_) => StatusCode::NOT_FOUND,
        CredentialError::ProxyDenied(_)
        | CredentialError::ProxyPolicyMissing(_)
        | CredentialError::LeaseDenied(_)
        | CredentialError::NotConfigured(_)
        | CredentialError::UnsupportedBackend(_) => StatusCode::FORBIDDEN,
        CredentialError::MissingId
        | CredentialError::MissingProvider
        | CredentialError::MissingType
        | CredentialError::AlreadyExists(_) => StatusCode::BAD_REQUEST,
        CredentialError::Persistence(_)
        | CredentialError::Serialization(_)
        | CredentialError::BackendRead { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(ProxyErrorResponse {
            error: err.to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request as HttpRequest};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use crate::credentials::{
        CredentialProxyInjectedHeader, CredentialProxyPolicy, CredentialValueInput,
        IssueCredentialLeaseRequest, UpsertCredentialRequest,
    };

    fn test_state() -> AppState {
        let registry = Arc::new(crate::registry::AgentRegistry::new());
        AppState {
            registry: registry.clone(),
            output_agg: Arc::new(crate::output::OutputAggregator::new(64)),
            dispatcher: Arc::new(crate::dispatch::CommandDispatcher::new(registry)),
            orchestrator: None,
            metrics: None,
            operation_store: Some(Arc::new(super::super::operations::OperationStore::new())),
            audit_logger: None,
            credential_broker: Arc::new(crate::credentials::CredentialBroker::new_in_memory()),
            startup_profiles: Arc::new(
                crate::startup_profiles::StartupProfileStore::new_in_memory(),
            ),
            ssh_gateway_leases: Arc::new(crate::ssh_gateway::SshGatewayLeaseStore::new_in_memory()),
            bootstrap_token_store: None,
            grpc_ca_backend: None,
            screen_registry: None,
            hitl_store: None,
            aiwg_handle: None,
            mission_store: None,
            session_registry: None,
            transport_identity_resolver: None,
            agentshare_root: None,
            tasks_root: None,
            operator_auth: None,
            mtls_config: super::super::operator_auth::MtlsConfig::default(),
            unix_peer_creds_config: super::super::operator_auth::UnixPeerCredsConfig::default(),
            executor_instance_registry: None,
            executor_signing_keys_dir: None,
            executor_idempotency: None,
            host_runtime_supervisor: None,
            v1_counter: None,
        }
    }

    fn create_credential_and_lease(state: &AppState, host: String, secret: &str) -> String {
        state
            .credential_broker
            .create(UpsertCredentialRequest {
                id: "cred_proxy_api".to_string(),
                provider: "example".to_string(),
                credential_type: "api_key".to_string(),
                owner: None,
                scopes: vec!["api:read".to_string()],
                allowed_uses: vec!["proxy.http".to_string()],
                backend: None,
                value: Some(CredentialValueInput {
                    kind: "write_only".to_string(),
                    plaintext: Some(secret.to_string()),
                }),
            })
            .unwrap();
        state
            .credential_broker
            .issue_lease(
                "cred_proxy_api",
                IssueCredentialLeaseRequest {
                    agent_id: "agent-01".to_string(),
                    instance_id: "instance-01".to_string(),
                    session_id: "session-01".to_string(),
                    provider: "example".to_string(),
                    allowed_use: "proxy.http".to_string(),
                    ttl_seconds: 60,
                    proxy_policy: Some(CredentialProxyPolicy {
                        allowed_hosts: vec![host],
                        allowed_path_prefixes: vec!["/v1/".to_string()],
                        allowed_methods: vec!["GET".to_string(), "POST".to_string()],
                        allowed_headers: vec!["x-client-trace".to_string()],
                        injected_header: Some(CredentialProxyInjectedHeader {
                            name: "authorization".to_string(),
                            value_prefix: "Bearer ".to_string(),
                        }),
                        rate_limit_per_minute: Some(60),
                    }),
                },
            )
            .unwrap()
            .id
    }

    async fn upstream_server() -> (String, tokio::task::JoinHandle<()>) {
        async fn handler(headers: HeaderMap) -> Json<Value> {
            let auth = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            Json(json!({
                "authorized": auth == "Bearer proxy-secret-fake",
                "observed_authorization": auth,
            }))
        }

        let app = Router::new().route("/v1/resource", post(handler).get(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("127.0.0.1:{}", addr.port()), handle)
    }

    async fn proxy_request(app: Router, body: Value) -> axum::response::Response {
        app.oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/v2/credential-proxy/http")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn credential_proxy_injects_secret_only_upstream_and_redacts_response() {
        let (host, upstream) = upstream_server().await;
        let state = test_state();
        let lease_id = create_credential_and_lease(&state, host.clone(), "proxy-secret-fake");
        let app = Router::new()
            .nest("/api/v2/credential-proxy", router())
            .with_state(state);

        let response = proxy_request(
            app,
            json!({
                "lease_id": lease_id,
                "agent_id": "agent-01",
                "instance_id": "instance-01",
                "session_id": "session-01",
                "method": "POST",
                "url": format!("http://{host}/v1/resource"),
                "headers": {"x-client-trace": "trace-1"},
                "body": "{\"hello\":\"world\"}"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("proxy-secret-fake"));
        assert!(text.contains("[REDACTED_CREDENTIAL]"));
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["status"], 200);
        assert!(
            body["body"]
                .as_str()
                .unwrap()
                .contains("\"authorized\":true"),
            "{body}"
        );
        upstream.abort();
    }

    #[tokio::test]
    async fn credential_proxy_denies_host_path_method_header_and_bad_lease_scope() {
        let (host, upstream) = upstream_server().await;
        let state = test_state();
        let lease_id = create_credential_and_lease(&state, host.clone(), "proxy-secret-fake");
        let app = Router::new()
            .nest("/api/v2/credential-proxy", router())
            .with_state(state);

        for (label, mut body) in [
            (
                "host",
                json!({
                    "lease_id": lease_id,
                    "agent_id": "agent-01",
                    "instance_id": "instance-01",
                    "session_id": "session-01",
                    "method": "GET",
                    "url": "http://example.invalid/v1/resource",
                    "headers": {}
                }),
            ),
            (
                "path",
                json!({
                    "lease_id": lease_id,
                    "agent_id": "agent-01",
                    "instance_id": "instance-01",
                    "session_id": "session-01",
                    "method": "GET",
                    "url": format!("http://{host}/private"),
                    "headers": {}
                }),
            ),
            (
                "method",
                json!({
                    "lease_id": lease_id,
                    "agent_id": "agent-01",
                    "instance_id": "instance-01",
                    "session_id": "session-01",
                    "method": "DELETE",
                    "url": format!("http://{host}/v1/resource"),
                    "headers": {}
                }),
            ),
            (
                "header",
                json!({
                    "lease_id": lease_id,
                    "agent_id": "agent-01",
                    "instance_id": "instance-01",
                    "session_id": "session-01",
                    "method": "GET",
                    "url": format!("http://{host}/v1/resource"),
                    "headers": {"x-not-allowed": "1"}
                }),
            ),
            (
                "scope",
                json!({
                    "lease_id": lease_id,
                    "agent_id": "agent-02",
                    "instance_id": "instance-01",
                    "session_id": "session-01",
                    "method": "GET",
                    "url": format!("http://{host}/v1/resource"),
                    "headers": {}
                }),
            ),
        ] {
            body["label"] = json!(label);
            let response = proxy_request(app.clone(), body).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{label}");
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(!text.contains("proxy-secret-fake"), "{label}: {text}");
        }
        upstream.abort();
    }

    #[tokio::test]
    async fn credential_proxy_denies_missing_revoked_expired_and_policyless_leases() {
        let (host, upstream) = upstream_server().await;
        let state = test_state();
        let lease_id = create_credential_and_lease(&state, host.clone(), "proxy-secret-fake");
        state.credential_broker.revoke_lease(&lease_id).unwrap();
        let app = Router::new()
            .nest("/api/v2/credential-proxy", router())
            .with_state(state.clone());

        let base = json!({
            "agent_id": "agent-01",
            "instance_id": "instance-01",
            "session_id": "session-01",
            "method": "GET",
            "url": format!("http://{host}/v1/resource"),
            "headers": {}
        });
        let mut revoked = base.clone();
        revoked["lease_id"] = json!(lease_id);
        assert_eq!(
            proxy_request(app.clone(), revoked).await.status(),
            StatusCode::FORBIDDEN
        );

        let mut missing = base.clone();
        missing["lease_id"] = json!("lease_missing");
        assert_eq!(
            proxy_request(app.clone(), missing).await.status(),
            StatusCode::NOT_FOUND
        );

        state
            .credential_broker
            .create(UpsertCredentialRequest {
                id: "cred_policyless".to_string(),
                provider: "example".to_string(),
                credential_type: "api_key".to_string(),
                owner: None,
                scopes: vec![],
                allowed_uses: vec!["proxy.http".to_string()],
                backend: None,
                value: Some(CredentialValueInput {
                    kind: "write_only".to_string(),
                    plaintext: Some("proxy-secret-fake".to_string()),
                }),
            })
            .unwrap();
        let policyless = state
            .credential_broker
            .issue_lease(
                "cred_policyless",
                IssueCredentialLeaseRequest {
                    agent_id: "agent-01".to_string(),
                    instance_id: "instance-01".to_string(),
                    session_id: "session-01".to_string(),
                    provider: "example".to_string(),
                    allowed_use: "proxy.http".to_string(),
                    ttl_seconds: 60,
                    proxy_policy: None,
                },
            )
            .unwrap();
        let mut no_policy = base.clone();
        no_policy["lease_id"] = json!(policyless.id);
        assert_eq!(
            proxy_request(app.clone(), no_policy).await.status(),
            StatusCode::FORBIDDEN
        );

        let expired = state
            .credential_broker
            .issue_lease(
                "cred_policyless",
                IssueCredentialLeaseRequest {
                    agent_id: "agent-01".to_string(),
                    instance_id: "instance-01".to_string(),
                    session_id: "session-expired".to_string(),
                    provider: "example".to_string(),
                    allowed_use: "proxy.http".to_string(),
                    ttl_seconds: 1,
                    proxy_policy: Some(CredentialProxyPolicy {
                        allowed_hosts: vec![host.clone()],
                        allowed_path_prefixes: vec!["/v1/".to_string()],
                        allowed_methods: vec!["GET".to_string()],
                        allowed_headers: vec![],
                        injected_header: None,
                        rate_limit_per_minute: None,
                    }),
                },
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let mut expired_request = base;
        expired_request["lease_id"] = json!(expired.id);
        expired_request["session_id"] = json!("session-expired");
        assert_eq!(
            proxy_request(app, expired_request).await.status(),
            StatusCode::FORBIDDEN
        );
        upstream.abort();
    }
}
