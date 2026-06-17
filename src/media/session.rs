//! Media Session - RTP socket management and audio channel interface

use std::time::Duration;

use super::deadline::Deadline;
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
    /// Duration to wait for an RTP packet before closing the call
    media_receive_timeout: Duration,
    /// Session ID for SDP
    session_id: u64,
    /// Cancellation token
    cancel_token: CancellationToken,
}

impl MediaSession {
    pub fn new(
        rtp_socket_pair: ConnectedSocketPair,
        advertise_ip_addr: AdvertiseIpAddr,
        offer: &SdpOffer,
        cancel_token: CancellationToken,
        media_receive_timeout: Duration,
    ) -> Self {
        // Bind RTP socket to local interface

        let session_id = rand::random::<u64>();

        Self {
            advertise_ip_addr,
            rtp_socket_pair,
            payload_type: offer.payload_type,
            codec_name: offer.codec_name.clone(),
            offered_codecs: offer.codecs.clone(),
            media_receive_timeout,
            session_id,
            cancel_token,
        }
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
        tokio::spawn(async move {
            Self::rtp_receive_task(
                recv_socket,
                audio_in_tx,
                recv_cancel,
                recv_codec,
                self.media_receive_timeout,
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
        media_receive_timeout: Duration,
    ) {
        let mut buf = vec![0u8; 2048];
        let mut packet_count = 0u64;
        let mut deadline = Deadline::new(media_receive_timeout);

        loop {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => {
                    debug!("RTP receive task cancelled after {} packets", packet_count);
                    break;
                }
                _ = &mut deadline => {
                    warn!("Did not receive any RTP packet for {:?}, cancelling the call", media_receive_timeout);
                    cancel_token.cancel();
                }
                result = socket.recv(&mut buf) => {
                    match result {
                        Ok(len) => {
                            trace!(
                                from = ?socket.peer_addr(),
                                len = len,
                                "Received RTP packet"
                            );

                            deadline.reset();

                            if let Some(raw) = parse_rtp_packet(&buf[..len]) {
                                // TODO: check that raw.pt (payload type) matched selected pt from sdp
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use crate::{
        media::{
            rtp::{try_allocate_socket_pair, RtpPortRange, RtpSendState},
            sdp::{PeerIpAddr, PeerPort},
            PeerSocketAddr,
        },
        server::LocalIpAddr,
    };

    use super::*;

    struct TestSetup {
        session: MediaSession,
        cancel_token: CancellationToken,
        rtp_port: u16,
        peer_port: u16,
    }

    impl TestSetup {
        /// Create a UDP socket bound to the peer port that can send RTP to the session
        async fn create_peer_socket(&self) -> UdpSocket {
            let peer_addr: SocketAddr = format!("127.0.0.1:{}", self.peer_port).parse().unwrap();
            let socket = UdpSocket::bind(peer_addr).await.unwrap();
            let session_addr: SocketAddr = format!("127.0.0.1:{}", self.rtp_port).parse().unwrap();
            socket.connect(session_addr).await.unwrap();
            socket
        }
    }

    /// Send a single RTP packet (standalone function to use after session.start() consumes TestSetup)
    async fn send_rtp_packet(socket: &UdpSocket, state: &RtpSendState) {
        let payload = vec![0u8; 160]; // 20ms of PCMU silence
        let packet = build_rtp_packet(&payload, state);
        socket.send(&packet).await.unwrap();
    }

    async fn setup_test_session(offer: &SdpOffer, timeout: Duration) -> TestSetup {
        let peer_socket_addr = PeerSocketAddr::new(offer.peer_addr, offer.peer_port);

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
        let session = MediaSession::new(
            port_pair,
            AdvertiseIpAddr(local_ip_addr.0),
            offer,
            cancel_token.clone(),
            timeout,
        );

        TestSetup {
            session,
            cancel_token,
            rtp_port: test_port,
            peer_port: offer.peer_port.0,
        }
    }

    fn pcmu_offer() -> SdpOffer {
        // Use a random peer port to avoid collisions between parallel tests
        let peer_port = 30000 + (rand::random::<u16>() % 10000);
        let peer_addr = PeerIpAddr("127.0.0.1".parse().unwrap());
        SdpOffer {
            peer_addr,
            peer_port: PeerPort(peer_port),
            codecs: vec![CodecInfo {
                payload_type: 0,
                codec_name: "PCMU".to_string(),
            }],
            payload_type: 0,
            codec_name: "PCMU".to_string(),
        }
    }

    #[tokio::test]
    async fn test_media_session_sdp_pcmu_only() {
        let offer = pcmu_offer();
        let setup = setup_test_session(&offer, Duration::MAX).await;

        let sdp = setup.session.generate_sdp_answer();
        assert!(sdp.contains(&format!("m=audio {}", setup.rtp_port)));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
        // Should NOT contain opus since it wasn't offered
        assert!(!sdp.contains("opus"));
    }

    #[tokio::test]
    async fn test_media_session_sdp_multiple_codecs() {
        let peer_addr = PeerIpAddr("127.0.0.1".parse().unwrap());
        let peer_port = PeerPort(5000);
        let offer = SdpOffer {
            peer_addr,
            peer_port,
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

        let setup = setup_test_session(&offer, Duration::MAX).await;

        let sdp = setup.session.generate_sdp_answer();
        // Should contain both offered codecs
        assert!(sdp.contains("opus/48000"));
        assert!(sdp.contains("PCMU/8000"));
        // Payload types should match what was offered
        assert!(sdp.contains("a=rtpmap:111 opus"));
        assert!(sdp.contains("a=rtpmap:0 PCMU"));
    }

    #[tokio::test]
    async fn test_media_receive_timeout_cancels_call() {
        let offer = pcmu_offer();
        let setup = setup_test_session(&offer, Duration::from_millis(50)).await;

        // Start the session - this spawns the RTP receive task
        let (_audio_rx, _audio_tx) = setup.session.start().await;

        // Wait less than the timeout
        tokio::time::sleep(Duration::from_millis(25)).await;

        // The cancel token should NOT be cancelled yet
        assert!(
            !setup.cancel_token.is_cancelled(),
            "Cancel token should not be cancelled before timeout"
        );

        // Wait past the timeout (50ms timeout + margin)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now the cancel token should be cancelled due to no RTP packets received
        assert!(
            setup.cancel_token.is_cancelled(),
            "Cancel token should be cancelled after timeout with no RTP packets"
        );
    }

    #[tokio::test]
    async fn test_large_timeout_does_not_cancel() {
        let offer = pcmu_offer();
        let setup = setup_test_session(&offer, Duration::from_secs(3600)).await;

        // Start the session with a very large timeout
        let (_audio_rx, _audio_tx) = setup.session.start().await;

        // Wait a bit without sending any RTP packets
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The cancel token should NOT be cancelled since timeout is very large
        assert!(
            !setup.cancel_token.is_cancelled(),
            "Cancel token should not be cancelled with large timeout"
        );
    }

    #[tokio::test]
    async fn test_timeout_resets_on_rtp_packet() {
        let offer = pcmu_offer();
        let setup = setup_test_session(&offer, Duration::from_millis(100)).await;

        // Create peer socket to send RTP packets
        let peer_socket = setup.create_peer_socket().await;
        let cancel_token = setup.cancel_token.clone();

        // Start the session
        let (_audio_rx, _audio_tx) = setup.session.start().await;

        let mut rtp_state = RtpSendState {
            ssrc: 0x12345678,
            sequence: 0,
            timestamp: 0,
            payload_type: 0,
        };

        // Send packets every 50ms for 250ms total (longer than the 100ms timeout)
        // This tests that the timeout resets on each packet
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            send_rtp_packet(&peer_socket, &rtp_state).await;
            rtp_state.sequence += 1;
            rtp_state.timestamp += 160;
        }

        // Should NOT be cancelled because packets kept resetting the timeout
        assert!(
            !cancel_token.is_cancelled(),
            "Cancel token should not be cancelled while receiving RTP"
        );

        // Now stop sending packets and wait for timeout
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Still should not be cancelled (within timeout window)
        assert!(
            !cancel_token.is_cancelled(),
            "Cancel token should not be cancelled before timeout after RTP stops"
        );

        // Now stop sending packets and wait past the timeout
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Now should be cancelled
        assert!(
            cancel_token.is_cancelled(),
            "Cancel token should be cancelled after timeout when RTP stops"
        );
    }
}
