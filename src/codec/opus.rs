//! Opus codec implementation
//!
//! Opus is a modern audio codec designed for interactive real-time applications.
//! It supports 16kHz directly, so the encoder and decoder run at the internal
//! PCM rate and no resampling is needed here. Only the RTP clock stays at 48kHz
//! (RFC 7587), independent of the sample rate, see `rtp_timestamp_increment`.
//!
//! This implementation uses the `opus` crate for encoding/decoding.

use super::{Codec, TimestampIncrement};
use crate::{SAMPLES_PER_FRAME, SAMPLE_RATE_HZ};
use opus::{Channels, Decoder, Encoder};

/// Opus codec for 16kHz mono audio
pub struct OpusCodec {
    encoder: Encoder,
    decoder: Decoder,
}

impl OpusCodec {
    /// Create a new Opus codec instance
    ///
    /// Returns an error if the opus encoder/decoder cannot be initialized.
    pub fn new() -> Result<Self, opus::Error> {
        // 16kHz mono for VoIP applications
        let encoder = Encoder::new(SAMPLE_RATE_HZ, Channels::Mono, opus::Application::Voip)?;
        let decoder = Decoder::new(SAMPLE_RATE_HZ, Channels::Mono)?;

        Ok(Self { encoder, decoder })
    }
}

impl Codec for OpusCodec {
    fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        // Opus frames are typically 20ms = 320 samples at 16kHz
        let mut output = vec![0i16; SAMPLES_PER_FRAME];

        match self.decoder.decode(payload, &mut output, false) {
            Ok(samples) => {
                output.truncate(samples);
                output
            }
            Err(e) => {
                tracing::warn!("Opus decode error: {}", e);
                // Return silence on error
                vec![0i16; SAMPLES_PER_FRAME]
            }
        }
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        // Maximum Opus frame size
        let mut output = vec![0u8; 4000];

        match self.encoder.encode(samples, &mut output) {
            Ok(len) => {
                output.truncate(len);
                output
            }
            Err(e) => {
                tracing::warn!("Opus encode error: {}", e);
                // Return empty on error (will be dropped)
                Vec::new()
            }
        }
    }

    fn samples_per_frame(&self) -> usize {
        SAMPLES_PER_FRAME
    }

    fn rtp_timestamp_increment(&self) -> TimestampIncrement {
        // The RTP clock is 48kHz (RFC 7587) whatever rate the codec runs at:
        // 20ms = 960 ticks
        TimestampIncrement::new(960)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus_codec_creation() {
        let codec = OpusCodec::new();
        assert!(codec.is_ok());
    }

    #[test]
    fn test_opus_encode_decode_roundtrip() {
        let mut codec = OpusCodec::new().unwrap();

        // Generate a simple test signal (sine wave)
        let samples: Vec<i16> = (0..SAMPLES_PER_FRAME)
            .map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16)
            .collect();

        let encoded = codec.encode(&samples);
        assert!(!encoded.is_empty(), "Encoded data should not be empty");

        let decoded = codec.decode(&encoded);
        assert_eq!(
            decoded.len(),
            SAMPLES_PER_FRAME,
            "Decoded frame should have {} samples",
            SAMPLES_PER_FRAME
        );

        // Opus is lossy, so we just check the signal is reasonable
        // (not silent and within range)
        let max_sample = decoded.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(max_sample > 1000, "Decoded signal should not be silent");
    }

    #[test]
    fn test_opus_decode_silence() {
        let mut codec = OpusCodec::new().unwrap();

        let silence = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = codec.encode(&silence);

        let decoded = codec.decode(&encoded);
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);

        // Should be near-silent
        let max_sample = decoded.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(
            max_sample < 1000,
            "Decoded silence should be quiet, got max: {}",
            max_sample
        );
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = OpusCodec::new().unwrap();
        assert_eq!(codec.samples_per_frame(), SAMPLES_PER_FRAME);
        assert_eq!(codec.samples_per_frame(), 320);
    }
}
