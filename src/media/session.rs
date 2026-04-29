//! Media Session - RTP socket management and audio channel interface

use super::rtp::{build_rtp_packet, parse_rtp_packet, AudioFrame};
use super::sdp::{generate_sdp_answer, SdpOffer};
use std::net::{IpAddr, SocketAddr};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace, warn};

/// Media session for handling RTP audio
pub struct MediaSession {
    /// Local IP address
    local_ip: IpAddr,
    /// Local RTP port
    rtp_port: u16,
    /// UDP socket for RTP
    rtp_socket: UdpSocket,
    /// Peer address for sending RTP
    peer_addr: SocketAddr,
    /// Selected payload type
    payload_type: u8,
    /// Session ID for SDP
    session_id: u64,
    /// Cancellation token
    cancel_token: CancellationToken,
}

impl MediaSession {
    /// Create a new media session
    pub async fn new(
        local_ip: IpAddr,
        rtp_port: u16,
        offer: &SdpOffer,
        cancel_token: CancellationToken,
    ) -> std::io::Result<Self> {
        // Bind RTP socket
        let rtp_addr = SocketAddr::new(local_ip, rtp_port);
        let rtp_socket = UdpSocket::bind(rtp_addr).await?;

        debug!("RTP socket bound to {}", rtp_addr);

        // Also bind RTCP socket (RTP port + 1) but don't process it yet
        let rtcp_addr = SocketAddr::new(local_ip, rtp_port + 1);
        match UdpSocket::bind(rtcp_addr).await {
            Ok(_) => debug!("RTCP socket bound to {}", rtcp_addr),
            Err(e) => warn!("Failed to bind RTCP socket {}: {}", rtcp_addr, e),
        }

        let peer_addr = SocketAddr::new(offer.peer_addr, offer.peer_port);
        let session_id = rand::random::<u64>();

        Ok(Self {
            local_ip,
            rtp_port,
            rtp_socket,
            peer_addr,
            payload_type: offer.payload_type,
            session_id,
            cancel_token,
        })
    }

    /// Generate SDP answer for this session
    pub fn generate_sdp_answer(&self) -> String {
        generate_sdp_answer(self.local_ip, self.rtp_port, self.session_id, self.payload_type)
    }

    /// Start the media session and return audio channels
    ///
    /// Returns (audio_in_receiver, audio_out_sender) for the audio handler to use
    pub async fn start(
        self,
    ) -> (
        mpsc::UnboundedReceiver<AudioFrame>,
        mpsc::UnboundedSender<AudioFrame>,
    ) {
        // Create channels for audio frames
        let (audio_in_tx, audio_in_rx) = mpsc::unbounded_channel::<AudioFrame>();
        let (audio_out_tx, audio_out_rx) = mpsc::unbounded_channel::<AudioFrame>();

        let rtp_socket = std::sync::Arc::new(self.rtp_socket);
        let cancel_token = self.cancel_token.clone();

        // Spawn RTP receive task
        let recv_socket = rtp_socket.clone();
        let recv_cancel = cancel_token.clone();
        let recv_peer = self.peer_addr;
        tokio::spawn(async move {
            Self::rtp_receive_task(recv_socket, audio_in_tx, recv_cancel, recv_peer).await;
        });

        // Spawn RTP send task
        let send_socket = rtp_socket;
        let send_cancel = cancel_token;
        let send_peer = self.peer_addr;
        tokio::spawn(async move {
            Self::rtp_send_task(send_socket, audio_out_rx, send_cancel, send_peer).await;
        });

        (audio_in_rx, audio_out_tx)
    }

    /// RTP receive task - receives RTP packets and sends AudioFrames to the channel
    async fn rtp_receive_task(
        socket: std::sync::Arc<UdpSocket>,
        audio_tx: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
        _expected_peer: SocketAddr,
    ) {
        let mut buf = vec![0u8; 2048];
        let mut packet_count = 0u64;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!("RTP receive task cancelled after {} packets", packet_count);
                    break;
                }
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, addr)) => {
                            // Only accept packets from the expected peer
                            // (allow any address for now to handle NAT)
                            trace!(
                                from = %addr,
                                len = len,
                                "Received RTP packet"
                            );

                            if let Some(frame) = parse_rtp_packet(&buf[..len]) {
                                packet_count += 1;
                                if packet_count % 500 == 1 {
                                    debug!(
                                        count = packet_count,
                                        seq = frame.sequence,
                                        ts = frame.timestamp,
                                        "RTP receive progress"
                                    );
                                }
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

    /// RTP send task - receives AudioFrames from the channel and sends RTP packets
    async fn rtp_send_task(
        socket: std::sync::Arc<UdpSocket>,
        mut audio_rx: mpsc::UnboundedReceiver<AudioFrame>,
        cancel_token: CancellationToken,
        peer_addr: SocketAddr,
    ) {
        let mut packet_count = 0u64;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!("RTP send task cancelled after {} packets", packet_count);
                    break;
                }
                frame = audio_rx.recv() => {
                    match frame {
                        Some(frame) => {
                            let packet = build_rtp_packet(&frame);
                            match socket.send_to(&packet, peer_addr).await {
                                Ok(_) => {
                                    packet_count += 1;
                                    if packet_count % 500 == 1 {
                                        debug!(
                                            count = packet_count,
                                            seq = frame.sequence,
                                            ts = frame.timestamp,
                                            peer = %peer_addr,
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
    use super::*;

    #[tokio::test]
    async fn test_media_session_sdp() {
        let offer = SdpOffer {
            peer_addr: "192.168.1.100".parse().unwrap(),
            peer_port: 5000,
            payload_type: 0,
            codec_name: "PCMU".to_string(),
        };

        // Use a random high port to avoid collisions
        let test_port = 40000 + (rand::random::<u16>() % 10000);
        let test_port = test_port & !1; // Ensure even port

        let cancel_token = CancellationToken::new();
        let session = MediaSession::new(
            "127.0.0.1".parse().unwrap(),
            test_port,
            &offer,
            cancel_token,
        )
        .await
        .unwrap();

        let sdp = session.generate_sdp_answer();
        assert!(sdp.contains(&format!("m=audio {}", test_port)));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"));
    }
}
