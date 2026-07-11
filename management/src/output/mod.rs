//! Output aggregation subsystem

mod aggregator;
mod chat_projection;

pub use aggregator::{OutputAggregator, OutputMessage, OutputRecvError, StreamType};
pub use chat_projection::{
    stream_interrupted_frame, ChatSource, ChatStreamFrame, StreamJsonProjector,
};
