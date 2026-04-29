//! SDP parsing and generation

use std::net::IpAddr;

/// Parsed SDP offer information
#[derive(Debug, Clone)]
pub struct SdpOffer {
    pub peer_addr: IpAddr,
    pub peer_port: u16,
    pub payload_type: u8,
    #[allow(dead_code)]
    pub codec_name: String,
}

/// Parse an SDP offer and extract relevant information
pub fn parse_sdp_offer(sdp_body: &str) -> Result<SdpOffer, SdpParseError> {
    let sdp = sdp_rs::SessionDescription::try_from(sdp_body)
        .map_err(|e| SdpParseError::ParseError(format!("{:?}", e)))?;

    // Get connection address from the SDP connection line
    // connection_address.base is already an IpAddr
    let peer_addr = sdp
        .connection
        .as_ref()
        .map(|c| c.connection_address.base)
        .ok_or(SdpParseError::MissingConnectionAddress)?;

    // Find audio media description
    let audio_media = sdp
        .media_descriptions
        .iter()
        .find(|m| m.media.media == sdp_rs::lines::media::MediaType::Audio)
        .ok_or(SdpParseError::NoAudioMedia)?;

    let peer_port = audio_media.media.port;

    // Parse the first format (payload type)
    let payload_type: u8 = audio_media
        .media
        .fmt
        .split_whitespace()
        .next()
        .and_then(|pt| pt.parse().ok())
        .unwrap_or(0); // Default to PCMU

    // Determine codec name from payload type
    // Static payload types (0-95) have fixed meanings, dynamic types (96-127) need rtpmap lookup
    let codec_name = match payload_type {
        0 => "PCMU".to_string(),
        8 => "PCMA".to_string(),
        _ => {
            // Try to find rtpmap attribute for this payload type
            let mut found_codec = None;
            for attr in &audio_media.attributes {
                if let sdp_rs::lines::Attribute::Rtpmap(rtpmap) = attr {
                    if rtpmap.payload_type == payload_type as u32 {
                        found_codec = Some(rtpmap.encoding_name.clone());
                        break;
                    }
                }
            }
            found_codec.unwrap_or_else(|| format!("PT{}", payload_type))
        }
    };

    Ok(SdpOffer {
        peer_addr,
        peer_port,
        payload_type,
        codec_name,
    })
}

/// Generate an SDP answer
pub fn generate_sdp_answer(
    local_ip: IpAddr,
    rtp_port: u16,
    session_id: u64,
    _payload_type: u8,
) -> String {
    // Generate SDP with Opus, PCMU (0), and PCMA (8) support
    format!(
        "v=0\r\n\
         o=- {} 1 IN IP4 {}\r\n\
         s=rsipstack-server\r\n\
         c=IN IP4 {}\r\n\
         t=0 0\r\n\
         m=audio {} RTP/AVP 111 0 8\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         a=fmtp:111 minptime=10;useinbandfec=1\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=ptime:20\r\n\
         a=sendrecv\r\n",
        session_id, local_ip, local_ip, rtp_port
    )
}

/// SDP parsing error
#[derive(Debug)]
pub enum SdpParseError {
    ParseError(String),
    MissingConnectionAddress,
    #[allow(dead_code)]
    InvalidAddress(String),
    NoAudioMedia,
}

impl std::fmt::Display for SdpParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SdpParseError::ParseError(e) => write!(f, "SDP parse error: {}", e),
            SdpParseError::MissingConnectionAddress => write!(f, "Missing connection address in SDP"),
            SdpParseError::InvalidAddress(addr) => write!(f, "Invalid address in SDP: {}", addr),
            SdpParseError::NoAudioMedia => write!(f, "No audio media in SDP"),
        }
    }
}

impl std::error::Error for SdpParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sdp_offer() {
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 0 8\r\n\
                   a=rtpmap:0 PCMU/8000\r\n\
                   a=rtpmap:8 PCMA/8000\r\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.peer_addr.to_string(), "192.168.1.100");
        assert_eq!(offer.peer_port, 5000);
        assert_eq!(offer.payload_type, 0);
        assert_eq!(offer.codec_name, "PCMU");
    }

    #[test]
    fn test_generate_sdp_answer() {
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        let sdp = generate_sdp_answer(ip, 6000, 123456, 0);

        assert!(sdp.contains("c=IN IP4 192.168.1.1"));
        assert!(sdp.contains("m=audio 6000 RTP/AVP 111 0 8"));
        assert!(sdp.contains("a=rtpmap:111 opus/48000/2"));
        assert!(sdp.contains("a=fmtp:111 minptime=10;useinbandfec=1"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        assert!(sdp.contains("a=rtpmap:8 PCMA/8000"));
    }

    #[test]
    fn test_parse_sdp_offer_opus() {
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 111 0 8\r\n\
                   a=rtpmap:111 opus/48000/2\r\n\
                   a=fmtp:111 minptime=10;useinbandfec=1\r\n\
                   a=rtpmap:0 PCMU/8000\r\n\
                   a=rtpmap:8 PCMA/8000\r\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.peer_addr.to_string(), "192.168.1.100");
        assert_eq!(offer.peer_port, 5000);
        assert_eq!(offer.payload_type, 111);
        assert_eq!(offer.codec_name, "opus");
    }

    #[test]
    fn test_parse_sdp_offer_opus_only() {
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 10.0.0.50\r\n\
                   s=Orvibo Call\r\n\
                   c=IN IP4 10.0.0.50\r\n\
                   t=0 0\r\n\
                   m=audio 4000 RTP/AVP 111\r\n\
                   a=rtpmap:111 opus/48000/2\r\n\
                   a=fmtp:111 minptime=10;useinbandfec=1\r\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.peer_addr.to_string(), "10.0.0.50");
        assert_eq!(offer.peer_port, 4000);
        assert_eq!(offer.payload_type, 111);
        assert_eq!(offer.codec_name, "opus");
    }

    #[test]
    fn test_parse_sdp_offer_opus_dynamic_pt() {
        // Test that Opus works with any dynamic payload type (96-127), not just 111
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 96 0\r\n\
                   a=rtpmap:96 opus/48000/2\r\n\
                   a=fmtp:96 minptime=10;useinbandfec=1\r\n\
                   a=rtpmap:0 PCMU/8000\r\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.peer_port, 5000);
        assert_eq!(offer.payload_type, 96);
        assert_eq!(offer.codec_name, "opus");
    }
}
