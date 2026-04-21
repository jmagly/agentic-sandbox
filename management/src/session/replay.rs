//! Server-side replay buffer for session frames.
//!
//! Late-joining clients request frames from a specific `seq` without
//! any client-side buffering requirement.
//!
//! Eviction is dual-gated: frames are dropped from the front when either
//! the frame count OR the byte cap is exceeded, whichever triggers first.
//! This prevents OOM from TUI programs that emit large full-screen repaints.

use std::collections::VecDeque;
use std::sync::Arc;

use super::{SessionFrame, SessionPayload};

/// 2 MB per session — enough for a rich interactive history while bounding heap growth.
/// Each base64-encoded 80×24 full repaint is ~10 KB; this holds ~200 such frames.
const DEFAULT_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Bounded ring buffer of recent session frames.
///
/// Evicts oldest frames when either `max_frames` or `max_bytes` is exceeded.
pub struct ReplayBuffer {
    frames: VecDeque<Arc<SessionFrame>>,
    max_frames: usize,
    max_bytes: usize,
    total_bytes: usize,
}

impl ReplayBuffer {
    pub fn new(max_frames: usize) -> Self {
        Self::with_byte_cap(max_frames, DEFAULT_MAX_BYTES)
    }

    pub fn with_byte_cap(max_frames: usize, max_bytes: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(max_frames.min(1024)),
            max_frames,
            max_bytes,
            total_bytes: 0,
        }
    }

    /// Append a frame. Evicts oldest frames until both caps are satisfied.
    pub fn push(&mut self, frame: Arc<SessionFrame>) {
        let cost = frame_byte_cost(&frame);
        while self.frames.len() >= self.max_frames
            || (self.total_bytes + cost > self.max_bytes && !self.frames.is_empty())
        {
            if let Some(evicted) = self.frames.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(frame_byte_cost(&evicted));
            } else {
                break;
            }
        }
        self.frames.push_back(frame);
        self.total_bytes += cost;
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

    /// Current byte usage of buffered frames.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

/// Byte cost of a single frame for eviction accounting.
///
/// Output frames cost their data length (base64-encoded PTY bytes).
/// Control frames (resize, role, closed, error) are small fixed overhead.
fn frame_byte_cost(frame: &SessionFrame) -> usize {
    match &frame.payload {
        SessionPayload::Output { data, .. } => data.len(),
        _ => 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionPayload, StreamKind};

    fn make_output_frame(seq: u64, size: usize) -> Arc<SessionFrame> {
        Arc::new(SessionFrame {
            session_id: "test".to_string(),
            seq,
            ts: 0,
            payload: SessionPayload::Output {
                stream: StreamKind::Stdout,
                data: "x".repeat(size),
            },
        })
    }

    #[test]
    fn evicts_by_frame_count() {
        let mut buf = ReplayBuffer::with_byte_cap(3, usize::MAX);
        for i in 0..5 {
            buf.push(make_output_frame(i, 10));
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.oldest_seq(), Some(2));
    }

    #[test]
    fn evicts_by_byte_cap() {
        // 3 frames max, 100 byte cap — each frame is 50 bytes
        let mut buf = ReplayBuffer::with_byte_cap(100, 100);
        buf.push(make_output_frame(0, 50)); // 50 bytes, 1 frame
        buf.push(make_output_frame(1, 50)); // 100 bytes, 2 frames — at byte limit
        // Adding a third 50-byte frame must evict frame 0 to stay under byte cap
        buf.push(make_output_frame(2, 50));
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.oldest_seq(), Some(1));
        assert!(buf.total_bytes() <= 100);
    }

    #[test]
    fn total_bytes_tracks_correctly() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        buf.push(make_output_frame(0, 100));
        buf.push(make_output_frame(1, 200));
        assert_eq!(buf.total_bytes(), 300);
        // Evict by pushing beyond frame limit
        let mut buf2 = ReplayBuffer::with_byte_cap(1, usize::MAX);
        buf2.push(make_output_frame(0, 100));
        buf2.push(make_output_frame(1, 200));
        assert_eq!(buf2.len(), 1);
        assert_eq!(buf2.total_bytes(), 200);
    }

    #[test]
    fn frames_from_filters_correctly() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        for i in 0..5 {
            buf.push(make_output_frame(i, 10));
        }
        let seqs: Vec<u64> = buf.frames_from(3).map(|f| f.seq).collect();
        assert_eq!(seqs, vec![3, 4]);
    }
}
