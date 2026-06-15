//! SDP parsing and generation

use std::net::IpAddr;

/// Information about a single codec from SDP
#[derive(Debug, Clone)]
pub struct CodecInfo {
    pub payload_type: u8,
    pub codec_name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PeerPort(pub u16);

#[derive(Debug, Clone, Copy)]
pub struct PeerIpAddr(pub IpAddr);

impl std::fmt::Display for PeerIpAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Parsed SDP offer information
#[derive(Debug, Clone)]
pub struct SdpOffer {
    pub peer_addr: PeerIpAddr,
    pub peer_port: PeerPort,
    /// All codecs offered, in preference order
    pub codecs: Vec<CodecInfo>,
    /// The selected codec (first mutually supported codec)
    pub payload_type: u8,
    pub codec_name: String,
}

/// Codecs we support, in preference order
const SUPPORTED_CODECS: &[&str] = &["opus", "PCMU", "PCMA"];

/// Check if we support a codec by name (case-insensitive)
fn is_supported_codec(name: &str) -> bool {
    SUPPORTED_CODECS
        .iter()
        .any(|&supported| supported.eq_ignore_ascii_case(name))
}

/// Parse an SDP offer and extract relevant information
pub fn parse_sdp_offer(sdp_body: &str) -> Result<SdpOffer, SdpParseError> {
    let sdp = sdp_rs::SessionDescription::try_from(sdp_body)
        .map_err(|e| SdpParseError::ParseError(format!("{:?}", e)))?;

    // Find audio media description
    let audio_media = sdp
        .media_descriptions
        .iter()
        .find(|m| m.media.media == sdp_rs::lines::media::MediaType::Audio)
        .ok_or(SdpParseError::NoAudioMedia)?;

    // Get connection address - prefer media-level, fall back to session-level
    let peer_addr = PeerIpAddr(
        audio_media
            .connections
            .first()
            .or(sdp.connection.as_ref())
            .map(|c| c.connection_address.base)
            .ok_or(SdpParseError::MissingConnectionAddress)?,
    );

    let peer_port = PeerPort(audio_media.media.port);

    // Parse all offered payload types
    let payload_types: Vec<u8> = audio_media
        .media
        .fmt
        .split_whitespace()
        .filter_map(|pt| pt.parse().ok())
        .collect();

    // Build codec info for each payload type
    let mut codecs = Vec::new();
    for pt in &payload_types {
        let codec_name = match pt {
            0 => "PCMU".to_string(),
            8 => "PCMA".to_string(),
            _ => {
                // Try to find rtpmap attribute for this payload type
                let mut found_codec = None;
                for attr in &audio_media.attributes {
                    if let sdp_rs::lines::Attribute::Rtpmap(rtpmap) = attr {
                        if rtpmap.payload_type == *pt as u32 {
                            found_codec = Some(rtpmap.encoding_name.clone());
                            break;
                        }
                    }
                }
                found_codec.unwrap_or_else(|| format!("PT{}", pt))
            }
        };
        codecs.push(CodecInfo {
            payload_type: *pt,
            codec_name,
        });
    }

    // Select the first codec we support (respecting client's preference order)
    let selected = codecs
        .iter()
        .find(|c| is_supported_codec(&c.codec_name))
        .cloned()
        .unwrap_or_else(|| {
            // Fallback to first offered codec if none supported
            codecs.first().cloned().unwrap_or(CodecInfo {
                payload_type: 0,
                codec_name: "PCMU".to_string(),
            })
        });

    Ok(SdpOffer {
        peer_addr,
        peer_port,
        codecs,
        payload_type: selected.payload_type,
        codec_name: selected.codec_name,
    })
}
#[derive(Copy, Clone, Debug)]
pub struct AdvertiseIpAddr(pub IpAddr);

impl std::str::FromStr for AdvertiseIpAddr {
    type Err = std::net::AddrParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<IpAddr>().map(AdvertiseIpAddr)
    }
}

/// Generate an SDP answer based on the offered codecs
///
/// Only includes codecs that were both offered and are supported by us.
pub fn generate_sdp_answer(
    advertise_ip_addr: AdvertiseIpAddr,
    rtp_port: u16,
    session_id: u64,
    offered_codecs: &[CodecInfo],
) -> String {
    // Filter to only codecs we support, preserving offer order
    let supported: Vec<&CodecInfo> = offered_codecs
        .iter()
        .filter(|c| is_supported_codec(&c.codec_name))
        .collect();

    // Build payload type list for m= line
    let pt_list: String = supported
        .iter()
        .map(|c| c.payload_type.to_string())
        .collect::<Vec<_>>()
        .join(" ");

    // Build rtpmap attributes
    let mut rtpmap_lines = String::new();
    for codec in &supported {
        let rtpmap = match codec.codec_name.to_ascii_lowercase().as_str() {
            "opus" => format!(
                "a=rtpmap:{} opus/48000/2\r\na=fmtp:{} minptime=10;useinbandfec=1\r\n",
                codec.payload_type, codec.payload_type
            ),
            "pcmu" => format!("a=rtpmap:{} PCMU/8000\r\n", codec.payload_type),
            "pcma" => format!("a=rtpmap:{} PCMA/8000\r\n", codec.payload_type),
            _ => continue,
        };
        rtpmap_lines.push_str(&rtpmap);
    }

    format!(
        "v=0\r\n\
         o=- {} 1 IN IP4 {}\r\n\
         s=rsipstack-server\r\n\
         c=IN IP4 {}\r\n\
         t=0 0\r\n\
         m=audio {} RTP/AVP {}\r\n\
         {}a=ptime:20\r\n\
         a=sendrecv\r\n",
        session_id, advertise_ip_addr.0, advertise_ip_addr.0, rtp_port, pt_list, rtpmap_lines
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
            SdpParseError::MissingConnectionAddress => {
                write!(f, "Missing connection address in SDP")
            }
            SdpParseError::InvalidAddress(addr) => write!(f, "Invalid address in SDP: {}", addr),
            SdpParseError::NoAudioMedia => write!(f, "No audio media in SDP"),
        }
    }
}

impl std::error::Error for SdpParseError {}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

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
        assert_eq!(offer.peer_port.0, 5000);
        assert_eq!(offer.payload_type, 0);
        assert_eq!(offer.codec_name, "PCMU");
        // Should have both codecs
        assert_eq!(offer.codecs.len(), 2);
        assert_eq!(offer.codecs[0].payload_type, 0);
        assert_eq!(offer.codecs[0].codec_name, "PCMU");
        assert_eq!(offer.codecs[1].payload_type, 8);
        assert_eq!(offer.codecs[1].codec_name, "PCMA");
    }

    #[test]
    fn test_generate_sdp_answer_all_codecs() {
        let ip: AdvertiseIpAddr = AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        let offered = vec![
            CodecInfo {
                payload_type: 111,
                codec_name: "opus".to_string(),
            },
            CodecInfo {
                payload_type: 0,
                codec_name: "PCMU".to_string(),
            },
            CodecInfo {
                payload_type: 8,
                codec_name: "PCMA".to_string(),
            },
        ];
        let sdp = generate_sdp_answer(ip, 6000, 123456, &offered);

        assert!(sdp.contains("c=IN IP4 192.168.1.1"));
        assert!(sdp.contains("m=audio 6000 RTP/AVP 111 0 8"));
        assert!(sdp.contains("a=rtpmap:111 opus/48000/2"));
        assert!(sdp.contains("a=fmtp:111 minptime=10;useinbandfec=1"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        assert!(sdp.contains("a=rtpmap:8 PCMA/8000"));
    }

    #[test]
    fn test_generate_sdp_answer_pcmu_only() {
        let ip: AdvertiseIpAddr = AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        let offered = vec![CodecInfo {
            payload_type: 0,
            codec_name: "PCMU".to_string(),
        }];
        let sdp = generate_sdp_answer(ip, 6000, 123456, &offered);

        assert!(sdp.contains("m=audio 6000 RTP/AVP 0\r\n"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        // Should NOT contain opus or PCMA
        assert!(!sdp.contains("opus"));
        assert!(!sdp.contains("PCMA"));
    }

    #[test]
    fn test_generate_sdp_answer_filters_unsupported() {
        let ip: AdvertiseIpAddr = AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        // Offer includes an unsupported codec
        let offered = vec![
            CodecInfo {
                payload_type: 99,
                codec_name: "G729".to_string(),
            },
            CodecInfo {
                payload_type: 0,
                codec_name: "PCMU".to_string(),
            },
        ];
        let sdp = generate_sdp_answer(ip, 6000, 123456, &offered);

        // Should only contain PCMU, not G729
        assert!(sdp.contains("m=audio 6000 RTP/AVP 0\r\n"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        assert!(!sdp.contains("G729"));
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
        assert_eq!(offer.peer_port.0, 5000);
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
        assert_eq!(offer.peer_port.0, 4000);
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
        assert_eq!(offer.peer_port.0, 5000);
        assert_eq!(offer.payload_type, 96);
        assert_eq!(offer.codec_name, "opus");
    }

    #[test]
    fn test_parse_sdp_offer_complex() {
        let sdp = "v=0\n\
                   o=- 3754764223 37547642423 IN IP4 12.22.0.39\n\
                   s=My System\n\
                   t=0 0\n\
                   m=audio 53264 RTP/AVP 115 9 8 0 103 101\n\
                   c=IN IP4 12.22.0.39\n\
                   a=rtpmap:115 opus/48000/2\n\
                   a=rtpmap:9 G722/8000\n\
                   a=rtpmap:8 PCMA/8000\n\
                   a=rtpmap:0 PCMU/8000\n\
                   a=rtpmap:103 telephone-event/48000\n\
                   a=fmtp:103 0-15\n\
                   a=rtpmap:101 telephone-event/8000\n\
                   a=fmtp:101 0-15\n\
                   a=sendrecv\n\
                   a=rtcp:53265\n\
                   a=rtcp-mux\n\
                   a=ice-ufrag:GEmr7dsjs\n\
                   a=ice-pwd:askldkjad\n\
                   a=candidate:akjaslkjakajd 1 UDP 213012312331 12.22.0.39 53134 typ host\n\
                   a=candidate:akjaslkjakajd 2 UDP 213012312330 12.22.0.39 53135 typ host\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.peer_addr.to_string(), "12.22.0.39");
        assert_eq!(offer.peer_port.0, 53264);
        // Should select opus (PT 115) as it's first and we support it
        assert_eq!(offer.payload_type, 115);
        assert_eq!(offer.codec_name, "opus");
        // Should have all 6 codecs
        assert_eq!(offer.codecs.len(), 6);
        assert_eq!(offer.codecs[0].payload_type, 115);
        assert_eq!(offer.codecs[0].codec_name, "opus");
        assert_eq!(offer.codecs[1].payload_type, 9);
        assert_eq!(offer.codecs[1].codec_name, "G722");
        assert_eq!(offer.codecs[2].payload_type, 8);
        assert_eq!(offer.codecs[2].codec_name, "PCMA");
        assert_eq!(offer.codecs[3].payload_type, 0);
        assert_eq!(offer.codecs[3].codec_name, "PCMU");
    }
}
