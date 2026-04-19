//! Server-side replay buffer for session frames.
//!
//! Late-joining clients request frames from a specific `seq` without
//! any client-side buffering requirement.

use std::collections::VecDeque;
use std::sync::Arc;

use super::SessionFrame;

/// Bounded ring buffer of recent session frames.
pub struct ReplayBuffer {
    frames: VecDeque<Arc<SessionFrame>>,
    max_frames: usize,
}

impl ReplayBuffer {
    pub fn new(max_frames: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(max_frames.min(1024)),
            max_frames,
        }
    }

    /// Append a frame.  Drops the oldest if at capacity.
    pub fn push(&mut self, frame: Arc<SessionFrame>) {
        if self.frames.len() >= self.max_frames {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    /// Iterate frames with `seq >= from_seq`.
    pub fn frames_from(&self, from_seq: u64) -> impl Iterator<Item = &Arc<SessionFrame>> {
        self.frames.iter().filter(move |f| f.seq >= from_seq)
    }

    /// All frames (for a full replay on fresh attach).
    pub fn all_frames(&self) -> impl Iterator<Item = &Arc<SessionFrame>> {
        self.frames.iter()
    }

    pub fn oldest_seq(&self) -> Option<u64> {
        self.frames.front().map(|f| f.seq)
    }

    pub fn newest_seq(&self) -> Option<u64> {
        self.frames.back().map(|f| f.seq)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}
