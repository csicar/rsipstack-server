//! G.722 wideband codec implementation
//!
//! G.722 is a sub-band ADPCM (SB-ADPCM) wideband codec. Although it carries
//! 16kHz audio, by historical convention its RTP payload type is the static
//! type 9 and its `rtpmap` clock rate is advertised as 8000, not 16000.
//!
//! At 64 kbit/s the codec emits one octet for every two 16kHz input samples,
//! so a 20ms frame is 320 samples in / 160 octets out.
//!
//! Native sample rate: 16kHz
//! This implementation resamples to/from 48kHz for the internal PCM format,
//! using a 3x factor (16kHz * 3 = 48kHz).

use super::Codec;
use audio_codec::g722::{G722Decoder, G722Encoder};
use audio_codec::{Decoder as _, Encoder as _};

/// Upsample/downsample factor between G.722's 16kHz and the internal 48kHz.
const RESAMPLE_FACTOR: usize = 3;

/// G.722 codec (64 kbit/s, wideband)
pub struct G722Codec {
    encoder: G722Encoder,
    decoder: G722Decoder,
}

impl G722Codec {
    pub fn new() -> Self {
        // audio-codec's G.722 runs at the standard 64 kbit/s (highest quality)
        // and expects/produces 16kHz PCM.
        Self {
            encoder: G722Encoder::new(),
            decoder: G722Decoder::new(),
        }
    }
}

impl Default for G722Codec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for G722Codec {
    fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        // Decode to 16kHz PCM, then upsample 3x to 48kHz by sample duplication.
        let pcm16k = self.decoder.decode(payload);
        let mut samples = Vec::with_capacity(pcm16k.len() * RESAMPLE_FACTOR);
        for sample in pcm16k {
            for _ in 0..RESAMPLE_FACTOR {
                samples.push(sample);
            }
        }
        samples
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        // Downsample from 48kHz to 16kHz (take the middle of every 3 samples),
        // then G.722-encode. The encoder requires an even number of samples.
        let mut pcm16k = Vec::with_capacity(samples.len().div_ceil(RESAMPLE_FACTOR));
        for chunk in samples.chunks(RESAMPLE_FACTOR) {
            pcm16k.push(chunk[chunk.len() / 2]);
        }
        if pcm16k.len() % 2 != 0 {
            pcm16k.push(0);
        }
        self.encoder.encode(&pcm16k)
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
    fn test_frame_sizes() {
        let mut codec = G722Codec::new();

        // A 20ms frame at 48kHz = 960 samples.
        let frame = vec![0i16; 960];
        let encoded = codec.encode(&frame);
        // 960 / 3 = 320 samples at 16kHz; 64 kbit/s emits 1 octet per 2 samples.
        assert_eq!(encoded.len(), 160);

        let decoded = codec.decode(&encoded);
        // 160 octets -> 320 samples at 16kHz -> 960 samples at 48kHz.
        assert_eq!(decoded.len(), 960);
    }

    #[test]
    fn test_decode_silence() {
        let mut codec = G722Codec::new();

        // Silence encoded from a zero frame should decode back to near-silence.
        let encoded = codec.encode(&vec![0i16; 960]);
        let decoded = codec.decode(&encoded);

        assert_eq!(decoded.len(), 960);
        for sample in decoded {
            assert!(sample.abs() < 100, "Expected near-silence, got {}", sample);
        }
    }

    #[test]
    fn test_decode_encode_roundtrip() {
        let mut enc = G722Codec::new();

        // A low-frequency sine wave at 48kHz survives the 16kHz round-trip well.
        let original: Vec<i16> = (0..960)
            .map(|i| {
                let t = i as f32 / 48000.0;
                (8000.0 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()) as i16
            })
            .collect();

        let encoded = enc.encode(&original);
        assert_eq!(encoded.len(), 160);

        let decoded = enc.decode(&encoded);
        assert_eq!(decoded.len(), 960);

        // G.722 is lossy and has codec delay, so compare average energy rather
        // than sample-for-sample: the decoded frame should carry real signal,
        // not silence or garbage.
        let energy: i64 = decoded.iter().map(|&s| (s as i64) * (s as i64)).sum();
        let avg_energy = energy / decoded.len() as i64;
        assert!(
            avg_energy > 100_000,
            "Decoded signal energy too low: {}",
            avg_energy
        );
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = G722Codec::new();
        assert_eq!(codec.samples_per_frame(), 960);
    }
}
