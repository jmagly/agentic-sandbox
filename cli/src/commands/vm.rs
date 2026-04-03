//! VM lifecycle commands

use anyhow::Result;
use colored::Colorize;
use std::process::Command;

const PROVISION_SCRIPT: &str = "/home/roctinam/dev/agentic-sandbox/scripts/provision-vm.sh";

pub async fn create(name: &str, profile: &str, agentshare: bool) -> Result<()> {
    println!("{} Creating VM: {}", "=>".blue().bold(), name);

    let mut cmd = Command::new(PROVISION_SCRIPT);
    cmd.arg(name).arg("--profile").arg(profile);

    if agentshare {
        cmd.arg("--agentshare");
    }

    let status = cmd.status()?;

    if status.success() {
        println!("{} VM {} created successfully", "✓".green().bold(), name);
    } else {
        anyhow::bail!("Failed to create VM");
    }

    Ok(())
}

pub async fn list() -> Result<()> {
    println!("{}", "VM List".bold());
    println!("{}", "─".repeat(60));

    // Use virsh to list VMs
    let output = Command::new("virsh").args(["list", "--all"]).output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse and colorize output
    for line in stdout.lines().skip(2) {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let name = parts[1];
            let state = parts[2..].join(" ");

            let state_colored = match state.as_str() {
                "running" => state.green().to_string(),
                "shut off" => state.red().to_string(),
                "paused" => state.yellow().to_string(),
                _ => state.to_string(),
            };

            println!("  {} {}", name.cyan(), state_colored);
        }
    }

    Ok(())
}

pub async fn status(name: &str) -> Result<()> {
    println!("{} Status for VM: {}", "=>".blue().bold(), name);

    let output = Command::new("virsh").args(["dominfo", name]).output()?;

    if !output.status.success() {
        anyhow::bail!("VM {} not found", name);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();

            let value_colored = if key == "State" {
                match value {
                    "running" => value.green().to_string(),
                    "shut off" => value.red().to_string(),
                    _ => value.to_string(),
                }
            } else {
                value.to_string()
            };

            println!("  {}: {}", key.cyan(), value_colored);
        }
    }

    Ok(())
}

pub async fn start(name: &str) -> Result<()> {
    println!("{} Starting VM: {}", "=>".blue().bold(), name);

    let status = Command::new("virsh").args(["start", name]).status()?;

    if status.success() {
        println!("{} VM {} started", "✓".green().bold(), name);
    } else {
        anyhow::bail!("Failed to start VM {}", name);
    }

    Ok(())
}

pub async fn stop(name: &str, force: bool) -> Result<()> {
    println!("{} Stopping VM: {}", "=>".blue().bold(), name);

    let args = if force {
        vec!["destroy", name]
    } else {
        vec!["shutdown", name]
    };

    let status = Command::new("virsh").args(&args).status()?;

    if status.success() {
        println!("{} VM {} stopped", "✓".green().bold(), name);
    } else {
        anyhow::bail!("Failed to stop VM {}", name);
    }

    Ok(())
}

pub async fn destroy(name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!(
            "{} This will permanently destroy VM {}. Are you sure? (y/N)",
            "Warning:".yellow().bold(),
            name
        );

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted");
            return Ok(());
        }
    }

    println!("{} Destroying VM: {}", "=>".blue().bold(), name);

    // First, try to shutdown
    let _ = Command::new("virsh").args(["destroy", name]).status();

    // Then undefine
    let status = Command::new("virsh")
        .args(["undefine", name, "--remove-all-storage"])
        .status()?;

    if status.success() {
        println!("{} VM {} destroyed", "✓".green().bold(), name);
    } else {
        anyhow::bail!("Failed to destroy VM {}", name);
    }

    Ok(())
}
