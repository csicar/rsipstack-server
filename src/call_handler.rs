//! Call Handler - Handles incoming INVITE requests

use crate::audio::handler::AudioHandler;
use crate::media::sdp::parse_sdp_offer;
use crate::media::session::MediaSession;
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
                    peer_addr = %offer.peer_addr,
                    peer_port = offer.peer_port,
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

        // Allocate RTP port and create media session
        let rtp_port = self.state.allocate_rtp_port();
        let media_ip = self.state.media_ip();

        let media_session = match MediaSession::new(
            media_ip,
            rtp_port,
            &offer,
            self.state.cancel_token.child_token(),
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

        // Run audio handler in a separate task
        let handler_task = tokio::spawn(async move {
            self.audio_handler
                .process(audio_in, audio_out, dialog_cancel)
                .await;
        });

        // Wait for the dialog to be terminated
        self.dialog.cancel_token().cancelled().await;

        info!(dialog_id = %dialog_id, "Call ended");

        // Clean up
        handler_task.abort();

        Ok(())
    }
}
