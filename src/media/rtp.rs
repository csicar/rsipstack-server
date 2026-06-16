//! RTP packet handling

use std::{
    io,
    net::SocketAddr,
    sync::atomic::{AtomicU16, Ordering::Relaxed},
};

use metrics::{counter, gauge, histogram};
use rsipstack::Error::Error;
use rtp_rs::RtpReader;
use tokio::net::UdpSocket;
use tracing::{debug, trace, warn};

use crate::{media::PeerSocketAddr, server::LocalIpAddr};

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

#[derive(PartialEq, Debug, Copy, Clone)]
pub struct RtpPortPair {
    pub rtp_port: u16,
    pub rtcp_port: u16,
}
pub struct RtpPortRange {
    first_rtp_port: u16,
    last_rtp_port: u16,
    current_rtp_port: AtomicU16,
}

impl RtpPortRange {
    pub fn new(min_port: u16, max_port: u16) -> Result<RtpPortRange, rsipstack::Error> {
        if max_port <= min_port {
            return Err(Error(format!(
                "Max port {max_port} must be larger than min port {min_port}."
            )));
        };

        if !min_port.is_multiple_of(2) {
            warn!("RFC 3550 recommends to use an even port number for RTP. min_port={min_port}");
        };
        let last_rtp_port = if max_port.is_multiple_of(2) {
            let last_rtp_port = max_port - 2;
            warn!(
                "RFC 3550 recommends to use an uneven port number for RTCP. max_port={max_port}. The last
                RTCP port used will be {}", last_rtp_port + 1
            );
            last_rtp_port
        } else {
            max_port - 1
        };

        let range = RtpPortRange {
            first_rtp_port: min_port,
            last_rtp_port,
            current_rtp_port: AtomicU16::new(min_port),
        };
        gauge!(unit: metrics::Unit::Count, description: "Upper bound of allocatable RTP/RTCP port pairs. OS may have some ports bound.", "rsipstack_server.ports.pair_capacity").set(range.capacity());

        Ok(range)
    }

    pub fn next_port_pair(&self) -> RtpPortPair {
        // This cannot result in a deadlock because one thread always makes progress
        // TODO: try out `try_update`
        loop {
            let current_port = self.current_rtp_port.load(Relaxed);
            let next_port = if current_port + 2 > self.last_rtp_port {
                self.first_rtp_port
            } else {
                current_port + 2
            };
            if self
                .current_rtp_port
                .compare_exchange(current_port, next_port, Relaxed, Relaxed)
                .is_ok()
            {
                return RtpPortPair {
                    rtp_port: current_port,
                    rtcp_port: current_port + 1,
                };
            }
        }
    }

    pub fn capacity(&self) -> u16 {
        // TODO: add metric for capacity?
        (self.last_rtp_port - self.first_rtp_port) / 2 + 1
    }
}

#[derive(Debug)]
pub struct RtpSocketPair {
    pub rtp_port_pair: RtpPortPair,
    pub rtp_socket: UdpSocket,
    pub rtcp_socket: UdpSocket,
}

impl RtpSocketPair {
    pub async fn connect(
        self,
        peer_socket_addr: &PeerSocketAddr,
    ) -> io::Result<ConnectedSocketPair> {
        self.rtp_socket.connect(&peer_socket_addr.0).await?;
        self.rtcp_socket.connect(&peer_socket_addr.0).await?;
        Ok(ConnectedSocketPair(self))
    }

    async fn new(rtp_port_pair: RtpPortPair, local_ip_addr: LocalIpAddr) -> Option<Self> {
        let rtp_addr = SocketAddr::new(local_ip_addr.0, rtp_port_pair.rtp_port);
        let Ok(rtp_socket) = UdpSocket::bind(rtp_addr).await else {
            return None;
        };
        let rtcp_addr = SocketAddr::new(local_ip_addr.0, rtp_port_pair.rtcp_port);
        let Ok(rtcp_socket) = UdpSocket::bind(rtcp_addr).await else {
            return None;
        };
        Some(RtpSocketPair {
            rtp_port_pair,
            rtp_socket,
            rtcp_socket,
        })
    }
}

#[derive(Debug)]
pub struct ConnectedSocketPair(pub RtpSocketPair);

impl ConnectedSocketPair {
    pub fn ports(&self) -> RtpPortPair {
        self.0.rtp_port_pair
    }

    pub fn rtp_socket(self) -> UdpSocket {
        self.0.rtp_socket
    }
}

pub async fn try_allocate_socket_pair(
    rtp_port_range: &RtpPortRange,
    local_ip_addr: LocalIpAddr,
) -> Option<RtpSocketPair> {
    for num_attempts in 0..rtp_port_range.capacity() {
        let port_pair = rtp_port_range.next_port_pair();
        trace!("Attempting to bind to port pair {:?}", port_pair);
        if let Some(socket_pair) = RtpSocketPair::new(port_pair, local_ip_addr).await {
            debug!(
                "Found free socket pair {socket_pair:?} after {} attempts",
                num_attempts + 1
            );
            histogram!(
                unit: metrics::Unit::Count,
                description: "Number of attempts before free port pair was found",
                "rsipstack_server.ports.allocation_attempts"
            )
            .record(num_attempts + 1);
            return Some(socket_pair);
        }
    }
    counter!("rsipstack_server.ports.allocation_failures").increment(1);
    None
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

    mod try_allocate_socket_pair {
        use std::net::{IpAddr, Ipv4Addr};

        use super::*;

        #[tokio::test]
        async fn find_free_port_pair() {
            let local_ip_addr = LocalIpAddr(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
            let mut bound_sockets: Vec<UdpSocket> = vec![];
            for port in 10100..10104 {
                bound_sockets.push(
                    UdpSocket::bind(SocketAddr::new(local_ip_addr.0, port))
                        .await
                        .unwrap(),
                );
            }
            let rtp_port_range = RtpPortRange::new(10100, 10105).unwrap();
            let free_port_pair = try_allocate_socket_pair(&rtp_port_range, local_ip_addr)
                .await
                .unwrap();
            let target_rtp_socket = SocketAddr::new(local_ip_addr.0, 10104);
            let target_rtcp_socket = SocketAddr::new(local_ip_addr.0, 10105);
            assert_eq!(
                free_port_pair.rtp_socket.local_addr().unwrap(),
                target_rtp_socket
            );
            assert_eq!(
                free_port_pair.rtcp_socket.local_addr().unwrap(),
                target_rtcp_socket
            );
        }

        #[tokio::test]
        async fn no_free_ports_left() {
            let local_ip_addr = LocalIpAddr(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
            let mut bound_sockets: Vec<UdpSocket> = vec![];
            for port in 10400..=10405 {
                bound_sockets.push(
                    UdpSocket::bind(SocketAddr::new(local_ip_addr.0, port))
                        .await
                        .unwrap(),
                );
            }
            let rtp_port_range = RtpPortRange::new(10400, 10405).unwrap();
            let free_port_pair = try_allocate_socket_pair(&rtp_port_range, local_ip_addr).await;
            assert!(free_port_pair.is_none());
        }

        #[tokio::test]
        async fn port_in_between_bound() {
            let local_ip_addr = LocalIpAddr(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
            let _rtcp_first_pair = UdpSocket::bind(SocketAddr::new(local_ip_addr.0, 10501))
                .await
                .unwrap();

            let rtp_port_range = RtpPortRange::new(10500, 10503).unwrap();
            let free_port_pair = try_allocate_socket_pair(&rtp_port_range, local_ip_addr)
                .await
                .unwrap();
            let target_rtp_socket = SocketAddr::new(local_ip_addr.0, 10502);
            let target_rtcp_socket = SocketAddr::new(local_ip_addr.0, 10503);
            assert_eq!(
                free_port_pair.rtp_socket.local_addr().unwrap(),
                target_rtp_socket
            );
            assert_eq!(
                free_port_pair.rtcp_socket.local_addr().unwrap(),
                target_rtcp_socket
            );
        }
    }

    mod rtp_port_range {
        use super::*;
        #[test]
        fn next_port_pair() {
            let rtp_port_range = RtpPortRange::new(10000, 10009).unwrap();
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001,
                }
            )
        }

        #[test]
        fn wrap_around() {
            let rtp_port_range = RtpPortRange::new(10000, 10003).unwrap();
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001
                }
            );
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10002,
                    rtcp_port: 10003
                }
            );
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001
                }
            );
        }
        #[test]
        fn wrap_around_singleton_pair() {
            let rtp_port_range = RtpPortRange::new(10000, 10001).unwrap();
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001
                }
            );
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001
                }
            )
        }

        #[test]
        fn invalid_range() {
            assert!(RtpPortRange::new(10000, 9999).is_err());
        }

        #[test]
        fn capacity() {
            assert_eq!(RtpPortRange::new(10000, 10009).unwrap().capacity(), 5);
        }

        #[test]
        fn even_max_port() {
            let rtp_port_range = RtpPortRange::new(10000, 10002).unwrap();
            assert_eq!(rtp_port_range.last_rtp_port, 10000);
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001,
                }
            );
            assert_eq!(
                rtp_port_range.next_port_pair(),
                RtpPortPair {
                    rtp_port: 10000,
                    rtcp_port: 10001,
                }
            )
        }
    }
}
