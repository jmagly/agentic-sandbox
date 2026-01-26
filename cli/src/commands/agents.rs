//! List connected agents

use anyhow::Result;
use colored::Colorize;

pub async fn list(_server: &str, verbose: bool) -> Result<()> {
    println!("{}", "Connected Agents".bold());
    println!("{}", "─".repeat(60));

    // TODO: Implement gRPC call to get agent list
    // For now, show placeholder
    println!("{}", "Agent listing requires server connection".yellow());
    println!("Use 'agentic-sandbox server status' to check server");

    if verbose {
        println!("\nVerbose mode would show:");
        println!("  - Agent ID");
        println!("  - Hostname");
        println!("  - IP Address");
        println!("  - Status");
        println!("  - Connected At");
        println!("  - Last Heartbeat");
    }

    Ok(())
}
