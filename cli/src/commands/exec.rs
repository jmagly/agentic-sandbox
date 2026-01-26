//! Command execution on agents

use anyhow::Result;
use colored::Colorize;
use std::collections::HashMap;

use crate::proto::{agent_service_client::AgentServiceClient, exec_output, ExecRequest};

pub async fn run(
    server: &str,
    agent_id: &str,
    command: &str,
    args: Vec<String>,
    stream: bool,
    timeout: u32,
) -> Result<()> {
    println!(
        "{} Executing on {}: {} {}",
        "=>".blue().bold(),
        agent_id.cyan(),
        command,
        args.join(" ")
    );

    // Connect to server
    let channel = tonic::transport::Channel::from_shared(server.to_string())?
        .connect()
        .await?;

    let mut client = AgentServiceClient::new(channel);

    // Build request
    let request = ExecRequest {
        agent_id: agent_id.to_string(),
        command: command.to_string(),
        args,
        working_dir: String::new(),
        env: HashMap::new(),
        timeout_seconds: timeout as i32,
    };

    // Execute
    let response = client.exec(request).await?;
    let mut output_stream = response.into_inner();

    // Process output
    while let Some(output) = output_stream.message().await? {
        let stream_type = exec_output::Stream::try_from(output.stream)
            .unwrap_or(exec_output::Stream::Unknown);

        match stream_type {
            exec_output::Stream::Stdout => {
                let text = String::from_utf8_lossy(&output.data);
                if stream {
                    print!("{}", text);
                } else {
                    println!("{}", text);
                }
            }
            exec_output::Stream::Stderr => {
                let text = String::from_utf8_lossy(&output.data);
                if stream {
                    eprint!("{}", text.red());
                } else {
                    eprintln!("{}", text.red());
                }
            }
            _ => {}
        }

        if output.complete {
            if output.exit_code == 0 {
                println!("{} Command completed (exit code 0)", "✓".green().bold());
            } else {
                println!(
                    "{} Command failed (exit code {})",
                    "✗".red().bold(),
                    output.exit_code
                );
                if !output.error.is_empty() {
                    println!("  Error: {}", output.error.red());
                }
            }
            break;
        }
    }

    Ok(())
}
