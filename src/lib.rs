//! A generic VoIP server library built on rsipstack.
//!
//! This library provides a SIP server that accepts incoming calls and lets you
//! define custom audio processing through the [`AudioHandler`] trait.
//!
//! # Example
//!
//! ```no_run
//! use rsipstack_server::{SipServer, ServerConfig, AudioHandler, AudioFrame};
//! use tokio::sync::mpsc;
//! use tokio_util::sync::CancellationToken;
//! use async_trait::async_trait;
//!
//! // Define your audio handler
//! struct EchoHandler;
//!
//! #[async_trait]
//! impl AudioHandler for EchoHandler {
//!     async fn process(
//!         &self,
//!         mut audio_in: mpsc::UnboundedReceiver<AudioFrame>,
//!         audio_out: mpsc::UnboundedSender<AudioFrame>,
//!         cancel_token: CancellationToken,
//!     ) {
//!         loop {
//!             tokio::select! {
//!                 _ = cancel_token.cancelled() => break,
//!                 frame = audio_in.recv() => {
//!                     match frame {
//!                         Some(f) => { let _ = audio_out.send(f); }
//!                         None => break,
//!                     }
//!                 }
//!             }
//!         }
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = ServerConfig::default();
//!     let server = SipServer::new(config, || EchoHandler).await?;
//!     server.run().await?;
//!     Ok(())
//! }
//! ```

mod audio;
mod call_handler;
pub mod codec;
mod media;
mod server;

// Re-export public API
pub use audio::handler::AudioHandler;
pub use media::rtp::AudioFrame;
pub use server::{ServerConfig, ServerState, SipServer};

// Re-export useful types from dependencies
pub use async_trait::async_trait;
pub use tokio::sync::mpsc;
pub use tokio_util::sync::CancellationToken;
