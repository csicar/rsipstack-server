//! Media Session - RTP socket management and audio channel interface

use super::rtp::{build_rtp_packet, parse_rtp_packet, AudioFrame, RtpSendState};
use super::sdp::{generate_sdp_answer, CodecInfo, SdpOffer};
use crate::codec::{create_codec, Codec};
use crate::media::rtp::ConnectedSocketPair;
use crate::media::sdp::AdvertiseIpAddr;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace, warn};

/// Media session for handling RTP audio
pub struct MediaSession {
    /// IP address to advertise in SDP (external IP for NAT, or local IP)
    advertise_ip_addr: AdvertiseIpAddr,
    /// Connected RTP/RTCP port sockets
    rtp_socket_pair: ConnectedSocketPair,
    /// Selected payload type
    payload_type: u8,
    /// Codec name (for dynamic payload types)
    codec_name: String,
    /// All codecs offered by the peer
    offered_codecs: Vec<CodecInfo>,
    /// Session ID for SDP
    session_id: u64,
    /// Cancellation token
    cancel_token: CancellationToken,
}

impl MediaSession {
    pub async fn new(
        rtp_socket_pair: ConnectedSocketPair,
        advertise_ip_addr: AdvertiseIpAddr,
        offer: &SdpOffer,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<Self> {
        // Bind RTP socket to local interface

        let session_id = rand::random::<u64>();

        Ok(Self {
            advertise_ip_addr,
            rtp_socket_pair,
            payload_type: offer.payload_type,
            codec_name: offer.codec_name.clone(),
            offered_codecs: offer.codecs.clone(),
            session_id,
            cancel_token,
        })
    }

    /// Generate SDP answer for this session
    ///
    /// The answer only includes codecs that were both offered by the peer
    /// and supported by us. Uses the advertise_ip for the connection address.
    pub fn generate_sdp_answer(&self) -> String {
        generate_sdp_answer(
            self.advertise_ip_addr,
            self.rtp_socket_pair.ports().rtp_port,
            self.session_id,
            &self.offered_codecs,
        )
    }

    /// Start the media session and return audio channels
    ///
    /// Returns (audio_in_receiver, audio_out_sender) for the audio handler to use.
    /// Audio frames contain decoded PCM samples at 48kHz.
    pub async fn start(
        self,
    ) -> (
        mpsc::UnboundedReceiver<AudioFrame>,
        mpsc::UnboundedSender<AudioFrame>,
    ) {
        // Create channels for audio frames
        let (audio_in_tx, audio_in_rx) = mpsc::unbounded_channel::<AudioFrame>();
        let (audio_out_tx, audio_out_rx) = mpsc::unbounded_channel::<AudioFrame>();

        let rtp_socket = std::sync::Arc::new(self.rtp_socket_pair.rtp_socket());
        let cancel_token = self.cancel_token.clone();

        // Create codec for receiving (decoding)
        let recv_codec = create_codec(self.payload_type, Some(&self.codec_name));
        if recv_codec.is_none() {
            warn!(
                "No codec for payload type {} ({}), using passthrough",
                self.payload_type, self.codec_name
            );
        }

        // Create codec for sending (encoding)
        let send_codec = create_codec(self.payload_type, Some(&self.codec_name));

        // Spawn RTP receive task
        let recv_socket = rtp_socket.clone();
        let recv_cancel = cancel_token.clone();
        let recv_payload_type = self.payload_type;
        tokio::spawn(async move {
            Self::rtp_receive_task(
                recv_socket,
                audio_in_tx,
                recv_cancel,
                recv_codec,
                recv_payload_type,
            )
            .await;
        });

        // Spawn RTP send task
        let send_socket = rtp_socket;
        let send_cancel = cancel_token;
        let payload_type = self.payload_type;
        tokio::spawn(async move {
            Self::rtp_send_task(
                send_socket,
                audio_out_rx,
                send_cancel,
                send_codec,
                payload_type,
            )
            .await;
        });

        (audio_in_rx, audio_out_tx)
    }

    /// RTP receive task - receives RTP packets, decodes them, and sends AudioFrames to the channel
    async fn rtp_receive_task(
        socket: std::sync::Arc<UdpSocket>,
        audio_tx: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
        mut codec: Option<Box<dyn Codec>>,
        default_payload_type: u8,
    ) {
        let mut buf = vec![0u8; 2048];
        let mut packet_count = 0u64;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!("RTP receive task cancelled after {} packets", packet_count);
                    break;
                }
                result = socket.recv(&mut buf) => {
                    match result {
                        Ok(len) => {
                            trace!(
                                from = ?socket.peer_addr(),
                                len = len,
                                "Received RTP packet"
                            );

                            if let Some(raw) = parse_rtp_packet(&buf[..len]) {
                                packet_count += 1;
                                if packet_count % 500 == 1 {
                                    debug!(
                                        count = packet_count,
                                        seq = raw.sequence,
                                        ts = raw.timestamp,
                                        pt = raw.payload_type,
                                        "RTP receive progress"
                                    );
                                }

                                // Decode the payload using codec
                                let samples = if let Some(ref mut c) = codec {
                                    c.decode(&raw.payload)
                                } else {
                                    // Passthrough: interpret bytes as samples (for testing)
                                    raw.payload.iter().map(|&b| (b as i16 - 128) * 256).collect()
                                };

                                let frame = AudioFrame { samples };

                                if audio_tx.send(frame).is_err() {
                                    debug!("Audio channel closed, stopping receive task");
                                    break;
                                }
                            } else {
                                warn!(len = len, "Failed to parse RTP packet");
                            }
                        }
                        Err(e) => {
                            error!("RTP receive error: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        let _ = default_payload_type; // Keep parameter for future use
    }

    /// RTP send task - receives AudioFrames from the channel, encodes them, and sends RTP packets
    async fn rtp_send_task(
        socket: std::sync::Arc<UdpSocket>,
        mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
        cancel_token: CancellationToken,
        mut codec: Option<Box<dyn Codec>>,
        payload_type: u8,
    ) {
        let mut packet_count = 0u64;
        let mut rtp_state = RtpSendState::new(payload_type);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!("RTP send task cancelled after {} packets", packet_count);
                    break;
                }
                frame = audio_rx.recv() => {
                    match frame {
                        Some(frame) => {
                            // Encode the samples using codec
                            let payload = if let Some(ref mut c) = codec {
                                c.encode(&frame.samples)
                            } else {
                                // Passthrough: convert samples back to bytes (for testing)
                                frame.samples.iter().map(|&s| ((s / 256) + 128) as u8).collect()
                            };

                            // Skip empty payloads (codec error)
                            if payload.is_empty() {
                                continue;
                            }

                            // Get RTP header values from internal state
                            let rtp = rtp_state.next();
                            let packet = build_rtp_packet(&payload, &rtp);

                            match socket.send(&packet).await {
                                Ok(_) => {
                                    packet_count += 1;
                                    if packet_count % 500 == 1 {
                                        debug!(
                                            count = packet_count,
                                            seq = rtp.sequence,
                                            ts = rtp.timestamp,
                                            peer = ?socket.peer_addr(),
                                            "RTP send progress"
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!("RTP send error: {}", e);
                                    break;
                                }
                            }
                        }
                        None => {
                            debug!("Audio channel closed, stopping send task");
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use crate::{
        media::{
            rtp::{try_allocate_socket_pair, RtpPortRange},
            sdp::{PeerIpAddr, PeerPort},
            PeerSocketAddr,
        },
        server::LocalIpAddr,
    };

    use super::*;

    #[tokio::test]
    async fn test_media_session_sdp_pcmu_only() {
        let peer_addr = PeerIpAddr("127.0.0.1".parse().unwrap());
        let peer_port = PeerPort(5000);
        let peer_socket_addr = PeerSocketAddr::new(peer_addr, peer_port);

        let offer = SdpOffer {
            peer_addr,
            peer_port,
            codecs: vec![CodecInfo {
                payload_type: 0,
                codec_name: "PCMU".to_string(),
            }],
            payload_type: 0,
            codec_name: "PCMU".to_string(),
        };

        // Use a random high port to avoid collisions
        let test_port = 40000 + (rand::random::<u16>() % 10000);
        let test_port = test_port & !1; // Ensure even port
        let rtp_port_range = RtpPortRange::new(test_port, test_port + 1).unwrap();
        let local_ip_addr = LocalIpAddr(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        let port_pair = try_allocate_socket_pair(&rtp_port_range, local_ip_addr)
            .await
            .unwrap()
            .connect(&peer_socket_addr)
            .await
            .unwrap();

        let cancel_token = CancellationToken::new();
        let local_ip = AdvertiseIpAddr(local_ip_addr.0);
        let session = MediaSession::new(
            port_pair,
            local_ip, // In tests, bind and advertise are the same
            &offer,
            cancel_token,
        )
        .await
        .unwrap();

        let sdp = session.generate_sdp_answer();
        assert!(sdp.contains(&format!("m=audio {}", test_port)));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        // Should NOT contain opus since it wasn't offered
        assert!(!sdp.contains("opus"));
    }

    #[tokio::test]
    async fn test_media_session_sdp_multiple_codecs() {
        let peer_addr = PeerIpAddr("127.0.0.1".parse().unwrap());
        let peer_port = PeerPort(5000);
        let peer_socket_addr = PeerSocketAddr::new(peer_addr, peer_port);

        let offer = SdpOffer {
            peer_addr: peer_addr,
            peer_port: peer_port,
            codecs: vec![
                CodecInfo {
                    payload_type: 111,
                    codec_name: "opus".to_string(),
                },
                CodecInfo {
                    payload_type: 0,
                    codec_name: "PCMU".to_string(),
                },
            ],
            payload_type: 111,
            codec_name: "opus".to_string(),
        };

        let test_port = 40000 + (rand::random::<u16>() % 10000);
        let test_port = test_port & !1;

        let cancel_token = CancellationToken::new();

        let rtp_port_range = RtpPortRange::new(test_port, test_port + 1).unwrap();
        let local_ip_addr = LocalIpAddr(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        let port_pair = try_allocate_socket_pair(&rtp_port_range, local_ip_addr)
            .await
            .unwrap()
            .connect(&peer_socket_addr)
            .await
            .unwrap();

        let session = MediaSession::new(
            port_pair,
            AdvertiseIpAddr(local_ip_addr.0), // In tests, bind and advertise are the same
            &offer,
            cancel_token,
        )
        .await
        .unwrap();

        let sdp = session.generate_sdp_answer();
        // Should contain both offered codecs
        assert!(sdp.contains("opus/48000"));
        assert!(sdp.contains("PCMU/8000"));
        // Payload types should match what was offered
        assert!(sdp.contains("a=rtpmap:111 opus"));
        assert!(sdp.contains("a=rtpmap:0 PCMU"));
    }
}
