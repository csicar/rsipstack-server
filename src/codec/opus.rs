//! Opus codec implementation
//!
//! Opus is a modern audio codec designed for interactive real-time applications.
//! It natively supports 48kHz sample rate, so no resampling is needed.
//!
//! This implementation uses the `opus` crate for encoding/decoding.

use super::Codec;
use opus::{Channels, Decoder, Encoder};

/// Opus codec for 48kHz mono audio
pub struct OpusCodec {
    encoder: Encoder,
    decoder: Decoder,
}

impl OpusCodec {
    /// Create a new Opus codec instance
    ///
    /// Returns an error if the opus encoder/decoder cannot be initialized.
    pub fn new() -> Result<Self, opus::Error> {
        // 48kHz mono for VoIP applications
        let encoder = Encoder::new(48000, Channels::Mono, opus::Application::Voip)?;
        let decoder = Decoder::new(48000, Channels::Mono)?;

        Ok(Self { encoder, decoder })
    }
}

impl Codec for OpusCodec {
    fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        // Opus frames are typically 20ms = 960 samples at 48kHz
        let mut output = vec![0i16; 960];

        match self.decoder.decode(payload, &mut output, false) {
            Ok(samples) => {
                output.truncate(samples);
                output
            }
            Err(e) => {
                tracing::warn!("Opus decode error: {}", e);
                // Return silence on error
                vec![0i16; 960]
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
        // 20ms at 48kHz = 960 samples
        960
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
        let samples: Vec<i16> = (0..960)
            .map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16)
            .collect();

        let encoded = codec.encode(&samples);
        assert!(!encoded.is_empty(), "Encoded data should not be empty");

        let decoded = codec.decode(&encoded);
        assert_eq!(decoded.len(), 960, "Decoded frame should have 960 samples");

        // Opus is lossy, so we just check the signal is reasonable
        // (not silent and within range)
        let max_sample = decoded.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(max_sample > 1000, "Decoded signal should not be silent");
    }

    #[test]
    fn test_opus_decode_silence() {
        let mut codec = OpusCodec::new().unwrap();

        let silence = vec![0i16; 960];
        let encoded = codec.encode(&silence);

        let decoded = codec.decode(&encoded);
        assert_eq!(decoded.len(), 960);

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
        assert_eq!(codec.samples_per_frame(), 960);
    }
}
