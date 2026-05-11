//! Command dispatch subsystem

mod dispatcher;

pub use dispatcher::{
    CommandDispatcher, DispatchError, OutputObserver, SessionInfo, SessionType,
};
