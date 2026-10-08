//! G.711 A-law (PCMA) codec implementation
//!
//! PCMA is a companding algorithm used in telephony (primarily in Europe)
//! that compresses 13-bit linear PCM samples to 8-bit values.
//!
//! Native sample rate: 8kHz
//! This implementation resamples to/from 16kHz for the internal PCM format,
//! using a 2x factor (8kHz * 2 = 16kHz).

use super::{Codec, TimestampIncrement};
use crate::SAMPLES_PER_FRAME;
use std::sync::OnceLock;

/// Upsample/downsample factor between the 8kHz wire format and the internal 16kHz.
const RESAMPLE_FACTOR: usize = 2;

/// PCMA decode table (256 entries, lazy-initialized)
static DECODE_TABLE: OnceLock<[i16; 256]> = OnceLock::new();

fn get_decode_table() -> &'static [i16; 256] {
    DECODE_TABLE.get_or_init(|| {
        let mut table = [0i16; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = decode_alaw_sample(i as u8);
        }
        table
    })
}

/// Decode a single A-law byte to linear PCM
fn decode_alaw_sample(byte: u8) -> i16 {
    // A-law format: SEEEMMMM where S=sign, EEE=exponent, MMMM=mantissa
    // Even bits are inverted in transmission
    let byte = byte ^ 0x55;

    let sign = (byte & 0x80) != 0;
    let exponent = ((byte >> 4) & 0x07) as i32;
    let mantissa = (byte & 0x0F) as i32;

    let magnitude = if exponent == 0 {
        // For exponent 0, linear segment
        (mantissa << 1) | 1
    } else {
        // For exponent > 0, add implicit 1 and shift
        ((mantissa << 1) | 0x21) << (exponent - 1)
    };

    // Scale to 16-bit range (A-law is 13-bit, so shift left by 3)
    let magnitude = (magnitude << 3) as i16;

    if sign {
        -magnitude
    } else {
        magnitude
    }
}

/// Encode a single linear PCM sample to A-law byte
fn encode_alaw_sample(sample: i16) -> u8 {
    let sign: u8;
    let mut sample = sample as i32;

    // Get sign and make positive
    if sample < 0 {
        sign = 0x80;
        sample = -sample;
    } else {
        sign = 0;
    }

    // Scale from 16-bit to 13-bit
    sample >>= 3;

    // Clip to valid range
    if sample > 4095 {
        sample = 4095;
    }

    let (exponent, mantissa) = if sample < 32 {
        // Linear segment (exponent 0)
        (0, (sample >> 1) as u8)
    } else {
        // Find exponent
        let mut exp = 1;
        let mut seg = sample >> 1;
        while seg >= 32 && exp < 7 {
            seg >>= 1;
            exp += 1;
        }
        let mant = (seg - 16) as u8 & 0x0F;
        (exp, mant)
    };

    // Combine components
    let byte = sign | ((exponent as u8) << 4) | mantissa;
    // Toggle even bits for transmission
    byte ^ 0x55
}

/// G.711 A-law codec
pub struct PcmaCodec {
    decode_table: &'static [i16; 256],
}

impl PcmaCodec {
    pub fn new() -> Self {
        Self {
            decode_table: get_decode_table(),
        }
    }
}

impl Default for PcmaCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for PcmaCodec {
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
            payload.push(encode_alaw_sample(sample));
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
        let mut codec = PcmaCodec::new();

        // Standard 20ms frame at 8kHz = 160 bytes
        let original: Vec<u8> = (0..160).map(|i| (i * 17) as u8).collect();

        let decoded = codec.decode(&original);
        // 160 bytes * 2 = 320 samples at 16kHz
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);

        let encoded = codec.encode(&decoded);
        // Should get 160 bytes back
        assert_eq!(encoded.len(), 160);

        // Values should be close (not exact due to lossy encoding)
        for (orig, enc) in original.iter().zip(encoded.iter()) {
            let diff = (*orig as i16 - *enc as i16).abs();
            assert!(diff <= 1, "Difference too large: {} vs {}", orig, enc);
        }
    }

    #[test]
    fn test_decode_silence() {
        let mut codec = PcmaCodec::new();

        // A-law silence is 0xD5 (0x80 ^ 0x55)
        let silence = vec![0xD5; 160];
        let decoded = codec.decode(&silence);

        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
        // All samples should be very close to 0
        for sample in decoded {
            assert!(sample.abs() < 100, "Expected near-silence, got {}", sample);
        }
    }

    #[test]
    fn test_encode_silence() {
        let mut codec = PcmaCodec::new();

        let silence = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = codec.encode(&silence);

        assert_eq!(encoded.len(), 160);
        // A-law encodes 0 with XOR 0x55: (0 ^ 0x55) = 0x55
        for byte in encoded {
            assert_eq!(byte, 0x55, "Expected A-law silence byte");
        }
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = PcmaCodec::new();
        assert_eq!(codec.samples_per_frame(), SAMPLES_PER_FRAME);
        assert_eq!(codec.samples_per_frame(), 320);
    }

    #[test]
    fn test_decode_single_sample() {
        // Test specific A-law values
        let sample = decode_alaw_sample(0xD5); // Silence
        assert!(sample.abs() < 100, "Silence should decode to near-zero");
    }
}
