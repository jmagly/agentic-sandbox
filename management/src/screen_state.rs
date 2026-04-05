//! Per-session VT100 screen state tracker.
//!
//! Wraps a `vt100::Screen` and maintains a ring-buffer scrollback tail.
//! Used by the orchestrator WS endpoint to serve structured screen snapshots.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;

use crate::prompt_detector::{detect_prompt, PromptMatch};

/// How many lines of scrollback to retain beyond the visible screen
const SCROLLBACK_LINES: usize = 200;

/// Snapshot of the current terminal screen state
#[derive(Debug, Clone)]
pub struct ScreenSnapshot {
    pub rows: u16,
    pub cols: u16,
    /// Visible screen as plain text (newline-separated rows)
    pub text: String,
    pub cursor_row: u16,
    pub cursor_col: u16,
    /// Last N lines of scrollback as plain text
    pub scrollback_tail: String,
    /// Detected prompt on the current screen, if any
    pub prompt: Option<PromptMatch>,
}

/// Mutable screen state for a single PTY session
pub struct ScreenState {
    parser: vt100::Parser,
    scrollback: VecDeque<String>,
}

impl ScreenState {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            scrollback: VecDeque::with_capacity(SCROLLBACK_LINES + 10),
        }
    }

    /// Feed raw PTY bytes into the parser
    pub fn process(&mut self, data: &[u8]) {
        // Capture lines that are about to scroll off before processing
        let before = self.parser.screen().contents();
        let before_rows: Vec<&str> = before.lines().collect();

        self.parser.process(data);

        let after = self.parser.screen().contents();
        let after_rows: Vec<&str> = after.lines().collect();

        // Any rows that existed before but are no longer visible have scrolled off
        // (Simple heuristic: if the first row changed, the old first row scrolled off)
        if !before_rows.is_empty()
            && !after_rows.is_empty()
            && before_rows[0] != after_rows[0]
        {
            for old_row in &before_rows {
                if !old_row.trim().is_empty() {
                    self.scrollback.push_back(old_row.to_string());
                    if self.scrollback.len() > SCROLLBACK_LINES {
                        self.scrollback.pop_front();
                    }
                }
            }
        }
    }

    pub fn snapshot(&self) -> ScreenSnapshot {
        let screen = self.parser.screen();
        let text = screen.contents();
        let (cursor_row, cursor_col) = screen.cursor_position();

        let scrollback_tail = self
            .scrollback
            .iter()
            .rev()
            .take(50)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = detect_prompt(&text);

        ScreenSnapshot {
            rows: screen.size().0,
            cols: screen.size().1,
            text,
            cursor_row,
            cursor_col,
            scrollback_tail,
            prompt,
        }
    }
}

/// Registry of screen states, keyed by command_id
pub struct ScreenRegistry {
    states: DashMap<String, Arc<Mutex<ScreenState>>>,
}

impl ScreenRegistry {
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
        }
    }

    /// Get or create a screen state for a command
    pub fn get_or_create(&self, command_id: &str, rows: u16, cols: u16) -> Arc<Mutex<ScreenState>> {
        self.states
            .entry(command_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(ScreenState::new(rows, cols))))
            .clone()
    }

    /// Get an existing screen state (returns None if session not tracked)
    pub fn get(&self, command_id: &str) -> Option<Arc<Mutex<ScreenState>>> {
        self.states.get(command_id).map(|r| r.clone())
    }

    /// Remove a session (on session end)
    pub fn remove(&self, command_id: &str) {
        self.states.remove(command_id);
    }

    /// Feed bytes to a session's screen state (creates with defaults if new)
    pub fn process(&self, command_id: &str, data: &[u8]) {
        let arc = self.get_or_create(command_id, 24, 80);
        if let Ok(mut s) = arc.lock() {
            s.process(data);
        };
    }
}

impl Default for ScreenRegistry {
    fn default() -> Self {
        Self::new()
    }
}
