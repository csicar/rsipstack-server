//! SDP parsing and generation

use std::net::IpAddr;
use std::time::Duration;

use crate::codec::CodecKind;

/// Sample rate of the PCM audio in `AudioFrame` and the `Codec` trait.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// PCM samples in one 20ms `AudioFrame` (320 at 16kHz).
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE_HZ as usize * 20) / 1000;

/// A codec from the peer's offer that we support, with the payload type the
/// peer uses for it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferedCodec {
    pub payload_type: u8,
    pub kind: CodecKind,
}

/// The `a=ptime` this SIP server advertises, and the RTP send cadence it
/// actually uses — the two must match, so this is their single source of
/// truth rather than a value hardcoded separately in each place. Public so
/// sip-bridge's `PacedWriter`, which targets the same 20ms period, can read
/// this value instead of keeping its own copy in sync by hand. The inner
/// field stays private so callers can only read the canonical value below,
/// not construct their own.
#[derive(Debug, Clone, Copy)]
pub struct ExpectedSendInterval(Duration);

impl ExpectedSendInterval {
    pub fn duration(&self) -> Duration {
        self.0
    }
}

pub const EXPECTED_SEND_INTERVAL: ExpectedSendInterval =
    ExpectedSendInterval(Duration::from_millis(20));

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
    /// The offered codecs we support, in the peer's order of preference
    pub codecs: Vec<OfferedCodec>,
    /// Payload type of the selected codec (the first one in `codecs`)
    pub payload_type: u8,
    /// The selected codec (the first one in `codecs`)
    pub codec_kind: CodecKind,
}

/// Parse an SDP offer and extract relevant information
///
/// Selects the first offered codec we support, respecting the caller's preference order.
/// Returns [`SdpParseError::NoSupportedCodec`] if none of the offered codecs are supported,
/// so the caller can reject the call instead of answering with an empty codec list.
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

    // Keep each offered codec we support; the rest are dropped here, so nothing
    // downstream has to filter or re-parse names
    let mut codecs = Vec::new();
    for pt in &payload_types {
        let codec_name = match pt {
            0 => "PCMU".to_string(),
            8 => "PCMA".to_string(),
            // G.722 is a static payload type; name it even without an rtpmap line.
            9 => "G722".to_string(),
            _ => {
                // Try to find rtpmap attribute for this payload type
                let mut found_codec = None;
                for attr in &audio_media.attributes {
                    if let sdp_rs::lines::Attribute::Rtpmap(rtpmap) = attr {
                        if rtpmap.payload_type == *pt as u32 {
                            if rtpmap.encoding_name.eq_ignore_ascii_case("l16")
                                && rtpmap.clock_rate != 16000
                            {
                                break;
                            }
                            found_codec = Some(rtpmap.encoding_name.clone());
                            break;
                        }
                    }
                }
                found_codec.unwrap_or_else(|| format!("PT{}", pt))
            }
        };
        if let Some(kind) = CodecKind::from_name(&codec_name) {
            codecs.push(OfferedCodec {
                payload_type: *pt,
                kind,
            });
        }
    }

    // Select the first codec we support (respecting client's preference order)
    let selected = *codecs.first().ok_or(SdpParseError::NoSupportedCodec)?;

    Ok(SdpOffer {
        peer_addr,
        peer_port,
        codecs,
        payload_type: selected.payload_type,
        codec_kind: selected.kind,
    })
}

/// Newtype wrapper around [IpAddr] denoting this address should be used for sdp / rtp offerings.
/// I.e. "How can calling sip phones find this service?"
/// In contrast [crate::server::LocalIpAddr] is used to bind ports.
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
/// `offered_codecs` only holds codecs that were both offered and are supported by us
/// (see [`parse_sdp_offer`]), so every one of them is answered.
pub fn generate_sdp_answer(
    advertise_ip_addr: AdvertiseIpAddr,
    rtp_port: u16,
    session_id: u64,
    offered_codecs: &[OfferedCodec],
) -> String {
    // Build payload type list for m= line
    let pt_list: String = offered_codecs
        .iter()
        .map(|c| c.payload_type.to_string())
        .collect::<Vec<_>>()
        .join(" ");

    // Build rtpmap attributes
    let mut rtpmap_lines = String::new();
    for codec in offered_codecs {
        let rtpmap = match codec.kind {
            #[cfg(feature = "opus")]
            CodecKind::Opus => format!(
                "a=rtpmap:{} opus/48000/2\r\na=fmtp:{} minptime=10;useinbandfec=1\r\n",
                codec.payload_type, codec.payload_type
            ),
            CodecKind::Pcmu => format!("a=rtpmap:{} PCMU/8000\r\n", codec.payload_type),
            CodecKind::Pcma => format!("a=rtpmap:{} PCMA/8000\r\n", codec.payload_type),
            // G.722 carries 16kHz audio but by convention advertises 8000.
            #[cfg(feature = "g722")]
            CodecKind::G722 => format!("a=rtpmap:{} G722/8000\r\n", codec.payload_type),
            CodecKind::L16 => format!("a=rtpmap:{} L16/16000\r\n", codec.payload_type),
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
         {}a=ptime:{}\r\n\
         a=sendrecv\r\n",
        session_id,
        advertise_ip_addr.0,
        advertise_ip_addr.0,
        rtp_port,
        pt_list,
        rtpmap_lines,
        EXPECTED_SEND_INTERVAL.duration().as_millis()
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
    NoSupportedCodec,
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
            SdpParseError::NoSupportedCodec => write!(f, "No supported codec in SDP offer"),
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
        assert_eq!(offer.codec_kind, CodecKind::Pcmu);
        // Should have both codecs
        assert_eq!(offer.codecs.len(), 2);
        assert_eq!(
            offer.codecs,
            vec![
                OfferedCodec {
                    payload_type: 0,
                    kind: CodecKind::Pcmu
                },
                OfferedCodec {
                    payload_type: 8,
                    kind: CodecKind::Pcma
                },
            ]
        );
    }

    #[cfg(feature = "opus")]
    #[test]
    fn test_generate_sdp_answer_all_codecs() {
        let ip: AdvertiseIpAddr = AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        let offered = vec![
            OfferedCodec {
                payload_type: 111,
                kind: CodecKind::Opus,
            },
            OfferedCodec {
                payload_type: 0,
                kind: CodecKind::Pcmu,
            },
            OfferedCodec {
                payload_type: 8,
                kind: CodecKind::Pcma,
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
        let offered = vec![OfferedCodec {
            payload_type: 0,
            kind: CodecKind::Pcmu,
        }];
        let sdp = generate_sdp_answer(ip, 6000, 123456, &offered);

        assert!(sdp.contains("m=audio 6000 RTP/AVP 0\r\n"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        // Should NOT contain opus or PCMA
        assert!(!sdp.contains("opus"));
        assert!(!sdp.contains("PCMA"));
    }

    #[test]
    fn test_unsupported_codecs_are_dropped_from_offer_and_answer() {
        let ip: AdvertiseIpAddr = AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        // Offer includes an unsupported codec ahead of PCMU
        let offer = parse_sdp_offer(
            "v=0\r\n\
             o=- 123456 1 IN IP4 192.168.1.100\r\n\
             s=Test\r\n\
             c=IN IP4 192.168.1.100\r\n\
             t=0 0\r\n\
             m=audio 5000 RTP/AVP 99 0\r\n\
             a=rtpmap:99 G729/8000\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        )
        .unwrap();
        assert_eq!(
            offer.codecs,
            vec![OfferedCodec {
                payload_type: 0,
                kind: CodecKind::Pcmu
            }]
        );
        let sdp = generate_sdp_answer(ip, 6000, 123456, &offer.codecs);

        // Should only contain PCMU, not G729
        assert!(sdp.contains("m=audio 6000 RTP/AVP 0\r\n"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        assert!(!sdp.contains("G729"));
    }

    #[cfg(feature = "opus")]
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
        assert_eq!(offer.codec_kind, CodecKind::Opus);
    }

    #[cfg(feature = "opus")]
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
        assert_eq!(offer.codec_kind, CodecKind::Opus);
    }

    #[cfg(feature = "opus")]
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
        assert_eq!(offer.codec_kind, CodecKind::Opus);
    }

    #[test]
    fn test_parse_sdp_offer_rejects_l16_wrong_clock_rate() {
        // PT 97 claims to be L16 but at 8000 instead of the 16000 we support.
        // The parser must not accept the rtpmap match, or L16Codec (which
        // assumes 16kHz) would silently be built for 8kHz audio.
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 97 0\r\n\
                   a=rtpmap:97 L16/8000\r\n\
                   a=rtpmap:0 PCMU/8000\r\n";

        let offer = parse_sdp_offer(sdp).unwrap();

        // The mismatched-rate rtpmap must not be accepted as a real match.
        assert!(!offer.codecs.iter().any(|c| c.payload_type == 97));

        // With L16/8000 rejected, PCMU (which we do support) must be selected.
        assert_eq!(offer.payload_type, 0);
        assert_eq!(offer.codec_kind, CodecKind::Pcmu);
    }

    #[cfg(feature = "opus")]
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
        assert_eq!(offer.codec_kind, CodecKind::Opus);
        // telephone-event (103, 101) is dropped; G.722 only exists with its feature
        let mut expected = vec![OfferedCodec {
            payload_type: 115,
            kind: CodecKind::Opus,
        }];
        #[cfg(feature = "g722")]
        expected.push(OfferedCodec {
            payload_type: 9,
            kind: CodecKind::G722,
        });
        expected.extend([
            OfferedCodec {
                payload_type: 8,
                kind: CodecKind::Pcma,
            },
            OfferedCodec {
                payload_type: 0,
                kind: CodecKind::Pcmu,
            },
        ]);
        assert_eq!(offer.codecs, expected);
    }

    #[cfg(feature = "g722")]
    #[test]
    fn test_g722_selected_when_offered() {
        // G.722 (static PT 9) is offered ahead of PCMU. G.722 wins only if it
        // is genuinely in our supported list; otherwise the selector would skip
        // it and pick PCMU.
        let sdp = "v=0\n\
                   o=- 1 1 IN IP4 12.22.0.39\n\
                   s=-\n\
                   t=0 0\n\
                   m=audio 5000 RTP/AVP 9 0\n\
                   c=IN IP4 12.22.0.39\n\
                   a=rtpmap:9 G722/8000\n\
                   a=rtpmap:0 PCMU/8000\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.payload_type, 9);
        assert_eq!(offer.codec_kind, CodecKind::G722);
    }

    #[cfg(feature = "g722")]
    #[test]
    fn test_generate_answer_includes_g722() {
        let offered = vec![OfferedCodec {
            payload_type: 9,
            kind: CodecKind::G722,
        }];
        let answer = generate_sdp_answer(
            AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
            10000,
            42,
            &offered,
        );
        assert!(
            answer.contains("a=rtpmap:9 G722/8000\r\n"),
            "answer missing G722 rtpmap: {}",
            answer
        );
        assert!(
            answer.contains("RTP/AVP 9\r\n"),
            "answer missing G722 payload type in m= line: {}",
            answer
        );
    }

    #[test]
    fn test_l16_selected_when_offered() {
        // L16 (dynamic PT 97) is offered ahead of PCMU. L16 wins only if it
        // is genuinely in our supported list; otherwise the selector would
        // skip it and pick PCMU.
        let sdp = "v=0\n\
                   o=- 1 1 IN IP4 12.22.0.39\n\
                   s=-\n\
                   t=0 0\n\
                   m=audio 5000 RTP/AVP 97 0\n\
                   c=IN IP4 12.22.0.39\n\
                   a=rtpmap:97 L16/16000\n\
                   a=rtpmap:0 PCMU/8000\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.payload_type, 97);
        assert_eq!(offer.codec_kind, CodecKind::L16);
    }

    #[test]
    fn test_generate_answer_includes_l16() {
        let offered = vec![OfferedCodec {
            payload_type: 97,
            kind: CodecKind::L16,
        }];
        let answer = generate_sdp_answer(
            AdvertiseIpAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
            10000,
            42,
            &offered,
        );
        assert!(
            answer.contains("a=rtpmap:97 L16/16000\r\n"),
            "answer missing L16 rtpmap: {}",
            answer
        );
        assert!(
            answer.contains("RTP/AVP 97\r\n"),
            "answer missing L16 payload type in m= line: {}",
            answer
        );
    }

    #[test]
    fn test_parse_sdp_offer_rejects_when_no_codec_supported() {
        // Only telephone-event is offered: nothing we can negotiate, so the
        // offer must be rejected rather than silently falling back to it.
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 101\r\n\
                   a=rtpmap:101 telephone-event/8000\r\n";

        assert!(matches!(
            parse_sdp_offer(sdp),
            Err(SdpParseError::NoSupportedCodec)
        ));
    }

    #[test]
    fn test_parse_sdp_offer_rejects_l16_wrong_clock_rate_when_only_codec() {
        // Same wrong-rate L16 as above, but with nothing else to fall back to.
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 97\r\n\
                   a=rtpmap:97 L16/8000\r\n";

        assert!(matches!(
            parse_sdp_offer(sdp),
            Err(SdpParseError::NoSupportedCodec)
        ));
    }

    #[test]
    fn test_parse_sdp_offer_selects_supported_among_unsupported() {
        // An unsupported codec listed first must not cause a rejection when a
        // supported one is offered later.
        let sdp = "v=0\r\n\
                   o=- 123456 1 IN IP4 192.168.1.100\r\n\
                   s=Test\r\n\
                   c=IN IP4 192.168.1.100\r\n\
                   t=0 0\r\n\
                   m=audio 5000 RTP/AVP 101 0\r\n\
                   a=rtpmap:101 telephone-event/8000\r\n\
                   a=rtpmap:0 PCMU/8000\r\n";

        let offer = parse_sdp_offer(sdp).unwrap();
        assert_eq!(offer.payload_type, 0);
        assert_eq!(offer.codec_kind, CodecKind::Pcmu);
    }
}
