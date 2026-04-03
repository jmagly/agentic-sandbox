//! View agent logs from agentshare

use anyhow::Result;
use colored::Colorize;
use std::path::PathBuf;

const AGENTSHARE_PATH: &str = "/mnt/inbox";

pub async fn show(agent_id: &str, follow: bool, lines: usize) -> Result<()> {
    println!("{} Logs for agent: {}", "=>".blue().bold(), agent_id.cyan());

    // Find the latest run directory
    let inbox_path = PathBuf::from(AGENTSHARE_PATH);
    let current_link = inbox_path.join("current");

    if !current_link.exists() {
        println!("{}", "No agent logs found. Is the agent running?".yellow());
        return Ok(());
    }

    let run_dir = std::fs::read_link(&current_link)?;
    println!("  Run directory: {}", run_dir.display());

    // Read stdout.log
    let stdout_log = inbox_path.join(&run_dir).join("stdout.log");
    let stderr_log = inbox_path.join(&run_dir).join("stderr.log");

    if follow {
        println!("\n{}", "Following logs (Ctrl+C to stop)...".yellow());

        // Use tail -f
        let mut cmd = std::process::Command::new("tail");
        cmd.arg("-f")
            .arg("-n")
            .arg(lines.to_string())
            .arg(&stdout_log);

        if stderr_log.exists() {
            cmd.arg(&stderr_log);
        }

        let _ = cmd.status();
    } else {
        // Show last N lines
        if stdout_log.exists() {
            println!("\n{}", "=== stdout.log ===".cyan().bold());
            let output = std::process::Command::new("tail")
                .arg("-n")
                .arg(lines.to_string())
                .arg(&stdout_log)
                .output()?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }

        if stderr_log.exists() {
            println!("\n{}", "=== stderr.log ===".red().bold());
            let output = std::process::Command::new("tail")
                .arg("-n")
                .arg(lines.to_string())
                .arg(&stderr_log)
                .output()?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }

        // Show commands.log if exists
        let commands_log = inbox_path.join(&run_dir).join("commands.log");
        if commands_log.exists() {
            println!("\n{}", "=== commands.log ===".yellow().bold());
            let output = std::process::Command::new("tail")
                .arg("-n")
                .arg("20")
                .arg(&commands_log)
                .output()?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }

    Ok(())
}
