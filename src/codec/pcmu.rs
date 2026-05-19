//! G.711 μ-law (PCMU) codec implementation
//!
//! PCMU is a companding algorithm used in telephony that compresses
//! 14-bit linear PCM samples to 8-bit values using logarithmic encoding.
//!
//! Native sample rate: 8kHz
//! This implementation resamples to/from 48kHz for the internal PCM format.

use super::Codec;
use std::sync::OnceLock;

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
        // Each byte becomes one 8kHz sample, which we upsample 6x to 48kHz
        let mut samples = Vec::with_capacity(payload.len() * 6);

        for &byte in payload {
            let sample = self.decode_table[byte as usize];
            // Simple 6x upsampling by sample duplication
            for _ in 0..6 {
                samples.push(sample);
            }
        }

        samples
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        // Downsample from 48kHz to 8kHz (take every 6th sample)
        let mut payload = Vec::with_capacity(samples.len().div_ceil(6));

        for chunk in samples.chunks(6) {
            // Use the middle sample for better quality
            let sample = chunk[chunk.len() / 2];
            payload.push(encode_ulaw_sample(sample));
        }

        payload
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
    fn test_decode_encode_roundtrip() {
        let mut codec = PcmuCodec::new();

        // Standard 20ms frame at 8kHz = 160 bytes
        let original: Vec<u8> = (0..160).map(|i| (i * 17) as u8).collect();

        let decoded = codec.decode(&original);
        // 160 bytes * 6 = 960 samples at 48kHz
        assert_eq!(decoded.len(), 960);

        let encoded = codec.encode(&decoded);
        // Should get 160 bytes back
        assert_eq!(encoded.len(), 160);

        // μ-law is a lossy codec, so we verify the decoded audio is similar
        // by checking that re-decoding the encoded data produces similar PCM values.
        // The byte values themselves may differ due to quantization.
        let mut codec2 = PcmuCodec::new();
        let redecoded = codec2.decode(&encoded);

        // Compare every 6th sample (matching the original 8kHz rate)
        for i in 0..160 {
            let orig_sample = decoded[i * 6];
            let new_sample = redecoded[i * 6];
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

        assert_eq!(decoded.len(), 960);
        // All samples should be very close to 0
        for sample in decoded {
            assert!(sample.abs() < 10, "Expected near-silence, got {}", sample);
        }
    }

    #[test]
    fn test_encode_silence() {
        let mut codec = PcmuCodec::new();

        let silence = vec![0i16; 960];
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
        assert_eq!(codec.samples_per_frame(), 960);
    }

    #[test]
    fn test_decode_single_sample() {
        // Test specific μ-law values
        assert_eq!(decode_ulaw_sample(0xFF), 0); // Silence
        assert_ne!(decode_ulaw_sample(0x00), 0); // Max positive
        assert_ne!(decode_ulaw_sample(0x80), 0); // Max negative
    }
}
