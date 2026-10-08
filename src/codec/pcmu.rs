//! G.711 μ-law (PCMU) codec implementation
//!
//! PCMU is a companding algorithm used in telephony that compresses
//! 14-bit linear PCM samples to 8-bit values using logarithmic encoding.
//!
//! Native sample rate: 8kHz
//! This implementation resamples to/from 16kHz for the internal PCM format,
//! using a 2x factor (8kHz * 2 = 16kHz).

use super::{Codec, TimestampIncrement};
use crate::SAMPLES_PER_FRAME;
use std::sync::OnceLock;

/// Upsample/downsample factor between the 8kHz wire format and the internal 16kHz.
const RESAMPLE_FACTOR: usize = 2;

/// PCMU decode table (256 entries, lazy-initialized)
static DECODE_TABLE: OnceLock<[i16; 256]> = OnceLock::new();

fn get_decode_table() -> &'static [i16; 256] {
    DECODE_TABLE.get_or_init(|| {
        let mut table = [0i16; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = decode_ulaw_sample(i as u8);
        }
        table
    })
}

/// Decode a single μ-law byte to linear PCM
fn decode_ulaw_sample(byte: u8) -> i16 {
    // μ-law uses biased linear input
    // Format: SEEEMMMM where S=sign, EEE=exponent, MMMM=mantissa
    let byte = !byte; // Invert all bits (μ-law is transmitted inverted)

    let sign = (byte & 0x80) != 0;
    let exponent = ((byte >> 4) & 0x07) as i32;
    let mantissa = (byte & 0x0F) as i32;

    // Reconstruct the magnitude
    // Add 1/2 LSB (0x84 = bias), shift by exponent, subtract bias
    let magnitude = ((mantissa << 1) | 0x21) << (exponent + 2);
    let magnitude = magnitude - 0x84;

    if sign {
        -(magnitude as i16)
    } else {
        magnitude as i16
    }
}

/// Encode a single linear PCM sample to μ-law byte
fn encode_ulaw_sample(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    let sign: u8;
    let mut sample = sample as i32;

    // Get sign and make positive
    if sample < 0 {
        sign = 0x80;
        sample = -sample;
    } else {
        sign = 0;
    }

    // Clip to valid range
    if sample > CLIP {
        sample = CLIP;
    }

    // Add bias
    sample += BIAS;

    // Find exponent and mantissa
    let exponent = ((sample as u32).leading_zeros() as i32 - 17).clamp(0, 7);
    let exponent = 7 - exponent;

    let mantissa = (sample >> (exponent + 3)) & 0x0F;

    // Combine and invert
    let byte = sign | ((exponent as u8) << 4) | (mantissa as u8);
    !byte
}

/// G.711 μ-law codec
pub struct PcmuCodec {
    decode_table: &'static [i16; 256],
}

impl PcmuCodec {
    pub fn new() -> Self {
        Self {
            decode_table: get_decode_table(),
        }
    }
}

impl Default for PcmuCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for PcmuCodec {
    fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        // Each byte becomes one 8kHz sample, which we upsample 2x to 16kHz
        let mut samples = Vec::with_capacity(payload.len() * RESAMPLE_FACTOR);

        for &byte in payload {
            let sample = self.decode_table[byte as usize];
            // Simple 2x upsampling by sample duplication
            for _ in 0..RESAMPLE_FACTOR {
                samples.push(sample);
            }
        }

        samples
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        // Downsample from 16kHz to 8kHz (take one sample of every 2)
        let mut payload = Vec::with_capacity(samples.len().div_ceil(RESAMPLE_FACTOR));

        for chunk in samples.chunks(RESAMPLE_FACTOR) {
            // Use the middle sample for better quality
            let sample = chunk[chunk.len() / 2];
            payload.push(encode_ulaw_sample(sample));
        }

        payload
    }

    fn samples_per_frame(&self) -> usize {
        SAMPLES_PER_FRAME
    }

    fn rtp_timestamp_increment(&self) -> TimestampIncrement {
        // 8kHz RTP clock (RFC 3551): 20ms = 160 ticks
        TimestampIncrement::new(160)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_encode_roundtrip() {
        let mut codec = PcmuCodec::new();

        // Standard 20ms frame at 8kHz = 160 bytes
        let original: Vec<u8> = (0..160).map(|i| (i * 17) as u8).collect();

        let decoded = codec.decode(&original);
        // 160 bytes * 2 = 320 samples at 16kHz
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);

        let encoded = codec.encode(&decoded);
        // Should get 160 bytes back
        assert_eq!(encoded.len(), 160);

        // μ-law is a lossy codec, so we verify the decoded audio is similar
        // by checking that re-decoding the encoded data produces similar PCM values.
        // The byte values themselves may differ due to quantization.
        let mut codec2 = PcmuCodec::new();
        let redecoded = codec2.decode(&encoded);

        // Compare every 2nd sample (matching the original 8kHz rate)
        for i in 0..160 {
            let orig_sample = decoded[i * RESAMPLE_FACTOR];
            let new_sample = redecoded[i * RESAMPLE_FACTOR];
            let diff = (orig_sample as i32 - new_sample as i32).abs();
            // μ-law has about 14-bit dynamic range, allow some quantization error
            assert!(
                diff < 500,
                "Sample {} differs too much: {} vs {}",
                i,
                orig_sample,
                new_sample
            );
        }
    }

    #[test]
    fn test_decode_silence() {
        let mut codec = PcmuCodec::new();

        // μ-law silence is 0xFF (inverted 0x00)
        let silence = vec![0xFF; 160];
        let decoded = codec.decode(&silence);

        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
        // All samples should be very close to 0
        for sample in decoded {
            assert!(sample.abs() < 10, "Expected near-silence, got {}", sample);
        }
    }

    #[test]
    fn test_encode_silence() {
        let mut codec = PcmuCodec::new();

        let silence = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = codec.encode(&silence);

        assert_eq!(encoded.len(), 160);
        // All bytes should be μ-law silence (0xFF)
        for byte in encoded {
            assert_eq!(byte, 0xFF);
        }
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = PcmuCodec::new();
        assert_eq!(codec.samples_per_frame(), SAMPLES_PER_FRAME);
        assert_eq!(codec.samples_per_frame(), 320);
    }

    #[test]
    fn test_decode_single_sample() {
        // Test specific μ-law values
        assert_eq!(decode_ulaw_sample(0xFF), 0); // Silence
        assert_ne!(decode_ulaw_sample(0x00), 0); // Max positive
        assert_ne!(decode_ulaw_sample(0x80), 0); // Max negative
    }
}
