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
//! This implementation resamples to/from 48kHz for the internal PCM format,
//! using a 3x factor (16kHz * 3 = 48kHz), the same approach as G.722.

use super::Codec;

pub struct L16Codec;

const RESAMPLE_FACTOR: usize = 3;

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
        let chunks: std::slice::ChunksExact<'_, u8> = payload.chunks_exact(2);
        let mut samples = Vec::with_capacity(chunks.len() * RESAMPLE_FACTOR);
        for chunk in chunks {
            let sample = i16::from_be_bytes(
                chunk
                    .try_into()
                    .expect("chunks_exact(2) guarantees len == 2"),
            );
            for _ in 0..RESAMPLE_FACTOR {
                samples.push(sample);
            }
        }
        samples
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        let mut l16_16k: Vec<u8> = Vec::with_capacity(samples.len().div_ceil(RESAMPLE_FACTOR) * 2);
        for chunk in samples.chunks(RESAMPLE_FACTOR) {
            l16_16k.extend(chunk[chunk.len() / 2].to_be_bytes());
        }
        l16_16k
    }

    fn samples_per_frame(&self) -> usize {
        960
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_sizes() {
        let mut codec = L16Codec;

        // A 20ms frame at 48kHz = 960 samples.
        let frame = vec![0i16; 960];
        let encoded = codec.encode(&frame);
        // 960 / 3 = 320 samples at 16kHz; L16 is uncompressed, so each
        // sample is 2 bytes (unlike G722's ~1 byte per 2 samples).
        assert_eq!(encoded.len(), 640);

        let decoded = codec.decode(&encoded);
        // 640 bytes -> 320 samples at 16kHz -> 960 samples at 48kHz.
        assert_eq!(decoded.len(), 960);
    }

    #[test]
    fn test_decode_silence() {
        let mut codec = L16Codec;

        // L16 has no lossy compression step (unlike G722's ADPCM), so
        // silence round-trips to exact silence, not just "near" silence.
        let encoded = codec.encode(&vec![0i16; 960]);
        let decoded = codec.decode(&encoded);

        assert_eq!(decoded.len(), 960);
        assert!(decoded.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_encode_silence() {
        let mut codec = L16Codec;

        let silence = vec![0i16; 960];
        let encoded = codec.encode(&silence);

        assert_eq!(encoded.len(), 640);
        assert!(encoded.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_decode_encode_roundtrip() {
        let mut codec = L16Codec;

        // Build a 48kHz frame that's already "triplicated", i.e. exactly
        // what decode() produces: each 16kHz sample repeated 3x. Since L16
        // has no compression step, encoding then decoding such a frame is
        // lossless and must reproduce it exactly (the only lossy part of
        // this codec is the 48kHz<->16kHz resampling itself, which this
        // input doesn't exercise since it's already piecewise-constant in
        // groups of 3).
        let sixteen_khz: Vec<i16> = (0..320).map(|i| ((i * 97) % 30000) - 15000).collect();
        let original: Vec<i16> = sixteen_khz
            .iter()
            .flat_map(|&s| std::iter::repeat_n(s, RESAMPLE_FACTOR))
            .collect();
        assert_eq!(original.len(), 960);

        let encoded = codec.encode(&original);
        assert_eq!(encoded.len(), 640);

        let decoded = codec.decode(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_big_endian_byte_order() {
        let mut codec = L16Codec;

        // RFC 3551 requires L16 samples in network byte order (big-endian):
        // 0x0001 must decode to 1, not 256 (which would be little-endian).
        let decoded = codec.decode(&[0x00, 0x01]);
        assert_eq!(decoded[0], 1);

        // And the inverse: encoding should put the high byte first.
        let one_sample_16khz = vec![1i16; RESAMPLE_FACTOR];
        let encoded = codec.encode(&one_sample_16khz);
        assert_eq!(encoded, vec![0x00, 0x01]);
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = L16Codec;
        assert_eq!(codec.samples_per_frame(), 960);
    }
}
