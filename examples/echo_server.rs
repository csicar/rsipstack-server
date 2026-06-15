//! SIP Echo Server Example
//!
//! A SIP server that accepts all incoming calls and echoes audio back to the caller.
//! Useful for testing VoIP clients, network connectivity, and audio quality.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example echo_server -- --port 5060
//! ```

use clap::Parser;
use rsipstack_server::{
    async_trait, mpsc, AudioFrame, AudioHandler, CancellationToken, ServerConfig, SipHeaders,
    SipServer,
};
use std::net::IpAddr;
use tracing::{debug, info, trace};

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

    /// The first  
    #[arg(long, default_value = "10000")]
    first_rtp_port: u16,

    /// Last RTCP port (uneven number)
    #[arg(long, default_value = "10099")]
    last_rtcp_port: u16,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// Echo handler that forwards all received audio back to the sender
///
/// This is the simplest audio handler - it takes incoming audio frames
/// and sends them right back out. This creates an echo effect for the caller.
pub struct EchoHandler;

impl EchoHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EchoHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioHandler for EchoHandler {
    async fn process(
        &self,
        mut audio_in: mpsc::UnboundedReceiver<AudioFrame>,
        audio_out: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
        _headers: SipHeaders,
    ) {
        debug!("Echo handler started");
        let mut frame_count = 0u64;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!("Echo handler cancelled after {} frames", frame_count);
                    break;
                }
                frame = audio_in.recv() => {
                    match frame {
                        Some(frame) => {
                            frame_count += 1;
                            trace!(
                                samples = frame.samples.len(),
                                "Echoing frame"
                            );

                            // Send the frame right back (echo it)
                            if audio_out.send(AudioFrame { samples: frame.samples }).is_err() {
                                debug!("Output channel closed, stopping echo handler");
                                break;
                            }
                        }
                        None => {
                            debug!("Input channel closed after {} frames", frame_count);
                            break;
                        }
                    }
                }
            }
        }

        debug!("Echo handler stopped, processed {} frames", frame_count);
    }
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
    info!("RTP start port: {}", args.first_rtp_port);
    info!("RTP end port: {}", args.last_rtcp_port);

    let server_config = ServerConfig {
        port: args.port,
        bind_addr: args.bind,
        external_ip: args.external_ip,
        min_port: args.first_rtp_port,
        max_port: args.last_rtcp_port,
    };

    let server = SipServer::new(server_config, EchoHandler::new).await?;
    server.run().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_echo_handler() {
        let handler = EchoHandler::new();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();

        let cancel_clone = cancel_token.clone();
        let handle = tokio::spawn(async move {
            handler.process(in_rx, out_tx, cancel_clone, vec![]).await;
        });

        // Send a test frame (960 samples at 48kHz = 20ms)
        let frame = AudioFrame {
            samples: vec![100; 960],
        };
        in_tx.send(frame.clone()).unwrap();

        // Receive the echoed frame
        let echoed = out_rx.recv().await.unwrap();
        assert_eq!(echoed.samples, frame.samples);

        // Clean up
        cancel_token.cancel();
        handle.await.unwrap();
    }
}
