//! RTP packet handling

use rtp_rs::RtpReader;

/// Represents an audio frame with decoded PCM samples
///
/// All audio is normalized to 48kHz PCM i16 samples.
/// The `payload_type` field is preserved for encoding back to RTP.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Decoded PCM samples at 48kHz
    pub samples: Vec<i16>,
    /// RTP timestamp
    pub timestamp: u32,
    /// RTP sequence number
    pub sequence: u16,
    /// Synchronization source identifier
    pub ssrc: u32,
    /// Payload type (codec identifier, used when encoding back to RTP)
    pub payload_type: u8,
}

impl AudioFrame {
    /// Create a new audio frame
    #[allow(dead_code)]
    pub fn new(
        samples: Vec<i16>,
        timestamp: u32,
        sequence: u16,
        ssrc: u32,
        payload_type: u8,
    ) -> Self {
        Self {
            samples,
            timestamp,
            sequence,
            ssrc,
            payload_type,
        }
    }
}

/// Raw RTP packet data (before decoding)
pub(crate) struct RawRtpPacket {
    pub payload: Vec<u8>,
    pub timestamp: u32,
    pub sequence: u16,
    pub ssrc: u32,
    pub payload_type: u8,
}

/// Parse an RTP packet and extract raw data (before codec decoding)
pub(crate) fn parse_rtp_packet(data: &[u8]) -> Option<RawRtpPacket> {
    let rtp = RtpReader::new(data).ok()?;

    Some(RawRtpPacket {
        payload: rtp.payload().to_vec(),
        timestamp: rtp.timestamp(),
        sequence: rtp.sequence_number().into(),
        ssrc: rtp.ssrc(),
        payload_type: rtp.payload_type(),
    })
}

/// Build an RTP packet from raw data (after codec encoding)
pub(crate) fn build_rtp_packet(
    payload: &[u8],
    timestamp: u32,
    sequence: u16,
    ssrc: u32,
    payload_type: u8,
) -> Vec<u8> {
    use rtp_rs::RtpPacketBuilder;

    RtpPacketBuilder::new()
        .payload_type(payload_type)
        .ssrc(ssrc)
        .sequence(sequence.into())
        .timestamp(timestamp)
        .payload(payload)
        .build()
        .expect("RTP packet building should not fail with valid inputs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rtp_packet() {
        // Build a test RTP packet
        let mut packet = vec![
            0x80, // Version 2, no padding, no extension, no CSRC
            0x00, // Marker=0, PT=0 (PCMU)
            0x00, 0x01, // Sequence = 1
            0x00, 0x00, 0x00, 0xA0, // Timestamp = 160
            0x12, 0x34, 0x56, 0x78, // SSRC = 0x12345678
        ];
        // Add payload
        packet.extend_from_slice(&[0x55; 160]);

        let raw = parse_rtp_packet(&packet).unwrap();
        assert_eq!(raw.payload_type, 0);
        assert_eq!(raw.sequence, 1);
        assert_eq!(raw.timestamp, 160);
        assert_eq!(raw.ssrc, 0x12345678);
        assert_eq!(raw.payload.len(), 160);
    }

    #[test]
    fn test_build_rtp_packet() {
        let payload = vec![0xAA; 160];
        let packet = build_rtp_packet(&payload, 320, 2, 0xDEADBEEF, 0);

        assert!(packet.len() >= 12 + 160);

        // Verify we can parse it back
        let parsed = parse_rtp_packet(&packet).unwrap();
        assert_eq!(parsed.payload_type, 0);
        assert_eq!(parsed.sequence, 2);
        assert_eq!(parsed.timestamp, 320);
        assert_eq!(parsed.ssrc, 0xDEADBEEF);
        assert_eq!(parsed.payload.len(), 160);
    }

    #[test]
    fn test_audio_frame_new() {
        let frame = AudioFrame::new(
            vec![100, 200, 300],
            160,
            1,
            12345,
            0,
        );

        assert_eq!(frame.samples, vec![100, 200, 300]);
        assert_eq!(frame.timestamp, 160);
        assert_eq!(frame.sequence, 1);
        assert_eq!(frame.ssrc, 12345);
        assert_eq!(frame.payload_type, 0);
    }
}
