//! Audio codec implementations
//!
//! This module provides codec encoding/decoding for RTP audio.
//! All codecs normalize audio to 48kHz PCM i16 samples.

mod l16;
mod pcma;
mod pcmu;

#[cfg(feature = "opus")]
mod opus;

#[cfg(feature = "g722")]
mod g722;

pub use l16::L16Codec;
pub use pcma::PcmaCodec;
pub use pcmu::PcmuCodec;

#[cfg(feature = "opus")]
pub use self::opus::OpusCodec;

#[cfg(feature = "g722")]
pub use self::g722::G722Codec;

/// RTP timestamp advance per 20ms frame, in ticks of a codec's RTP clock.
///
/// A newtype so it can't be mixed up with other `u32` values in an RTP header
/// (timestamp, SSRC), or with a sample count or a clock rate in Hz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimestampIncrement(u32);

impl TimestampIncrement {
    pub const fn new(ticks: u32) -> Self {
        Self(ticks)
    }

    pub fn ticks(&self) -> u32 {
        self.0
    }
}

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

    /// RTP timestamp advance per 20ms frame, i.e. 20ms at the codec's RTP clock rate.
    ///
    /// This is the codec's *RTP clock* rate, which is not always its sampling rate
    /// (see G.722). Defined on the codec itself, rather than looked up by name
    /// elsewhere, so it can't drift from the codec implementation, and so codecs
    /// sharing a dynamic payload type (Opus, L16) are told apart by construction.
    fn rtp_timestamp_increment(&self) -> TimestampIncrement;
}

/// Create a codec instance from its SDP codec name (case-insensitive)
///
/// The name comes from `parse_sdp_offer`, which names the static payload types
/// (0, 8, 9) itself and takes the rtpmap encoding name for dynamic ones, so no
/// payload type is needed here.
///
/// Returns `None` for unsupported names.
///
/// # Supported Names
/// - `PCMU` (G.711 μ-law)
/// - `PCMA` (G.711 A-law)
/// - `G722` (when "g722" feature is enabled)
/// - `opus` (when "opus" feature is enabled)
/// - `L16` (16kHz, mono, always enabled)
pub fn create_codec(codec_name: &str) -> Option<Box<dyn Codec>> {
    if codec_name.eq_ignore_ascii_case("pcmu") {
        return Some(Box::new(PcmuCodec::new()));
    }
    if codec_name.eq_ignore_ascii_case("pcma") {
        return Some(Box::new(PcmaCodec::new()));
    }
    #[cfg(feature = "g722")]
    if codec_name.eq_ignore_ascii_case("g722") {
        return Some(Box::new(G722Codec::new()));
    }
    #[cfg(feature = "opus")]
    if codec_name.eq_ignore_ascii_case("opus") {
        return OpusCodec::new().ok().map(|c| Box::new(c) as Box<dyn Codec>);
    }
    if codec_name.eq_ignore_ascii_case("l16") {
        return Some(Box::new(L16Codec::new()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_pcmu_codec() {
        let codec = create_codec("PCMU");
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }

    #[test]
    fn test_create_pcma_codec() {
        let codec = create_codec("PCMA");
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }

    #[test]
    fn test_create_unknown_codec() {
        // `parse_sdp_offer` names payload types without an rtpmap `PT<n>`.
        assert!(create_codec("PT99").is_none());
        assert!(create_codec("").is_none());
    }

    #[test]
    fn test_create_codec_is_case_insensitive() {
        assert!(create_codec("pcmu").is_some());
        assert!(create_codec("Pcma").is_some());
        assert!(create_codec("l16").is_some());
    }

    #[cfg(feature = "g722")]
    #[test]
    fn test_create_g722_codec() {
        let codec = create_codec("G722");
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }

    #[test]
    fn test_create_l16_codec() {
        let codec = create_codec("L16");
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().samples_per_frame(), 960);
    }

    #[test]
    fn test_rtp_timestamp_increment_per_codec() {
        // Per RFC 3551, PCMU/PCMA use an 8kHz clock (160 per 20ms frame).
        assert_eq!(
            create_codec("PCMU").unwrap().rtp_timestamp_increment(),
            TimestampIncrement::new(160)
        );
        assert_eq!(
            create_codec("PCMA").unwrap().rtp_timestamp_increment(),
            TimestampIncrement::new(160)
        );
        // L16 at 16kHz rides a dynamic payload type, same as Opus, but has a
        // different clock rate (320 per 20ms, not 960).
        assert_eq!(
            create_codec("L16").unwrap().rtp_timestamp_increment(),
            TimestampIncrement::new(320)
        );
    }

    #[cfg(feature = "g722")]
    #[test]
    fn test_rtp_timestamp_increment_g722() {
        // RFC 3551 §4.5.2: G.722 uses an 8kHz RTP clock despite sampling at 16kHz.
        assert_eq!(
            create_codec("G722").unwrap().rtp_timestamp_increment(),
            TimestampIncrement::new(160)
        );
    }

    #[cfg(feature = "opus")]
    #[test]
    fn test_rtp_timestamp_increment_opus() {
        assert_eq!(
            create_codec("opus").unwrap().rtp_timestamp_increment(),
            TimestampIncrement::new(960)
        );
    }
}
