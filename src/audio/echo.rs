//! Echo Audio Handler - Simply echoes received audio back

use super::handler::AudioHandler;
use crate::media::rtp::AudioFrame;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace};

/// Echo handler that forwards all received audio back to the sender
///
/// This is the simplest audio handler - it takes incoming audio frames
/// and sends them right back out. This creates an echo effect for the caller.
pub struct EchoHandler;

impl EchoHandler {
    /// Create a new echo handler
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
                                seq = frame.sequence,
                                ts = frame.timestamp,
                                len = frame.payload.len(),
                                "Echoing frame"
                            );

                            // Send the frame right back (echo it)
                            if audio_out.send(frame).is_err() {
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

    fn name(&self) -> &'static str {
        "EchoHandler"
    }
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
            handler.process(in_rx, out_tx, cancel_clone).await;
        });

        // Send a test frame
        let frame = AudioFrame {
            payload: vec![0x55; 160],
            timestamp: 160,
            sequence: 1,
            ssrc: 12345,
            payload_type: 0,
        };
        in_tx.send(frame.clone()).unwrap();

        // Receive the echoed frame
        let echoed = out_rx.recv().await.unwrap();
        assert_eq!(echoed.payload, frame.payload);
        assert_eq!(echoed.sequence, frame.sequence);
        assert_eq!(echoed.timestamp, frame.timestamp);

        // Clean up
        cancel_token.cancel();
        handle.await.unwrap();
    }
}
