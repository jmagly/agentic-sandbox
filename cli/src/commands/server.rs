//! Management server commands

use anyhow::Result;
use colored::Colorize;
use std::process::Command;

const SERVER_BIN: &str = "agentic-mgmt";

pub async fn start(foreground: bool) -> Result<()> {
    println!("{} Starting management server", "=>".blue().bold());

    if foreground {
        // Run in foreground
        let status = Command::new(SERVER_BIN).status()?;

        if !status.success() {
            anyhow::bail!("Server exited with error");
        }
    } else {
        // Run in background
        let child = Command::new(SERVER_BIN)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        println!(
            "{} Server started (PID: {})",
            "✓".green().bold(),
            child.id()
        );
        println!("  gRPC:      http://localhost:8120");
        println!("  WebSocket: ws://localhost:8121");
    }

    Ok(())
}

pub async fn status() -> Result<()> {
    println!("{} Server status", "=>".blue().bold());

    // Check if server is running by trying to connect
    let output = Command::new("ss").args(["-tlnp"]).output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    let grpc_running = stdout.contains(":8120");
    let ws_running = stdout.contains(":8121");

    if grpc_running {
        println!("  gRPC (8120):      {}", "running".green());
    } else {
        println!("  gRPC (8120):      {}", "not running".red());
    }

    if ws_running {
        println!("  WebSocket (8121): {}", "running".green());
    } else {
        println!("  WebSocket (8121): {}", "not running".red());
    }

    Ok(())
}

pub async fn stop() -> Result<()> {
    println!("{} Stopping management server", "=>".blue().bold());

    // Find and kill server process
    let output = Command::new("pkill").args(["-f", SERVER_BIN]).output()?;

    if output.status.success() {
        println!("{} Server stopped", "✓".green().bold());
    } else {
        println!("{} Server was not running", "!".yellow().bold());
    }

    Ok(())
}
