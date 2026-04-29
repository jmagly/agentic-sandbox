//! Output renderers.
//!
//! Verbs declare a default human renderer (table or key:value). `--json`
//! and `--watch` are global flags applied at the dispatch boundary.
//! Streaming renderers (SSE / WS attach) live in their owning verb modules
//! since they need to drive the runtime, not just format a snapshot.

pub mod table;
