use std::path::PathBuf;

use agentic_management::grpc_local_ca::EmbeddedGrpcCa;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "grpc-local-ca",
    about = "Issue local gRPC mTLS credentials for agent provisioning"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Load or create the embedded local CA and issue one agent client leaf.
    IssueAgent {
        /// Embedded local CA directory.
        #[arg(long)]
        ca_dir: PathBuf,

        /// SPIFFE trust domain, for example sandbox.agentic.local.
        #[arg(long)]
        trust_domain: String,

        /// Agent instance UUID used in the SPIFFE URI-SAN.
        #[arg(long)]
        instance_id: String,

        /// Output client certificate path.
        #[arg(long)]
        cert: PathBuf,

        /// Output client private key path.
        #[arg(long)]
        key: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::IssueAgent {
            ca_dir,
            trust_domain,
            instance_id,
            cert,
            key,
        } => {
            let instance_id = Uuid::parse_str(&instance_id)
                .with_context(|| format!("invalid agent instance UUID `{instance_id}`"))?;
            let ca = EmbeddedGrpcCa::load_or_create(&ca_dir, &trust_domain)?;
            let spiffe_id = format!("spiffe://{trust_domain}/agent/{instance_id}");
            let leaf = ca.load_or_issue_agent_leaf(&spiffe_id, cert, key)?;
            println!("root_cert={}", ca.root_cert_path().display());
            println!("agent_cert={}", leaf.cert_path.display());
            println!("agent_key={}", leaf.key_path.display());
            println!("spiffe_id={}", leaf.spiffe_id);
        }
    }

    Ok(())
}
