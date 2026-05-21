//! Durable PTY transcript archive for frames evicted from hot replay.
//!
//! The hot replay ring remains the attach/reconnect cache. This archive is
//! the explicit, slower query path for older session output.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::warn;

use super::{RingEntry, RingEntryKind, SessionId, StreamKind};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptRecord {
    pub session_id: SessionId,
    pub seq: u64,
    pub ts: i64,
    pub kind: TranscriptKind,
    pub stream: StreamKind,
    pub text: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptKind {
    Output,
    Keyframe,
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptQuery {
    pub from_seq: Option<u64>,
    pub to_seq: Option<u64>,
    pub stream: Option<StreamKind>,
    pub pattern: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscriptMetrics {
    pub writes_total: u64,
    pub bytes_total: u64,
    pub write_errors_total: u64,
    pub searches_total: u64,
    pub search_errors_total: u64,
    pub pruned_total: u64,
}

#[derive(Debug)]
pub struct TranscriptArchive {
    root: PathBuf,
    writes_total: AtomicU64,
    bytes_total: AtomicU64,
    write_errors_total: AtomicU64,
    searches_total: AtomicU64,
    search_errors_total: AtomicU64,
    pruned_total: AtomicU64,
}

impl TranscriptArchive {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writes_total: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            write_errors_total: AtomicU64::new(0),
            searches_total: AtomicU64::new(0),
            search_errors_total: AtomicU64::new(0),
            pruned_total: AtomicU64::new(0),
        }
    }

    pub async fn append_evicted(
        &self,
        session_id: &SessionId,
        entries: &[std::sync::Arc<RingEntry>],
    ) {
        let records: Vec<TranscriptRecord> = entries
            .iter()
            .filter_map(|entry| record_from_entry(session_id, entry))
            .collect();
        if records.is_empty() {
            return;
        }
        if let Err(e) = self.append_records(session_id, &records).await {
            self.write_errors_total.fetch_add(1, Ordering::Relaxed);
            warn!(session_id = %session_id, error = %e, "failed to append PTY transcript records");
        }
    }

    async fn append_records(
        &self,
        session_id: &SessionId,
        records: &[TranscriptRecord],
    ) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.root).await?;
        let path = self.path_for_session(session_id);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let mut bytes_written = 0u64;
        for record in records {
            let line = serde_json::to_string(record)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            bytes_written += line.len() as u64 + 1;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        self.writes_total
            .fetch_add(records.len() as u64, Ordering::Relaxed);
        self.bytes_total.fetch_add(bytes_written, Ordering::Relaxed);
        Ok(())
    }

    pub async fn query(
        &self,
        session_id: &SessionId,
        query: &TranscriptQuery,
    ) -> std::io::Result<Vec<TranscriptRecord>> {
        self.searches_total.fetch_add(1, Ordering::Relaxed);
        match self.query_inner(session_id, query).await {
            Ok(records) => Ok(records),
            Err(e) => {
                self.search_errors_total.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    async fn query_inner(
        &self,
        session_id: &SessionId,
        query: &TranscriptQuery,
    ) -> std::io::Result<Vec<TranscriptRecord>> {
        let content = match tokio::fs::read_to_string(self.path_for_session(session_id)).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let pattern = query.pattern.as_deref();
        let mut records: Vec<TranscriptRecord> = content
            .lines()
            .filter_map(|line| serde_json::from_str::<TranscriptRecord>(line).ok())
            .filter(|record| query.from_seq.map(|seq| record.seq >= seq).unwrap_or(true))
            .filter(|record| query.to_seq.map(|seq| record.seq <= seq).unwrap_or(true))
            .filter(|record| {
                query
                    .stream
                    .map(|stream| record.stream == stream)
                    .unwrap_or(true)
            })
            .filter(|record| pattern.map(|p| record.text.contains(p)).unwrap_or(true))
            .collect();
        records.sort_by_key(|record| record.seq);
        records.truncate(query.limit.max(1).min(5000));
        Ok(records)
    }

    pub fn metrics_snapshot(&self) -> TranscriptMetrics {
        TranscriptMetrics {
            writes_total: self.writes_total.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            write_errors_total: self.write_errors_total.load(Ordering::Relaxed),
            searches_total: self.searches_total.load(Ordering::Relaxed),
            search_errors_total: self.search_errors_total.load(Ordering::Relaxed),
            pruned_total: self.pruned_total.load(Ordering::Relaxed),
        }
    }

    fn path_for_session(&self, session_id: &SessionId) -> PathBuf {
        let safe = session_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        self.root.join(format!("{safe}.jsonl"))
    }
}

fn record_from_entry(session_id: &SessionId, entry: &RingEntry) -> Option<TranscriptRecord> {
    match &entry.kind {
        RingEntryKind::Output { stream, data } => Some(TranscriptRecord {
            session_id: session_id.clone(),
            seq: entry.seq,
            ts: entry.ts,
            kind: TranscriptKind::Output,
            stream: *stream,
            text: String::from_utf8_lossy(data).into_owned(),
            bytes: data.len(),
        }),
        RingEntryKind::Keyframe { stream, data } => Some(TranscriptRecord {
            session_id: session_id.clone(),
            seq: entry.seq,
            ts: entry.ts,
            kind: TranscriptKind::Keyframe,
            stream: *stream,
            text: String::from_utf8_lossy(data).into_owned(),
            bytes: data.len(),
        }),
        RingEntryKind::Control(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn archive_appends_and_searches_records() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = TranscriptArchive::new(tmp.path());
        let session_id = "sess-1".to_string();
        let entries = vec![
            std::sync::Arc::new(RingEntry {
                seq: 1,
                ts: 10,
                kind: RingEntryKind::Output {
                    stream: StreamKind::Stdout,
                    data: Bytes::from_static(b"alpha hello"),
                },
            }),
            std::sync::Arc::new(RingEntry {
                seq: 2,
                ts: 11,
                kind: RingEntryKind::Output {
                    stream: StreamKind::Stderr,
                    data: Bytes::from_static(b"beta"),
                },
            }),
        ];

        archive.append_evicted(&session_id, &entries).await;
        let records = archive
            .query(
                &session_id,
                &TranscriptQuery {
                    pattern: Some("hello".to_string()),
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[0].stream, StreamKind::Stdout);
        assert_eq!(records[0].text, "alpha hello");
        let metrics = archive.metrics_snapshot();
        assert_eq!(metrics.writes_total, 2);
        assert!(metrics.bytes_total > 0);
        assert_eq!(metrics.searches_total, 1);
    }
}
