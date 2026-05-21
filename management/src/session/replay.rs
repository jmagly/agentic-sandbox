//! Server-side replay buffer for session frames.
//!
//! Late-joining clients request frames from a specific `seq` without
//! any client-side buffering requirement.
//!
//! Storage layout (post-#147): `Output` frames are stored as raw PTY
//! `Bytes` — base64 encoding happens at fan-out / replay time, not at
//! write time. This eliminates the 33% base64 overhead in the
//! steady-state ring (the encoded copy lives only in live mpsc queues
//! during the brief fan-out window). Control frames (Resize, Closed,
//! RoleAssigned, MembershipChanged, Error) are small and rare; they
//! keep the wire-format `SessionPayload` directly.
//!
//! Eviction is dual-gated: frames are dropped from the front when either
//! the frame count OR the byte cap is exceeded, whichever triggers first.
//! Bytes accounting is now against RAW data length so the ring holds
//! 33% more output frames at the same memory budget.

use base64::Engine as _;
use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::Arc;

use super::{SessionFrame, SessionId, SessionPayload, StreamKind};

/// Default visible terminal rows used to size the hot replay window.
pub const DEFAULT_VISIBLE_ROWS: usize = 24;

/// Default visible terminal columns used to size the hot replay window.
pub const DEFAULT_VISIBLE_COLS: usize = 80;

/// Keep only the previous three screenfuls hot in memory by default.
///
/// Older history belongs in durable/searchable session output storage, not
/// in every live session's RAM footprint.
pub const DEFAULT_HOT_SCREENS: usize = 3;

/// Frame count cap for the hot replay window.
pub const DEFAULT_MAX_FRAMES: usize = DEFAULT_VISIBLE_ROWS * DEFAULT_HOT_SCREENS;

/// Byte cap for the hot replay window, sized as three 80x24 full-screen
/// repaints plus small control-frame headroom.
pub const DEFAULT_MAX_BYTES: usize =
    DEFAULT_VISIBLE_ROWS * DEFAULT_VISIBLE_COLS * DEFAULT_HOT_SCREENS * 4;

/// What's stored in the ring per frame.
#[derive(Debug, Clone)]
pub enum RingEntryKind {
    /// PTY output. Bytes are raw — caller materializes a base64-encoded
    /// `SessionPayload::Output` on the way out via `to_wire`.
    Output { stream: StreamKind, data: Bytes },
    /// Periodic full-repaint snapshot. Same on-the-wire shape as
    /// `Output` (base64 bytes) but rendered as `SessionPayload::Keyframe`
    /// so smart clients know it's a safe replay starting point (#145).
    Keyframe { stream: StreamKind, data: Bytes },
    /// Small control frames kept in their wire format for cheap replay.
    Control(SessionPayload),
}

#[derive(Debug)]
pub struct RingEntry {
    pub seq: u64,
    pub ts: i64,
    pub kind: RingEntryKind,
}

impl RingEntry {
    /// Materialize this entry as a wire-format `SessionFrame` for an
    /// attaching client. Output entries are base64-encoded fresh; the
    /// encoding cost is paid once per replay (rare event), not once per
    /// frame at write time (the 33% overhead the issue calls out).
    pub fn to_wire(&self, session_id: &SessionId) -> SessionFrame {
        let payload = match &self.kind {
            RingEntryKind::Output { stream, data } => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                SessionPayload::Output {
                    stream: *stream,
                    data: encoded,
                }
            }
            RingEntryKind::Keyframe { stream, data } => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                SessionPayload::Keyframe {
                    stream: *stream,
                    data: encoded,
                }
            }
            RingEntryKind::Control(p) => p.clone(),
        };
        SessionFrame {
            session_id: session_id.clone(),
            seq: self.seq,
            ts: self.ts,
            payload,
        }
    }

    fn cost_bytes(&self) -> usize {
        match &self.kind {
            // Raw bytes — no base64 multiplier.
            RingEntryKind::Output { data, .. } | RingEntryKind::Keyframe { data, .. } => data.len(),
            // Control frames are small; flat overhead keeps eviction
            // accounting simple and bounded.
            RingEntryKind::Control(_) => 64,
        }
    }
}

/// Bounded ring buffer of recent session frames.
///
/// Evicts oldest frames when either `max_frames` or `max_bytes` is exceeded.
pub struct ReplayBuffer {
    frames: VecDeque<Arc<RingEntry>>,
    max_frames: usize,
    max_bytes: usize,
    total_bytes: usize,
    evicted_frames_total: u64,
    evicted_bytes_total: u64,
    /// Seq of the most recent keyframe that's still in the ring. `None`
    /// if no keyframe has been pushed yet OR the most recent keyframe
    /// was evicted. Used by `attach()` to choose a safe replay start
    /// for fresh joiners (#145).
    last_keyframe_seq: Option<u64>,
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
            evicted_frames_total: 0,
            evicted_bytes_total: 0,
            last_keyframe_seq: None,
        }
    }

    /// Push an `Output` frame as raw bytes (zero-copy from `Vec<u8>`).
    pub fn push_output(&mut self, seq: u64, ts: i64, stream: StreamKind, data: Bytes) {
        let entry = Arc::new(RingEntry {
            seq,
            ts,
            kind: RingEntryKind::Output { stream, data },
        });
        self.push_entry(entry);
    }

    /// Push a periodic keyframe (full repaint). Updates `last_keyframe_seq`
    /// so future fresh joiners replay from this point.
    pub fn push_keyframe(&mut self, seq: u64, ts: i64, stream: StreamKind, data: Bytes) {
        let entry = Arc::new(RingEntry {
            seq,
            ts,
            kind: RingEntryKind::Keyframe { stream, data },
        });
        self.push_entry(entry);
        self.last_keyframe_seq = Some(seq);
    }

    /// Push a small control frame (Resize, Closed, RoleAssigned, etc.).
    pub fn push_control(&mut self, seq: u64, ts: i64, payload: SessionPayload) {
        let entry = Arc::new(RingEntry {
            seq,
            ts,
            kind: RingEntryKind::Control(payload),
        });
        self.push_entry(entry);
    }

    fn push_entry(&mut self, entry: Arc<RingEntry>) {
        let cost = entry.cost_bytes();
        while self.frames.len() >= self.max_frames
            || (self.total_bytes + cost > self.max_bytes && !self.frames.is_empty())
        {
            if let Some(evicted) = self.frames.pop_front() {
                let evicted_cost = evicted.cost_bytes();
                self.total_bytes = self.total_bytes.saturating_sub(evicted_cost);
                self.evicted_frames_total = self.evicted_frames_total.saturating_add(1);
                self.evicted_bytes_total =
                    self.evicted_bytes_total.saturating_add(evicted_cost as u64);
                // If we just evicted the last-known keyframe, drop the
                // pointer; replay will fall back to the oldest entry.
                if self.last_keyframe_seq == Some(evicted.seq) {
                    self.last_keyframe_seq = None;
                }
            } else {
                break;
            }
        }
        self.frames.push_back(entry);
        self.total_bytes += cost;
    }

    /// Seq of the most recent in-ring keyframe. None ⇒ no safe
    /// mid-ring start exists; fresh joiners get either no replay or
    /// the entire ring (caller's policy).
    pub fn last_keyframe_seq(&self) -> Option<u64> {
        self.last_keyframe_seq
    }

    /// Iterate entries with `seq >= from_seq`. Caller materializes wire
    /// frames via `RingEntry::to_wire(&session_id)`.
    pub fn frames_from(&self, from_seq: u64) -> impl Iterator<Item = &Arc<RingEntry>> {
        self.frames.iter().filter(move |f| f.seq >= from_seq)
    }

    /// All entries (full replay on fresh attach).
    pub fn all_frames(&self) -> impl Iterator<Item = &Arc<RingEntry>> {
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

    /// Current byte usage of buffered frames (raw bytes, not base64-encoded).
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn evicted_frames_total(&self) -> u64 {
        self.evicted_frames_total
    }

    pub fn evicted_bytes_total(&self) -> u64 {
        self.evicted_bytes_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_output(buf: &mut ReplayBuffer, seq: u64, size: usize) {
        let raw = Bytes::from(vec![b'x'; size]);
        buf.push_output(seq, 0, StreamKind::Stdout, raw);
    }

    #[test]
    fn evicts_by_frame_count() {
        let mut buf = ReplayBuffer::with_byte_cap(3, usize::MAX);
        for i in 0..5 {
            push_output(&mut buf, i, 10);
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.oldest_seq(), Some(2));
    }

    #[test]
    fn evicts_by_byte_cap() {
        let mut buf = ReplayBuffer::with_byte_cap(100, 100);
        push_output(&mut buf, 0, 50);
        push_output(&mut buf, 1, 50);
        push_output(&mut buf, 2, 50);
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.oldest_seq(), Some(1));
        assert!(buf.total_bytes() <= 100);
    }

    #[test]
    fn total_bytes_uses_raw_size_not_encoded() {
        // 100 raw bytes ⇒ 100 cost. With base64 storage this would have
        // been ~136. The test asserts the post-#147 invariant.
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        push_output(&mut buf, 0, 100);
        push_output(&mut buf, 1, 200);
        assert_eq!(buf.total_bytes(), 300);
    }

    #[test]
    fn frames_from_filters_correctly() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        for i in 0..5 {
            push_output(&mut buf, i, 10);
        }
        let seqs: Vec<u64> = buf.frames_from(3).map(|f| f.seq).collect();
        assert_eq!(seqs, vec![3, 4]);
    }

    #[test]
    fn to_wire_encodes_output_to_base64_on_demand() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        buf.push_output(7, 42, StreamKind::Stdout, Bytes::from_static(b"hello"));
        let entry = buf.frames_from(0).next().unwrap();
        let frame = entry.to_wire(&"sess-x".to_string());
        match frame.payload {
            SessionPayload::Output { stream, data } => {
                assert_eq!(stream, StreamKind::Stdout);
                assert_eq!(data, "aGVsbG8=");
            }
            _ => panic!("expected Output payload"),
        }
        assert_eq!(frame.session_id, "sess-x");
        assert_eq!(frame.seq, 7);
        assert_eq!(frame.ts, 42);
    }

    #[test]
    fn keyframe_tracks_last_seq() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        push_output(&mut buf, 0, 10);
        push_output(&mut buf, 1, 10);
        buf.push_keyframe(
            2,
            0,
            StreamKind::Stdout,
            Bytes::from_static(b"\x1b[2J\x1b[Hhello"),
        );
        push_output(&mut buf, 3, 10);
        assert_eq!(buf.last_keyframe_seq(), Some(2));
    }

    #[test]
    fn keyframe_pointer_dropped_when_evicted() {
        // Tight cap: 2 frames. Push KF then enough output to evict it.
        let mut buf = ReplayBuffer::with_byte_cap(2, usize::MAX);
        buf.push_keyframe(0, 0, StreamKind::Stdout, Bytes::from_static(b"kf"));
        assert_eq!(buf.last_keyframe_seq(), Some(0));
        push_output(&mut buf, 1, 10);
        push_output(&mut buf, 2, 10); // evicts seq 0
        assert_eq!(buf.last_keyframe_seq(), None);
    }

    #[test]
    fn keyframe_to_wire_emits_keyframe_payload() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        buf.push_keyframe(5, 0, StreamKind::Stdout, Bytes::from_static(b"hi"));
        let entry = buf.frames_from(0).next().unwrap();
        let frame = entry.to_wire(&"s".to_string());
        match frame.payload {
            SessionPayload::Keyframe { stream, data } => {
                assert_eq!(stream, StreamKind::Stdout);
                assert_eq!(data, "aGk=");
            }
            _ => panic!("expected Keyframe payload"),
        }
    }

    #[test]
    fn default_hot_window_is_three_visible_screens() {
        let mut buf = ReplayBuffer::new(DEFAULT_MAX_FRAMES);
        assert_eq!(buf.max_frames(), DEFAULT_VISIBLE_ROWS * DEFAULT_HOT_SCREENS);
        assert_eq!(
            buf.max_bytes(),
            DEFAULT_VISIBLE_ROWS * DEFAULT_VISIBLE_COLS * DEFAULT_HOT_SCREENS * 4
        );
        for i in 0..(DEFAULT_MAX_FRAMES as u64 + 5) {
            push_output(&mut buf, i, 1);
        }
        assert_eq!(buf.len(), DEFAULT_MAX_FRAMES);
        assert_eq!(buf.evicted_frames_total(), 5);
    }

    #[test]
    fn eviction_counters_track_dropped_hot_history() {
        let mut buf = ReplayBuffer::with_byte_cap(3, usize::MAX);
        for i in 0..5 {
            push_output(&mut buf, i, 10);
        }
        assert_eq!(buf.evicted_frames_total(), 2);
        assert_eq!(buf.evicted_bytes_total(), 20);
    }
    #[test]
    fn control_frames_round_trip_payload() {
        let mut buf = ReplayBuffer::with_byte_cap(10, usize::MAX);
        buf.push_control(3, 0, SessionPayload::Resize { cols: 80, rows: 24 });
        let entry = buf.frames_from(0).next().unwrap();
        let frame = entry.to_wire(&"s".to_string());
        match frame.payload {
            SessionPayload::Resize { cols, rows } => {
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
            }
            _ => panic!("expected Resize"),
        }
    }
}
