use std::path::PathBuf;
use std::time::Duration;

use agentic_management::grpc_ca_backend::local_ca_options_from_env;
use agentic_management::grpc_local_ca::{EmbeddedGrpcCa, LocalCaOptions};
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

        /// Agent leaf certificate TTL in seconds.
        #[arg(long)]
        ttl_secs: Option<u64>,

        /// Renew an existing leaf when it expires within this many seconds.
        #[arg(long)]
        renew_before_secs: Option<u64>,
    },
    /// Load or create the embedded local CA and issue one server leaf.
    IssueServer {
        /// Embedded local CA directory.
        #[arg(long)]
        ca_dir: PathBuf,

        /// SPIFFE trust domain associated with this local CA.
        #[arg(long)]
        trust_domain: String,

        /// Server DNS name to place in the subjectAltName extension.
        #[arg(long)]
        dns_name: String,

        /// Output server certificate path.
        #[arg(long)]
        cert: PathBuf,

        /// Output server private key path.
        #[arg(long)]
        key: PathBuf,

        /// Server leaf certificate TTL in seconds.
        #[arg(long)]
        ttl_secs: Option<u64>,

        /// Renew an existing leaf when it expires within this many seconds.
        #[arg(long)]
        renew_before_secs: Option<u64>,
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
            ttl_secs,
            renew_before_secs,
        } => {
            let instance_id = Uuid::parse_str(&instance_id)
                .with_context(|| format!("invalid agent instance UUID `{instance_id}`"))?;
            let ca = EmbeddedGrpcCa::load_or_create_with_options(
                &ca_dir,
                &trust_domain,
                options_with_overrides(ttl_secs, None, renew_before_secs)?,
            )?;
            let spiffe_id = format!("spiffe://{trust_domain}/agent/{instance_id}");
            let leaf = ca.load_or_issue_agent_leaf(&spiffe_id, cert, key)?;
            println!("root_cert={}", ca.root_cert_path().display());
            println!("agent_cert={}", leaf.cert_path.display());
            println!("agent_key={}", leaf.key_path.display());
            println!("spiffe_id={}", leaf.spiffe_id);
        }
        Command::IssueServer {
            ca_dir,
            trust_domain,
            dns_name,
            cert,
            key,
            ttl_secs,
            renew_before_secs,
        } => {
            let ca = EmbeddedGrpcCa::load_or_create_with_options(
                &ca_dir,
                &trust_domain,
                options_with_overrides(None, ttl_secs, renew_before_secs)?,
            )?;
            ca.load_or_issue_server_leaf(&dns_name, &cert, &key)?;
            println!("root_cert={}", ca.root_cert_path().display());
            println!("server_cert={}", cert.display());
            println!("server_key={}", key.display());
            println!("dns_name={dns_name}");
        }
    }

    Ok(())
}

fn options_with_overrides(
    agent_ttl_secs: Option<u64>,
    server_ttl_secs: Option<u64>,
    renew_before_secs: Option<u64>,
) -> Result<LocalCaOptions> {
    let mut options = local_ca_options_from_env()?;
    if let Some(value) = agent_ttl_secs {
        options.agent_leaf_ttl = checked_duration(value, "--ttl-secs")?;
    }
    if let Some(value) = server_ttl_secs {
        options.server_leaf_ttl = checked_duration(value, "--ttl-secs")?;
    }
    if let Some(value) = renew_before_secs {
        options.renew_before = checked_duration(value, "--renew-before-secs")?;
    }
    Ok(options)
}

fn checked_duration(value: u64, name: &str) -> Result<Duration> {
    if value == 0 {
        anyhow::bail!("{name} must be greater than zero");
    }
    Ok(Duration::from_secs(value))
}
