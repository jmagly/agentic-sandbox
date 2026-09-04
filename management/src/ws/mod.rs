//! WebSocket monitoring hub

mod connection;
mod hub;

pub(crate) use connection::MANAGEMENT_WS_PROTOCOL_VERSION;
pub use hub::WebSocketHub;
