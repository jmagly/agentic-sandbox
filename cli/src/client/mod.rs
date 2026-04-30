//! Transport-agnostic SDK for the management server.
//!
//! Verbs build on `client::http::HttpClient`. Streaming verbs add SSE/WS
//! later (#156). Auth headers come from the active `ContextsFile` context.

pub mod http;
pub mod models;
pub mod sse;
pub mod ws;
