//! RTP packet handling

use rtp_rs::RtpReader;

/// Represents an audio frame extracted from an RTP packet
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Raw audio payload data
    pub payload: Vec<u8>,
    /// RTP timestamp
    pub timestamp: u32,
    /// RTP sequence number
    pub sequence: u16,
    /// Synchronization source identifier
    pub ssrc: u32,
    /// Payload type (codec identifier)
    pub payload_type: u8,
}

impl AudioFrame {
    /// Create a new audio frame
    #[allow(dead_code)]
    pub fn new(
        payload: Vec<u8>,
        timestamp: u32,
        sequence: u16,
        ssrc: u32,
        payload_type: u8,
    ) -> Self {
        Self {
            payload,
            timestamp,
            sequence,
            ssrc,
            payload_type,
        }
    }
}

/// Parse an RTP packet and extract the audio frame
pub fn parse_rtp_packet(data: &[u8]) -> Option<AudioFrame> {
    let rtp = RtpReader::new(data).ok()?;

    Some(AudioFrame {
        payload: rtp.payload().to_vec(),
        timestamp: rtp.timestamp(),
        sequence: rtp.sequence_number().into(),
        ssrc: rtp.ssrc(),
        payload_type: rtp.payload_type(),
    })
}

/// Build an RTP packet from an audio frame
pub fn build_rtp_packet(frame: &AudioFrame) -> Vec<u8> {
    use rtp_rs::RtpPacketBuilder;

    RtpPacketBuilder::new()
        .payload_type(frame.payload_type)
        .ssrc(frame.ssrc)
        .sequence(frame.sequence.into())
        .timestamp(frame.timestamp)
        .payload(&frame.payload)
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

        let frame = parse_rtp_packet(&packet).unwrap();
        assert_eq!(frame.payload_type, 0);
        assert_eq!(frame.sequence, 1);
        assert_eq!(frame.timestamp, 160);
        assert_eq!(frame.ssrc, 0x12345678);
        assert_eq!(frame.payload.len(), 160);
    }

    #[test]
    fn test_build_rtp_packet() {
        let frame = AudioFrame {
            payload: vec![0xAA; 160],
            timestamp: 320,
            sequence: 2,
            ssrc: 0xDEADBEEF,
            payload_type: 0,
        };

        let packet = build_rtp_packet(&frame);
        assert!(packet.len() >= 12 + 160);

        // Verify we can parse it back
        let parsed = parse_rtp_packet(&packet).unwrap();
        assert_eq!(parsed.payload_type, 0);
        assert_eq!(parsed.sequence, 2);
        assert_eq!(parsed.timestamp, 320);
        assert_eq!(parsed.ssrc, 0xDEADBEEF);
        assert_eq!(parsed.payload.len(), 160);
    }
}
