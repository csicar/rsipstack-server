//! Audio Handler trait definition

use crate::media::rtp::AudioFrame;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Trait for audio handlers that process incoming audio and produce outgoing audio
///
/// Audio handlers receive audio frames from the RTP layer via the `audio_in` channel
/// and send processed audio frames back via the `audio_out` channel.
///
/// Implementations should run until the cancellation token is triggered or
/// the input channel is closed.
#[async_trait]
pub trait AudioHandler: Send + Sync {
    /// Process audio frames
    ///
    /// # Arguments
    ///
    /// * `audio_in` - Channel receiver for incoming audio frames
    /// * `audio_out` - Channel sender for outgoing audio frames
    /// * `cancel_token` - Token to signal when processing should stop
    async fn process(
        &self,
        audio_in: mpsc::UnboundedReceiver<AudioFrame>,
        audio_out: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
    );

    /// Get the name of this handler (for logging)
    #[allow(dead_code)]
    fn name(&self) -> &'static str {
        "AudioHandler"
    }
}
