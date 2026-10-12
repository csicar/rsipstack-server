//! L16 codec implementation
//!
//! L16 (RFC 3551 §4.5.11) is uncompressed linear PCM: each sample is
//! transmitted as-is, as a 16-bit big-endian ("network byte order") integer.
//! There is no encoding step, unlike G.722's ADPCM - `to_be_bytes`/
//! `from_be_bytes` are the entire codec.
//!
//! Unlike PCMU/PCMA/G.722, L16 at 16kHz has no static RTP payload type, so it
//! is always negotiated on a dynamic payload type (like Opus), identified by
//! its `a=rtpmap` name rather than a fixed number.
//!
//! Native sample rate: 16kHz, mono.
//! No resampling is needed: the internal PCM format is also 16kHz mono.

use super::{Codec, TimestampIncrement};

pub struct L16Codec;

impl L16Codec {
    pub fn new() -> Self {
        Self
    }
}

impl Default for L16Codec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for L16Codec {
    fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        payload
            .chunks_exact(2)
            .map(|chunk| {
                i16::from_be_bytes(
                    chunk
                        .try_into()
                        .expect("chunks_exact(2) guarantees len == 2"),
                )
            })
            .collect()
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_be_bytes()).collect()
    }

    fn samples_per_frame(&self) -> usize {
        crate::SAMPLES_PER_FRAME
    }

    fn rtp_timestamp_increment(&self) -> TimestampIncrement {
        // 16kHz RTP clock: 20ms = 320 ticks
        TimestampIncrement::new(320)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SAMPLES_PER_FRAME;

    #[test]
    fn test_frame_sizes() {
        let mut codec = L16Codec;

        // A 20ms frame at 16kHz = 320 samples.
        let frame = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = codec.encode(&frame);
        // L16 is uncompressed, so each sample is 2 bytes (unlike G722's ~1 byte
        // per 2 samples).
        assert_eq!(encoded.len(), SAMPLES_PER_FRAME * 2);

        let decoded = codec.decode(&encoded);
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
    }

    #[test]
    fn test_decode_silence() {
        let mut codec = L16Codec;

        // L16 has no lossy compression step (unlike G722's ADPCM), so
        // silence round-trips to exact silence, not just "near" silence.
        let encoded = codec.encode(&vec![0i16; SAMPLES_PER_FRAME]);
        let decoded = codec.decode(&encoded);

        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
        assert!(decoded.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_encode_silence() {
        let mut codec = L16Codec;

        let silence = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = codec.encode(&silence);

        assert_eq!(encoded.len(), SAMPLES_PER_FRAME * 2);
        assert!(encoded.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_decode_encode_roundtrip() {
        let mut codec = L16Codec;

        // L16 has no compression step and no resampling, so encoding then
        // decoding any frame is lossless and must reproduce it exactly.
        let original: Vec<i16> = (0..SAMPLES_PER_FRAME as i16)
            .map(|i| ((i * 97) % 30000) - 15000)
            .collect();
        assert_eq!(original.len(), SAMPLES_PER_FRAME);

        let encoded = codec.encode(&original);
        assert_eq!(encoded.len(), SAMPLES_PER_FRAME * 2);

        let decoded = codec.decode(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_big_endian_byte_order() {
        let mut codec = L16Codec;

        // RFC 3551 requires L16 samples in network byte order (big-endian):
        // 0x0001 must decode to 1, not 256 (which would be little-endian).
        let decoded = codec.decode(&[0x00, 0x01]);
        assert_eq!(decoded, vec![1]);

        // And the inverse: encoding should put the high byte first.
        let encoded = codec.encode(&[1i16]);
        assert_eq!(encoded, vec![0x00, 0x01]);
    }

    #[test]
    fn test_decode_ignores_trailing_odd_byte() {
        let mut codec = L16Codec;

        // A truncated payload must not panic: the incomplete last sample is dropped.
        let decoded = codec.decode(&[0x00, 0x01, 0xFF]);
        assert_eq!(decoded, vec![1]);
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = L16Codec;
        assert_eq!(codec.samples_per_frame(), SAMPLES_PER_FRAME);
        assert_eq!(codec.samples_per_frame(), 320);
    }
}
