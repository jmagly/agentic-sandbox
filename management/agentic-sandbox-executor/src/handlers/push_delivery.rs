//! Push-notification delivery worker (#211).
//!
//! Background worker that POSTs Task status updates to subscriber URLs
//! registered via `tasks/pushNotificationConfig` (CRUD lives in
//! `push_notification.rs`).
//!
//! ## Trigger
//!
//! Caller invokes [`PushDelivery::spawn`] once at startup, receiving a
//! [`tokio::sync::mpsc::Sender<DeliveryEvent>`]. To enqueue a delivery,
//! send a [`DeliveryEvent`] on that sender. The worker consumes events,
//! looks up active push configs for the task via [`TaskStore::list_push_configs`],
//! and dispatches one HTTP POST per config.
//!
//! ## Per-attempt
//!
//! Each POST carries an `X-AIWG-Signature` header of the form:
//!
//! ```text
//! t=<unix-seconds>,v1=<hmac-sha256(body, secret) as 64 hex chars>
//! ```
//!
//! The signature input is `<timestamp>.<body-bytes>` (Stripe-style v1 scheme
//! to prevent body-only replay). When the auth descriptor has no secret —
//! `{ "type": "none" }` or `{ "type": "bearer", "secret": "..." }` — the
//! HMAC step is skipped. Bearer tokens are emitted as
//! `Authorization: Bearer <secret>`.
//!
//! ## Retry policy
//!
//! Exponential backoff: 1, 2, 4, 8, 16, 32, 64 seconds (7 attempts total).
//! After 7 failed attempts the delivery is dropped and a `tracing::warn!` is
//! emitted with the task_id, config_id, and the last status code observed.
//! Future work (#211 follow-up) may persist a dead-letter row for replay.
//!
//! ## Body shape
//!
//! ```json
//! {
//!   "task_id": "<tid>",
//!   "status_event": { ...A2A status event JSON... }
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::mpsc;

use crate::store::task_store::{PushNotificationConfigRow, TaskStore};

type HmacSha256 = Hmac<Sha256>;

/// Maximum number of delivery attempts before giving up.
const MAX_ATTEMPTS: u32 = 7;

/// Backoff schedule in seconds, one entry per attempt index (0-based).
/// `delay_for_attempt(n)` is `BACKOFF_SECS[n]` clamped to the last entry.
///
/// In test builds the schedule collapses to 0ms-equivalent waits so the
/// retry tests don't wall-clock the suite. Production (non-test) builds
/// use the real exponential schedule (1, 2, 4, 8, 16, 32, 64s).
#[cfg(not(test))]
const BACKOFF_SECS: &[u64] = &[1, 2, 4, 8, 16, 32, 64];
#[cfg(test)]
const BACKOFF_SECS: &[u64] = &[0, 0, 0, 0, 0, 0, 0];

/// Channel capacity for the delivery mpsc.
const CHANNEL_CAPACITY: usize = 1024;

/// A push-delivery request: post `status_event` to every active config for
/// `task_id`.
#[derive(Debug, Clone)]
pub struct DeliveryEvent {
    pub task_id: String,
    pub status_event: Value,
}

/// Outbound HTTP delivery service.
///
/// Cheaply cloneable; the inner [`reqwest::Client`] holds a connection pool.
pub struct PushDelivery {
    store: Arc<TaskStore>,
    http: reqwest::Client,
}

impl PushDelivery {
    /// Build a delivery worker bound to the given store.
    pub fn new(store: Arc<TaskStore>) -> Self {
        // `connect_timeout` keeps a hung subscriber from blocking the worker
        // for the whole 30s default; per-attempt overall timeout caps a slow
        // body read.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { store, http }
    }

    /// Spawn the worker on the current Tokio runtime and return the sender
    /// used to enqueue deliveries.
    pub fn spawn(self) -> mpsc::Sender<DeliveryEvent> {
        let (tx, mut rx) = mpsc::channel::<DeliveryEvent>(CHANNEL_CAPACITY);
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                // Sequential per-event; per-config deliveries run in parallel.
                self.deliver_event(ev).await;
            }
        });
        tx
    }

    /// Look up every active config for `ev.task_id` and dispatch in parallel.
    async fn deliver_event(&self, ev: DeliveryEvent) {
        let configs = match self.store.list_push_configs(&ev.task_id) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(task_id = %ev.task_id, error = %e, "push: list_push_configs failed");
                return;
            }
        };
        if configs.is_empty() {
            return;
        }
        let body = build_body(&ev);
        let body_bytes = match serde_json::to_vec(&body) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(task_id = %ev.task_id, error = %e, "push: body serialize failed");
                return;
            }
        };
        let mut joins = Vec::with_capacity(configs.len());
        for cfg in configs {
            let http = self.http.clone();
            let body_bytes = body_bytes.clone();
            let task_id = ev.task_id.clone();
            joins.push(tokio::spawn(async move {
                deliver_to_subscriber(&http, &cfg, &task_id, &body_bytes).await;
            }));
        }
        for j in joins {
            let _ = j.await;
        }
    }

    /// Public single-event entry point for tests.
    pub async fn deliver_one(&self, ev: DeliveryEvent) {
        self.deliver_event(ev).await;
    }

    /// Compute the v1 HMAC-SHA256 signature value (`<64-hex-chars>`) over
    /// `<ts>.<body>` using `secret`.
    pub fn sign(secret: &str, body: &[u8], ts: i64) -> String {
        sign_v1(secret, body, ts)
    }
}

/// Build the A2A StreamResponse-shaped delivery body.
fn build_body(ev: &DeliveryEvent) -> Value {
    json!({
        "task_id": ev.task_id,
        "status_event": ev.status_event,
    })
}

/// HMAC-SHA256 over `<ts>.<body>`, hex-encoded.
fn sign_v1(secret: &str, body: &[u8], ts: i64) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes.iter() {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

/// Decode the optional auth descriptor into `(scheme, secret)`.
///
/// Scheme is one of `"bearer"`, `"hmac"`, `"none"`. `secret` is `None` for
/// `none` or when missing/empty.
fn decode_auth(auth: &Option<Value>) -> (String, Option<String>) {
    let auth = match auth {
        Some(a) => a,
        None => return ("none".to_string(), None),
    };
    let scheme = auth
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
        .to_string();
    let secret = auth
        .get("secret")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    (scheme, secret)
}

/// Deliver `body` to one subscriber with retry/backoff.
///
/// Behavior summary (matches doc-comment at top of file):
/// - 2xx → success, return.
/// - non-2xx or transport error → retry up to `MAX_ATTEMPTS - 1` more times.
/// - After all attempts fail → log a warning and return.
async fn deliver_to_subscriber(
    http: &reqwest::Client,
    cfg: &PushNotificationConfigRow,
    task_id: &str,
    body: &[u8],
) {
    let (scheme, secret) = decode_auth(&cfg.auth_json);

    let mut last_status: Option<u16> = None;

    for attempt in 0..MAX_ATTEMPTS {
        let ts = chrono::Utc::now().timestamp();
        let mut req = http
            .post(&cfg.url)
            .header("content-type", "application/json")
            .body(body.to_vec());

        match scheme.as_str() {
            "bearer" => {
                if let Some(token) = &secret {
                    req = req.header("authorization", format!("Bearer {token}"));
                }
            }
            "hmac" => {
                if let Some(s) = &secret {
                    let sig = sign_v1(s, body, ts);
                    let header_val = format!("t={ts},v1={sig}");
                    req = req.header("x-aiwg-signature", header_val);
                }
            }
            _ => {} // "none" or unknown: no auth header
        }

        let res = req.send().await;
        match res {
            Ok(resp) => {
                let status = resp.status();
                last_status = Some(status.as_u16());
                if status.is_success() {
                    return;
                }
                tracing::debug!(
                    task_id = %task_id,
                    config_id = %cfg.config_id,
                    attempt = attempt,
                    status = status.as_u16(),
                    "push: non-2xx response, will retry"
                );
            }
            Err(e) => {
                tracing::debug!(
                    task_id = %task_id,
                    config_id = %cfg.config_id,
                    attempt = attempt,
                    error = %e,
                    "push: transport error, will retry"
                );
            }
        }

        // After the last attempt, do not sleep.
        if attempt + 1 < MAX_ATTEMPTS {
            let secs = BACKOFF_SECS
                .get(attempt as usize)
                .copied()
                .unwrap_or(*BACKOFF_SECS.last().unwrap_or(&64));
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }
    }

    tracing::warn!(
        task_id = %task_id,
        config_id = %cfg.config_id,
        last_status = ?last_status,
        attempts = MAX_ATTEMPTS,
        "push: dead-lettered after max attempts"
    );
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::task_store::{
        PushNotificationConfigRow, TaskRow, TaskState, TaskStore,
    };
    use chrono::Utc;
    use std::sync::Arc;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn seed_task(store: &TaskStore, tid: &str) {
        let now = Utc::now();
        store
            .upsert_task(&TaskRow {
                task_id: tid.to_string(),
                context_id: None,
                state: TaskState::Submitted,
                fail_kind: None,
                status_json: serde_json::json!({"state": "submitted"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: None,
            })
            .unwrap();
    }

    fn seed_config(store: &TaskStore, cid: &str, tid: &str, url: &str, auth: Option<Value>) {
        store
            .put_push_config(&PushNotificationConfigRow {
                config_id: cid.to_string(),
                task_id: tid.to_string(),
                url: url.to_string(),
                auth_json: auth,
                created_at: Utc::now(),
            })
            .unwrap();
    }

    #[test]
    fn hmac_signature_format() {
        // Verify the format `t=<ts>,v1=<64-hex>` and that the digest matches
        // an independently computed reference.
        let secret = "test-secret";
        let body = br#"{"task_id":"t-1"}"#;
        let ts: i64 = 1_700_000_000;
        let sig = PushDelivery::sign(secret, body, ts);
        assert_eq!(sig.len(), 64, "sig must be 64 hex chars");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));

        // Reference: HMAC-SHA256(secret, "<ts>." || body)
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(ts.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        assert_eq!(sig, expected);

        // The wire format that the worker emits.
        let header = format!("t={ts},v1={sig}");
        assert!(header.starts_with(&format!("t={ts},v1=")));
    }

    #[tokio::test]
    async fn successful_delivery_logs_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_task(&store, "t-1");
        seed_config(
            &store,
            "c-1",
            "t-1",
            &format!("{}/hook", server.uri()),
            None,
        );

        let pd = PushDelivery::new(store);
        pd.deliver_one(DeliveryEvent {
            task_id: "t-1".to_string(),
            status_event: serde_json::json!({"state": "working"}),
        })
        .await;

        // wiremock's .expect(1) verifies on Drop, but we also explicitly
        // check the call count for clarity.
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
    }

    #[tokio::test]
    async fn retry_on_5xx() {
        // 500, 500, 500, 200 → 4 attempts.
        //
        // wiremock matches mocks by priority then by insertion order. To
        // get a deterministic 500-500-500-200 sequence we use `up_to_n_times`
        // on the failing mock with HIGHER priority so it wins until its
        // budget is exhausted, then the success mock catches the rest.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(3)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .with_priority(2)
            .mount(&server)
            .await;

        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_task(&store, "t-1");
        seed_config(
            &store,
            "c-1",
            "t-1",
            &format!("{}/hook", server.uri()),
            None,
        );

        // In test builds BACKOFF_SECS is zeroed so this completes promptly.
        let pd = PushDelivery::new(store);
        pd.deliver_one(DeliveryEvent {
            task_id: "t-1".to_string(),
            status_event: serde_json::json!({"state": "working"}),
        })
        .await;

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 4, "should retry 3 times after failures, then succeed on 4th");
    }

    #[tokio::test]
    async fn give_up_after_7_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_task(&store, "t-1");
        seed_config(
            &store,
            "c-1",
            "t-1",
            &format!("{}/hook", server.uri()),
            None,
        );

        let pd = PushDelivery::new(store);
        pd.deliver_one(DeliveryEvent {
            task_id: "t-1".to_string(),
            status_event: serde_json::json!({"state": "working"}),
        })
        .await;

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 7, "should attempt MAX_ATTEMPTS times before giving up");
    }

    #[tokio::test]
    async fn body_contains_streamresponse_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_task(&store, "t-1");
        seed_config(
            &store,
            "c-1",
            "t-1",
            &format!("{}/hook", server.uri()),
            None,
        );

        let pd = PushDelivery::new(store);
        pd.deliver_one(DeliveryEvent {
            task_id: "t-1".to_string(),
            status_event: serde_json::json!({"state": "working", "timestamp": "2026-01-01T00:00:00Z"}),
        })
        .await;

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let req = &received[0];
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["task_id"], "t-1");
        assert_eq!(body["status_event"]["state"], "working");
        assert_eq!(body["status_event"]["timestamp"], "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn hmac_auth_sends_signature_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .and(header_exists("x-aiwg-signature"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_task(&store, "t-1");
        seed_config(
            &store,
            "c-1",
            "t-1",
            &format!("{}/hook", server.uri()),
            Some(serde_json::json!({"type": "hmac", "secret": "shh"})),
        );

        let pd = PushDelivery::new(store);
        pd.deliver_one(DeliveryEvent {
            task_id: "t-1".to_string(),
            status_event: serde_json::json!({"state": "working"}),
        })
        .await;

        let received = server.received_requests().await.unwrap();
        let header = received[0]
            .headers
            .get("x-aiwg-signature")
            .expect("signature header present");
        let val = header.to_str().unwrap();
        assert!(val.starts_with("t="), "header must start with t=, got {val}");
        assert!(val.contains(",v1="), "header must contain ,v1=, got {val}");
    }
}
