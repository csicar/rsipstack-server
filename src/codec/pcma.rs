//! G.711 A-law (PCMA) codec implementation
//!
//! PCMA is a companding algorithm used in telephony (primarily in Europe)
//! that compresses 13-bit linear PCM samples to 8-bit values.
//!
//! Native sample rate: 8kHz
//! This implementation resamples to/from 48kHz for the internal PCM format.

use super::Codec;
use std::sync::OnceLock;

/// PCMA decode table (256 entries, lazy-initialized)
static DECODE_TABLE: OnceLock<[i16; 256]> = OnceLock::new();

fn get_decode_table() -> &'static [i16; 256] {
    DECODE_TABLE.get_or_init(|| {
        let mut table = [0i16; 256];
        for i in 0..256 {
            table[i] = decode_alaw_sample(i as u8);
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
        let mut payload = Vec::with_capacity((samples.len() + 5) / 6);

        for chunk in samples.chunks(6) {
            // Use the middle sample for better quality
            let sample = chunk[chunk.len() / 2];
            payload.push(encode_alaw_sample(sample));
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
        let mut codec = PcmaCodec::new();

        // Standard 20ms frame at 8kHz = 160 bytes
        let original: Vec<u8> = (0..160).map(|i| (i * 17) as u8).collect();

        let decoded = codec.decode(&original);
        // 160 bytes * 6 = 960 samples at 48kHz
        assert_eq!(decoded.len(), 960);

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

        assert_eq!(decoded.len(), 960);
        // All samples should be very close to 0
        for sample in decoded {
            assert!(sample.abs() < 100, "Expected near-silence, got {}", sample);
        }
    }

    #[test]
    fn test_encode_silence() {
        let mut codec = PcmaCodec::new();

        let silence = vec![0i16; 960];
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
        assert_eq!(codec.samples_per_frame(), 960);
    }

    #[test]
    fn test_decode_single_sample() {
        // Test specific A-law values
        let sample = decode_alaw_sample(0xD5); // Silence
        assert!(sample.abs() < 100, "Silence should decode to near-zero");
    }
}
