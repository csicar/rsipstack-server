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

use strum::EnumIter;

use l16::L16Codec;
use pcma::PcmaCodec;
use pcmu::PcmuCodec;

#[cfg(feature = "opus")]
use self::opus::OpusCodec;

#[cfg(feature = "g722")]
use self::g722::G722Codec;

#[derive(Debug)]
pub struct CodecInitError {
    kind: CodecKind,
    reason: String,
}

impl std::fmt::Display for CodecInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to initialize {} codec: {}",
            self.kind.name(),
            self.reason
        )
    }
}

impl std::error::Error for CodecInitError {}

/// The codecs this server supports, parsed from an SDP codec name.
///
/// A codec only exists here if its feature is enabled, so holding a `CodecKind`
/// means the codec is supported. Parse the name once with [`CodecKind::from_name`]
/// and pass the kind around instead of the name.
#[derive(EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind {
    /// G.711 μ-law
    Pcmu,
    /// G.711 A-law
    Pcma,
    #[cfg(feature = "g722")]
    G722,
    #[cfg(feature = "opus")]
    Opus,
    /// 16kHz, mono
    L16,
}

impl CodecKind {
    /// Parses an SDP codec name, case-insensitively.
    ///
    /// `None` if the codec is unsupported or its feature is disabled. The name
    /// comes from `parse_sdp_offer`, which names the static payload types
    /// (0, 8, 9) itself and takes the rtpmap encoding name for dynamic ones.
    pub fn from_name(codec_name: &str) -> Option<Self> {
        if codec_name.eq_ignore_ascii_case("pcmu") {
            return Some(Self::Pcmu);
        }
        if codec_name.eq_ignore_ascii_case("pcma") {
            return Some(Self::Pcma);
        }
        #[cfg(feature = "g722")]
        if codec_name.eq_ignore_ascii_case("g722") {
            return Some(Self::G722);
        }
        #[cfg(feature = "opus")]
        if codec_name.eq_ignore_ascii_case("opus") {
            return Some(Self::Opus);
        }
        if codec_name.eq_ignore_ascii_case("l16") {
            return Some(Self::L16);
        }
        None
    }

    /// Canonical SDP / metric-label name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pcmu => "PCMU",
            Self::Pcma => "PCMA",
            #[cfg(feature = "g722")]
            Self::G722 => "G722",
            #[cfg(feature = "opus")]
            Self::Opus => "opus",
            Self::L16 => "L16",
        }
    }

    /// Create a codec instance of this kind.
    ///
    /// Only Opus can fail, when the underlying encoder or decoder cannot be
    /// initialized.
    pub fn create(self) -> Result<Box<dyn Codec>, CodecInitError> {
        Ok(match self {
            Self::Pcmu => Box::new(PcmuCodec::new()),
            Self::Pcma => Box::new(PcmaCodec::new()),
            #[cfg(feature = "g722")]
            Self::G722 => Box::new(G722Codec::new()),
            #[cfg(feature = "opus")]
            Self::Opus => Box::new(OpusCodec::new().map_err(|e| CodecInitError {
                kind: self,
                reason: e.to_string(),
            })?),
            Self::L16 => Box::new(L16Codec::new()),
        })
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn from_name_is_case_insensitive() {
        assert_eq!(CodecKind::from_name("pcmu"), Some(CodecKind::Pcmu));
        assert_eq!(CodecKind::from_name("Pcma"), Some(CodecKind::Pcma));
        assert_eq!(CodecKind::from_name("l16"), Some(CodecKind::L16));
    }

    #[test]
    fn name_round_trips_for_every_kind() {
        for kind in CodecKind::iter() {
            assert_eq!(CodecKind::from_name(kind.name()), Some(kind));
        }
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn from_name_rejects_opus_when_feature_disabled() {
        assert_eq!(CodecKind::from_name("opus"), None);
    }

    #[cfg(not(feature = "g722"))]
    #[test]
    fn from_name_rejects_g722_when_feature_disabled() {
        assert_eq!(CodecKind::from_name("G722"), None);
    }

    #[test]
    fn from_name_rejects_unknown() {
        assert_eq!(CodecKind::from_name("PT99"), None);
        assert_eq!(CodecKind::from_name(""), None);
    }

    #[test]
    fn every_kind_can_be_created() {
        for kind in CodecKind::iter() {
            assert!(kind.create().is_ok(), "{} failed to create", kind.name());
        }
    }

    #[test]
    fn created_codecs_have_the_internal_frame_size() {
        for kind in CodecKind::iter() {
            let codec = kind.create().unwrap();
            assert_eq!(codec.samples_per_frame(), 960, "{}", kind.name());
        }
    }

    #[test]
    fn rtp_timestamp_increment_per_kind() {
        let increment = |kind: CodecKind| kind.create().unwrap().rtp_timestamp_increment();

        // Per RFC 3551, PCMU/PCMA use an 8kHz clock (160 per 20ms frame).
        assert_eq!(increment(CodecKind::Pcmu), TimestampIncrement::new(160));
        assert_eq!(increment(CodecKind::Pcma), TimestampIncrement::new(160));
        // L16 at 16kHz rides a dynamic payload type, same as Opus, but has a
        // different clock rate (320 per 20ms, not 960).
        assert_eq!(increment(CodecKind::L16), TimestampIncrement::new(320));
    }

    #[cfg(feature = "g722")]
    #[test]
    fn rtp_timestamp_increment_g722() {
        // RFC 3551 §4.5.2: G.722 uses an 8kHz RTP clock despite sampling at 16kHz.
        let codec = CodecKind::G722.create().unwrap();
        assert_eq!(
            codec.rtp_timestamp_increment(),
            TimestampIncrement::new(160)
        );
    }

    #[cfg(feature = "opus")]
    #[test]
    fn rtp_timestamp_increment_opus() {
        // RFC 7587: Opus always uses a 48kHz RTP clock.
        let codec = CodecKind::Opus.create().unwrap();
        assert_eq!(
            codec.rtp_timestamp_increment(),
            TimestampIncrement::new(960)
        );
    }

    #[test]
    fn codec_init_error_names_the_codec_and_the_reason() {
        // This is the message `call_handler` logs when it rejects a call with
        // `RejectReason::CodecInitFailed`. A real failure can't be provoked on
        // demand (Opus init only fails when libopus cannot allocate), so the error
        // is built by hand.
        let error = CodecInitError {
            kind: CodecKind::L16,
            reason: "out of memory".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to initialize L16 codec: out of memory"
        );
    }
}
