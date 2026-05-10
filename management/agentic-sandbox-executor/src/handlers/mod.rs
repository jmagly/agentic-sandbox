//! A2A method handlers.
//!
//! One module per A2A method. Filled in by #210 (most) and #211
//! (push_notification). Handlers are transport-agnostic: they take typed
//! request structs and the [`crate::instance::InstanceContext`] and return
//! typed responses. The [`crate::bindings`] modules adapt them to HTTP/WS.

pub mod cancel_task;
pub mod get_task;
pub mod list_tasks;
pub mod push_notification;
pub mod send_message;
pub mod send_streaming_message;
pub mod subscribe_to_task;
