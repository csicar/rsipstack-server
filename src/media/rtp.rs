//! RTP packet handling

use rtp_rs::RtpReader;

/// Represents an audio frame with decoded PCM samples
///
/// Contains decoded PCM samples at 48kHz, 960 samples per frame (20ms).
/// All RTP details (timestamps, sequence numbers, SSRC) are handled
/// internally by the library.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Decoded PCM samples at 48kHz, 960 samples per frame (20ms)
    pub samples: Vec<i16>,
}

impl AudioFrame {
    /// Create a new audio frame with the given samples
    pub fn new(samples: Vec<i16>) -> Self {
        Self { samples }
    }
}

/// Internal state for RTP packet generation
#[derive(Clone)]
pub(crate) struct RtpSendState {
    /// Synchronization source (random, unique per session)
    pub ssrc: u32,
    /// Current sequence number (auto-increments)
    pub sequence: u16,
    /// Current timestamp (increments by 960 per frame for 48kHz)
    pub timestamp: u32,
    /// Payload type (determined by negotiated codec)
    pub payload_type: u8,
}

impl RtpSendState {
    pub fn new(payload_type: u8) -> Self {
        Self {
            ssrc: rand::random(),
            sequence: rand::random(),
            timestamp: rand::random(),
            payload_type,
        }
    }

    /// Return current state and advance for next packet
    pub fn next(&mut self) -> RtpSendState {
        let current = self.clone();
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(960); // 20ms at 48kHz
        current
    }
}

/// Raw RTP packet data (before decoding)
#[allow(dead_code)]
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
pub(crate) fn build_rtp_packet(payload: &[u8], state: &RtpSendState) -> Vec<u8> {
    use rtp_rs::RtpPacketBuilder;

    RtpPacketBuilder::new()
        .payload_type(state.payload_type)
        .ssrc(state.ssrc)
        .sequence(state.sequence.into())
        .timestamp(state.timestamp)
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
        let state = RtpSendState {
            ssrc: 0xDEADBEEF,
            sequence: 2,
            timestamp: 320,
            payload_type: 0,
        };
        let packet = build_rtp_packet(&payload, &state);

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
        let frame = AudioFrame::new(vec![100, 200, 300]);

        assert_eq!(frame.samples, vec![100, 200, 300]);
    }
}
