//! SIP Echo Server - Entry point
//!
//! A SIP server that accepts all incoming calls and echoes audio back to the caller.

mod audio;
mod call_handler;
mod media;
mod server;

use clap::Parser;
use std::net::IpAddr;
use tracing::info;

/// SIP Echo Server - Accepts calls and echoes audio back
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// SIP listening port
    #[arg(long, default_value = "5060")]
    port: u16,

    /// Bind address (defaults to first non-loopback interface)
    #[arg(long)]
    bind: Option<IpAddr>,

    /// External IP address (for NAT traversal)
    #[arg(long)]
    external_ip: Option<IpAddr>,

    /// RTP port range start (even number)
    #[arg(long, default_value = "10000")]
    rtp_start_port: u16,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&args.log_level)),
        )
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting SIP Echo Server");
    info!("SIP port: {}", args.port);
    info!("RTP start port: {}", args.rtp_start_port);

    let server_config = server::ServerConfig {
        port: args.port,
        bind_addr: args.bind,
        external_ip: args.external_ip,
        rtp_start_port: args.rtp_start_port,
    };

    let server = server::SipServer::new(server_config).await?;
    server.run().await?;

    Ok(())
}
