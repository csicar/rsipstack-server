//! Call Handler - Handles incoming INVITE requests

use crate::audio::handler::AudioHandler;
use crate::media::rtp::maybe_find_port_pair;
use crate::media::sdp::AdvertiseIpAddr;
use crate::media::sdp::parse_sdp_offer;
use crate::media::session::MediaSession;
use crate::media::PeerSocketAddr;
use crate::server::ServerState;
use rsipstack::dialog::server_dialog::ServerInviteDialog;
use rsipstack::sip as rsip;
use rsipstack::Result;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Handles an incoming call
pub struct CallHandler<H: AudioHandler> {
    state: Arc<ServerState>,
    dialog: ServerInviteDialog,
    audio_handler: H,
}

impl<H: AudioHandler + 'static> CallHandler<H> {
    /// Create a new call handler
    pub fn new(state: Arc<ServerState>, dialog: ServerInviteDialog, audio_handler: H) -> Self {
        Self {
            state,
            dialog,
            audio_handler,
        }
    }

    /// Handle the incoming call
    pub async fn handle_call(self) -> Result<()> {
        let dialog_id = self.dialog.id();
        info!(dialog_id = %dialog_id, "Handling incoming call");

        // Parse the SDP offer from the INVITE request
        let initial_request = self.dialog.initial_request();
        let body = String::from_utf8_lossy(initial_request.body()).to_string();

        let offer = match parse_sdp_offer(&body) {
            Ok(offer) => {
                debug!(
                    dialog_id = %dialog_id,
                    peer_addr = %offer.peer_addr.0,
                    peer_port = offer.peer_port.0,
                    "Parsed SDP offer"
                );
                offer
            }
            Err(e) => {
                warn!(dialog_id = %dialog_id, error = ?e, "Failed to parse SDP offer");
                self.dialog
                    .reject(Some(rsip::StatusCode::NotAcceptableHere), None)?;
                return Ok(());
            }
        };

        // Bind  a free port pair
        let Some(rtp_socket_pair) =
            maybe_find_port_pair(&self.state.rtp_port_range, self.state.local_ip_addr).await
        else {
            warn!(dialog_id = %dialog_id, "Failed to find and bind free RTP/RTCP port pair.");
            self.dialog.reject(
                Some(rsip::StatusCode::ServiceUnavailable),
                Some("No free RTP/RTCP port pair available".to_string()),
            )?;
            return Ok(());
        };
        debug!("RTP/RTCP Socket pair bound to {rtp_socket_pair:?}");

        // Connect so we only accept traffic from a known peer address
        let peer_socket_addr = PeerSocketAddr::new(offer.peer_addr, offer.peer_port);
        let connected_socket_pair = match rtp_socket_pair.connect(&peer_socket_addr).await {
            Ok(pair) => pair,
            Err(e) => {
                warn!(dialog_id = %dialog_id, error = %e, "Unable to connect to to peer {peer_socket_addr:?}");
                self.dialog.reject(
                    Some(rsip::StatusCode::ServerInternalError),
                    Some("Unable to connect to RTP peer".to_string()),
                )?;
                return Ok(());
            }
        };

        let advertise_ip = AdvertiseIpAddr(self.state.media_ip());

        let media_session = match MediaSession::new(
            connected_socket_pair,
            advertise_ip,
            &offer,
            self.dialog.cancel_token().child_token(),
        )
        .await
        {
            Ok(session) => session,
            Err(e) => {
                error!(dialog_id = %dialog_id, error = ?e, "Failed to create media session");
                self.dialog
                    .reject(Some(rsip::StatusCode::ServerInternalError), None)?;
                return Ok(());
            }
        };

        // Generate SDP answer
        let sdp_answer = media_session.generate_sdp_answer();
        debug!(dialog_id = %dialog_id, "Generated SDP answer:\n{}", sdp_answer);

        // Send 180 Ringing (optional, for better UX)
        if let Err(e) = self.dialog.ringing(None, None) {
            warn!(dialog_id = %dialog_id, error = ?e, "Failed to send ringing");
        }

        // Accept the call with SDP answer
        let headers = vec![rsip::Header::ContentType("application/sdp".into())];
        if let Err(e) = self
            .dialog
            .accept(Some(headers), Some(sdp_answer.into_bytes()))
        {
            error!(dialog_id = %dialog_id, error = ?e, "Failed to accept call");
            return Ok(());
        }

        info!(dialog_id = %dialog_id, "Call accepted, starting audio handler");

        // Start the media session and audio handler
        let (audio_in, audio_out) = media_session.start().await;

        let dialog_cancel = self.dialog.cancel_token().clone();

        // Extract headers from the INVITE request
        let headers = initial_request.headers.0.to_vec();

        // Run audio handler in a separate task
        let handler_task = tokio::spawn(async move {
            self.audio_handler
                .process(audio_in, audio_out, dialog_cancel, headers)
                .await;
        });

        // Wait for the dialog to be terminated
        self.dialog.cancel_token().cancelled().await;

        info!(dialog_id = %dialog_id, "Call ended");

        // Clean up
        handler_task.abort();

        // Send BYE to ensure the SIP call is properly closed.
        // This is safe to call even if the remote already sent BYE -
        // bye() is a no-op if the dialog is already terminated.
        if let Err(e) = self.dialog.bye().await {
            warn!(dialog_id = %dialog_id, error = ?e, "Failed to send BYE");
        }

        Ok(())
    }
}
