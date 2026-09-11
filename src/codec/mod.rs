//! Audio codec implementations
//!
//! This module provides codec encoding/decoding for RTP audio.
//! All codecs normalize audio to 48kHz PCM i16 samples.

mod pcma;
mod pcmu;

#[cfg(feature = "opus")]
mod opus;

#[cfg(feature = "g722")]
mod g722;

pub use pcma::PcmaCodec;
pub use pcmu::PcmuCodec;

#[cfg(feature = "opus")]
pub use self::opus::OpusCodec;

#[cfg(feature = "g722")]
pub use self::g722::G722Codec;

/// Trait for audio codecs that encode/decode RTP payloads
///
/// All codecs work with PCM samples at 48kHz sample rate.
/// The `decode` method converts codec-specific bytes to PCM samples,
/// and `encode` converts PCM samples back to codec bytes.
pub trait Codec: Send {
    /// Decode codec payload bytes to PCM i16 samples at 48kHz
    fn decode(&mut self, payload: &[u8]) -> Vec<i16>;

    /// Encode PCM i16 samples at 48kHz to codec payload bytes
    fn encode(&mut self, samples: &[i16]) -> Vec<u8>;

    /// Number of PCM samples per frame at 48kHz (typically 960 for 20ms)
    fn samples_per_frame(&self) -> usize;
}

/// Create a codec instance based on RTP payload type
///
/// Returns `None` for unsupported payload types.
///
/// # Supported Payload Types
/// - 0: PCMU (G.711 μ-law)
/// - 8: PCMA (G.711 A-law)
/// - 9: G.722 (when "g722" feature is enabled)
/// - Dynamic types for Opus (when "opus" feature is enabled)
pub fn create_codec(payload_type: u8, codec_name: Option<&str>) -> Option<Box<dyn Codec>> {
    match payload_type {
        0 => Some(Box::new(PcmuCodec::new())),
        8 => Some(Box::new(PcmaCodec::new())),
        #[cfg(feature = "g722")]
        9 => Some(Box::new(G722Codec::new())),
        _ => {
            // For dynamic payload types, check codec name
            #[cfg(feature = "opus")]
            if let Some(name) = codec_name {
                if name.eq_ignore_ascii_case("opus") {
                    return OpusCodec::new().ok().map(|c| Box::new(c) as Box<dyn Codec>);
                }
            }
            let _ = codec_name; // Suppress unused warning when opus feature is disabled
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_pcmu_codec() {
        let codec = create_codec(0, None);
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }

    #[test]
    fn test_create_pcma_codec() {
        let codec = create_codec(8, None);
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }

    #[test]
    fn test_create_unknown_codec() {
        let codec = create_codec(99, None);
        assert!(codec.is_none());
    }

    #[cfg(feature = "g722")]
    #[test]
    fn test_create_g722_codec() {
        // G.722 uses the static payload type 9.
        let codec = create_codec(9, Some("G722"));
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }
}
