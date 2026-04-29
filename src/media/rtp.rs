//! RTP packet handling

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
///
/// RTP Header format (RFC 3550):
/// ```text
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |V=2|P|X|  CC   |M|     PT      |       sequence number         |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                           timestamp                           |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |           synchronization source (SSRC) identifier            |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
pub fn parse_rtp_packet(data: &[u8]) -> Option<AudioFrame> {
    // Minimum RTP header size is 12 bytes
    if data.len() < 12 {
        return None;
    }

    // Check RTP version (must be 2)
    let version = (data[0] >> 6) & 0x03;
    if version != 2 {
        return None;
    }

    let padding = (data[0] >> 5) & 0x01;
    let extension = (data[0] >> 4) & 0x01;
    let csrc_count = data[0] & 0x0F;

    let payload_type = data[1] & 0x7F;
    let sequence = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    // Calculate header size
    let mut header_size = 12 + (csrc_count as usize * 4);

    // Handle extension header
    if extension == 1 {
        if data.len() < header_size + 4 {
            return None;
        }
        let ext_length =
            u16::from_be_bytes([data[header_size + 2], data[header_size + 3]]) as usize;
        header_size += 4 + (ext_length * 4);
    }

    if data.len() < header_size {
        return None;
    }

    // Calculate payload size (accounting for padding)
    let mut payload_end = data.len();
    if padding == 1 && !data.is_empty() {
        let padding_size = data[data.len() - 1] as usize;
        if padding_size <= data.len() - header_size {
            payload_end -= padding_size;
        }
    }

    let payload = data[header_size..payload_end].to_vec();

    Some(AudioFrame {
        payload,
        timestamp,
        sequence,
        ssrc,
        payload_type,
    })
}

/// Build an RTP packet from an audio frame
pub fn build_rtp_packet(frame: &AudioFrame) -> Vec<u8> {
    use rtp_rs::RtpPacketBuilder;

    match RtpPacketBuilder::new()
        .payload_type(frame.payload_type)
        .ssrc(frame.ssrc)
        .sequence(frame.sequence.into())
        .timestamp(frame.timestamp)
        .payload(&frame.payload)
        .build()
    {
        Ok(packet) => packet,
        Err(_) => {
            // Fallback: build packet manually if rtp-rs fails
            let mut packet = Vec::with_capacity(12 + frame.payload.len());

            // Version (2), no padding, no extension, no CSRC
            packet.push(0x80);
            // Marker bit (0), payload type
            packet.push(frame.payload_type & 0x7F);
            // Sequence number
            packet.extend_from_slice(&frame.sequence.to_be_bytes());
            // Timestamp
            packet.extend_from_slice(&frame.timestamp.to_be_bytes());
            // SSRC
            packet.extend_from_slice(&frame.ssrc.to_be_bytes());
            // Payload
            packet.extend_from_slice(&frame.payload);

            packet
        }
    }
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
