//! G.722 wideband codec implementation
//!
//! G.722 is a sub-band ADPCM (SB-ADPCM) wideband codec. Although it carries
//! 16kHz audio, by historical convention its RTP payload type is the static
//! type 9 and its `rtpmap` clock rate is advertised as 8000, not 16000.
//!
//! At 64 kbit/s the codec emits one octet for every two 16kHz input samples,
//! so a 20ms frame is 320 samples in / 160 octets out.
//!
//! Native sample rate: 16kHz.
//! No resampling is needed: the internal PCM format is also 16kHz mono.

use super::{Codec, TimestampIncrement};
use audio_codec::g722::{G722Decoder, G722Encoder};
use audio_codec::{Decoder as _, Encoder as _};

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
        // The decoder already produces 16kHz PCM, the internal format.
        self.decoder.decode(payload)
    }

    fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        // The G.722 encoder needs an even number of samples. A 20ms frame always
        // has 320, so only a malformed frame takes the copying path.
        if samples.len().is_multiple_of(2) {
            return self.encoder.encode(samples);
        }

        let mut padded = samples.to_vec();
        padded.push(0);
        self.encoder.encode(&padded)
    }

    fn samples_per_frame(&self) -> usize {
        crate::SAMPLES_PER_FRAME
    }

    fn rtp_timestamp_increment(&self) -> TimestampIncrement {
        // RFC 3551 §4.5.2: G.722 uses an 8kHz RTP clock by convention, even though
        // it samples audio at 16kHz, so 20ms = 160 ticks
        TimestampIncrement::new(160)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SAMPLES_PER_FRAME;

    #[test]
    fn test_frame_sizes() {
        let mut codec = G722Codec::new();

        // A 20ms frame at 16kHz = 320 samples.
        let frame = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = codec.encode(&frame);
        // 64 kbit/s emits 1 octet per 2 samples.
        assert_eq!(encoded.len(), 160);

        let decoded = codec.decode(&encoded);
        // 160 octets -> 320 samples at 16kHz.
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
    }

    #[test]
    fn test_decode_silence() {
        let mut codec = G722Codec::new();

        // Silence encoded from a zero frame should decode back to near-silence.
        let encoded = codec.encode(&vec![0i16; SAMPLES_PER_FRAME]);
        let decoded = codec.decode(&encoded);

        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
        for sample in decoded {
            assert!(sample.abs() < 100, "Expected near-silence, got {}", sample);
        }
    }

    #[test]
    fn test_decode_encode_roundtrip() {
        let mut enc = G722Codec::new();

        // A low-frequency sine wave at 16kHz survives the codec round-trip well.
        let original: Vec<i16> = (0..SAMPLES_PER_FRAME)
            .map(|i| {
                let t = i as f32 / 16000.0;
                (8000.0 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()) as i16
            })
            .collect();

        let encoded = enc.encode(&original);
        assert_eq!(encoded.len(), 160);

        let decoded = enc.decode(&encoded);
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);

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
    fn test_encode_pads_odd_sample_count() {
        let mut codec = G722Codec::new();

        // The encoder needs an even number of samples; a malformed odd-sized
        // frame is padded with one zero sample instead of failing.
        let encoded = codec.encode(&vec![0i16; SAMPLES_PER_FRAME - 1]);
        assert_eq!(encoded.len(), 160);
    }

    #[test]
    fn test_samples_per_frame() {
        let codec = G722Codec::new();
        assert_eq!(codec.samples_per_frame(), SAMPLES_PER_FRAME);
        assert_eq!(codec.samples_per_frame(), 320);
    }
}
