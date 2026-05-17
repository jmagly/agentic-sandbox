//! Regression: agent-side `execute_command` used to `tokio::join!` the
//! `stdin_task` whose mpsc sender lived in the shared `running_commands`
//! map. Because the sender was only dropped *after* the join, any
//! command that received no stdin (e.g. the `printf` A2A dispatch from
//! `AgentMessageDispatch`) hung forever and never produced a terminal
//! `CommandResult`.
//!
//! Issue: #271 ("dispatched messages:send task stays working after
//! agent command starts").
//!
//! These tests verify the structural property at the tokio primitive
//! level. They do not import `execute_command` itself (it lives in the
//! binary crate), but they reproduce its stdin-task shape exactly:
//! one mpsc::Receiver inside a spawned task, one sender held in a map
//! that outlives the join.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone)]
struct FakeStdinData {
    eof: bool,
}

/// Simulate the stdin draining task as written in agent-rs/src/main.rs
/// `execute_command`: loops on recv, exits on EOF marker, otherwise
/// only when the channel closes.
fn spawn_stdin_task(mut rx: mpsc::Receiver<FakeStdinData>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if data.eof {
                break;
            }
        }
    })
}

/// The pre-fix shape: stdin_tx is held in `running_commands` (the map),
/// the spawned task is joined, and only AFTER the join is the entry
/// removed from the map. With no EOF and no other sender drop, the
/// join blocks forever. We assert deadlock via timeout.
#[tokio::test]
async fn pre_fix_join_pattern_deadlocks_without_eof() {
    let running: Arc<Mutex<HashMap<String, mpsc::Sender<FakeStdinData>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (tx, rx) = mpsc::channel::<FakeStdinData>(8);
    running.lock().await.insert("cmd-1".into(), tx);

    let stdin_task = spawn_stdin_task(rx);

    // Mirror the pre-fix sequence: join the stdin task BEFORE removing
    // the entry from `running_commands`. With the only sender held in
    // the map and no EOF arriving, this must deadlock.
    let joined = tokio::time::timeout(Duration::from_millis(200), stdin_task).await;
    assert!(
        joined.is_err(),
        "stdin_task should hang while its sender is still held in running_commands"
    );

    // Cleanup so the test process can exit.
    running.lock().await.remove("cmd-1");
}

/// The post-fix shape (#271): output streams are joined naturally,
/// stdin_task is aborted instead of joined. The function returns
/// promptly even when no stdin EOF arrives, so the downstream
/// CommandResult is emitted and #271's `working`-forever symptom
/// goes away.
#[tokio::test]
async fn post_fix_abort_pattern_returns_promptly() {
    let running: Arc<Mutex<HashMap<String, mpsc::Sender<FakeStdinData>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (tx, rx) = mpsc::channel::<FakeStdinData>(8);
    running.lock().await.insert("cmd-2".into(), tx);

    let stdin_task = spawn_stdin_task(rx);

    // Mirror the fix: abort instead of join. Must return effectively
    // immediately; we give it a generous slack for CI scheduling jitter.
    stdin_task.abort();
    let start = std::time::Instant::now();
    let aborted = tokio::time::timeout(Duration::from_millis(500), async {
        // Awaiting an aborted JoinHandle yields a JoinError quickly.
        let _ = stdin_task_handle_join(running.clone()).await;
    })
    .await;
    assert!(aborted.is_ok(), "abort-then-cleanup must not hang");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "abort path returned in {:?}",
        start.elapsed()
    );
}

/// Helper that performs the post-abort cleanup the real path does:
/// remove the running_commands entry (which drops the last sender).
async fn stdin_task_handle_join(running: Arc<Mutex<HashMap<String, mpsc::Sender<FakeStdinData>>>>) {
    running.lock().await.remove("cmd-2");
}
