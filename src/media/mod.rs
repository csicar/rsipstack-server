//! Media handling module

use std::net::SocketAddr;

use crate::media::sdp::{PeerIpAddr, PeerPort};

pub mod deadline;
pub mod rtp;
pub mod sdp;
pub mod session;

#[derive(Debug)]
pub struct PeerSocketAddr(pub SocketAddr);

impl PeerSocketAddr {
    pub fn new(peer_ip_addr: PeerIpAddr, peer_port: PeerPort) -> Self {
        PeerSocketAddr(SocketAddr::new(peer_ip_addr.0, peer_port.0))
    }
}
