//! SIP Server setup and request routing

use crate::audio::handler::AudioHandler;
use crate::call_handler::CallHandler;
use rsipstack::dialog::dialog::{Dialog, DialogState, DialogStateReceiver, DialogStateSender};
use rsipstack::dialog::dialog_layer::DialogLayer;
use rsipstack::sip as rsip;
use rsipstack::sip::prelude::HeadersExt;
use rsipstack::transport::udp::UdpConnection;
use rsipstack::transport::TransportLayer;
use rsipstack::transaction::TransactionReceiver;
use rsipstack::{EndpointBuilder, Error, Result};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// SIP listening port
    pub port: u16,
    /// Bind address (defaults to first non-loopback interface)
    pub bind_addr: Option<IpAddr>,
    /// External IP address for NAT traversal
    pub external_ip: Option<IpAddr>,
    /// Starting port for RTP media (even number)
    pub rtp_start_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 5060,
            bind_addr: None,
            external_ip: None,
            rtp_start_port: 10000,
        }
    }
}

/// Shared state for the SIP server
pub struct ServerState {
    pub local_ip: IpAddr,
    pub external_ip: Option<IpAddr>,
    pub rtp_port_counter: AtomicU16,
    pub cancel_token: CancellationToken,
}

impl ServerState {
    /// Allocate the next available RTP port (returns even port number)
    pub fn allocate_rtp_port(&self) -> u16 {
        self.rtp_port_counter.fetch_add(2, Ordering::Relaxed)
    }

    /// Get the IP address to use for media (external IP if set, otherwise local)
    pub fn media_ip(&self) -> IpAddr {
        self.external_ip.unwrap_or(self.local_ip)
    }
}

/// Factory trait for creating audio handlers
///
/// Implement this trait or use a closure with `SipServer::new()`.
pub trait AudioHandlerFactory: Send + Sync + 'static {
    /// The audio handler type this factory creates
    type Handler: AudioHandler + 'static;

    /// Create a new audio handler for a call
    fn create(&self) -> Self::Handler;
}

/// Blanket implementation for closures
impl<F, H> AudioHandlerFactory for F
where
    F: Fn() -> H + Send + Sync + 'static,
    H: AudioHandler + 'static,
{
    type Handler = H;

    fn create(&self) -> H {
        self()
    }
}

/// SIP Server
pub struct SipServer<F: AudioHandlerFactory> {
    cancel_token: CancellationToken,
    transport_layer: TransportLayer,
    state: Arc<ServerState>,
    local_addr: SocketAddr,
    handler_factory: Arc<F>,
}

const SIP_USER_AGENT: &str = concat!("rsipstack-server/", env!("CARGO_PKG_VERSION"));

impl<F: AudioHandlerFactory> SipServer<F> {
    /// Create a new SIP server with the given configuration and audio handler factory.
    ///
    /// # Arguments
    ///
    /// * `config` - Server configuration
    /// * `handler_factory` - Factory that creates audio handlers for each call.
    ///   Can be a closure like `|| MyHandler::new()`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use rsipstack_server::{SipServer, ServerConfig, SipHeaders};
    /// # struct MyHandler;
    /// # impl rsipstack_server::AudioHandler for MyHandler {
    /// #     fn process<'a, 'b>(
    /// #         &'a self,
    /// #         _: tokio::sync::mpsc::UnboundedReceiver<rsipstack_server::AudioFrame>,
    /// #         _: tokio::sync::mpsc::UnboundedSender<rsipstack_server::AudioFrame>,
    /// #         _: tokio_util::sync::CancellationToken,
    /// #         _: SipHeaders,
    /// #     ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'b>>
    /// #     where 'a: 'b {
    /// #         Box::pin(async {})
    /// #     }
    /// # }
    /// # async fn example() -> anyhow::Result<()> {
    /// let server = SipServer::new(ServerConfig::default(), || MyHandler).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(config: ServerConfig, handler_factory: F) -> Result<Self> {
        let cancel_token = CancellationToken::new();

        // Get local IP address
        let local_ip = match config.bind_addr {
            Some(addr) => addr,
            None => get_first_non_loopback_interface()?,
        };

        let local_addr = SocketAddr::new(local_ip, config.port);
        let external_addr = config.external_ip.map(|ip| SocketAddr::new(ip, config.port));

        info!("Binding to {}", local_addr);
        if let Some(ext) = external_addr {
            info!("External address: {}", ext);
        }

        // Create transport layer
        let transport_layer = TransportLayer::new(cancel_token.clone());

        // Create UDP connection
        let udp_conn = UdpConnection::create_connection(
            local_addr,
            external_addr,
            Some(cancel_token.child_token()),
        )
        .await?;

        transport_layer.add_transport(udp_conn.into());

        let state = Arc::new(ServerState {
            local_ip,
            external_ip: config.external_ip,
            rtp_port_counter: AtomicU16::new(config.rtp_start_port),
            cancel_token: cancel_token.clone(),
        });

        Ok(Self {
            cancel_token,
            transport_layer,
            state,
            local_addr,
            handler_factory: Arc::new(handler_factory),
        })
    }

    /// Run the SIP server
    ///
    /// This will block until the server is shut down (via Ctrl+C or cancellation token).
    pub async fn run(self) -> Result<()> {
        // Extract all needed values from self before consuming transport_layer
        let cancel_token = self.cancel_token;
        let state = self.state;
        let local_addr = self.local_addr;
        let transport_layer = self.transport_layer;
        let handler_factory = self.handler_factory;

        let endpoint = EndpointBuilder::new()
            .with_user_agent(SIP_USER_AGENT)
            .with_cancel_token(cancel_token.clone())
            .with_transport_layer(transport_layer)
            .build();

        let dialog_layer = Arc::new(DialogLayer::new(endpoint.inner.clone()));
        let (state_sender, state_receiver) = dialog_layer.new_dialog_state_channel();

        let incoming = endpoint.incoming_transactions()?;

        // Build contact URI
        let contact = rsip::Uri {
            scheme: Some(rsip::Scheme::Sip),
            auth: Some(rsip::Auth {
                user: "server".to_string(),
                password: None,
            }),
            host_with_port: local_addr.into(),
            params: vec![],
            headers: vec![],
        };

        info!("SIP Server listening on {}", local_addr);
        info!("Contact URI: {}", contact);

        select! {
            _ = endpoint.serve() => {
                info!("Endpoint finished");
            }
            r = Self::process_incoming_requests(
                dialog_layer.clone(),
                incoming,
                state_sender,
                contact
            ) => {
                match r {
                    Ok(_) => info!("Request processing finished"),
                    Err(e) => error!("Request processing error: {:?}", e),
                }
            }
            r = Self::process_dialog_states(state.clone(), dialog_layer.clone(), state_receiver, handler_factory) => {
                match r {
                    Ok(_) => info!("Dialog state processing finished"),
                    Err(e) => error!("Dialog state processing error: {:?}", e),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received Ctrl+C, shutting down...");
                cancel_token.cancel();
            }
        }

        Ok(())
    }

    /// Process incoming SIP requests
    async fn process_incoming_requests(
        dialog_layer: Arc<DialogLayer>,
        mut incoming: TransactionReceiver,
        state_sender: DialogStateSender,
        contact: rsip::Uri,
    ) -> Result<()> {
        while let Some(mut tx) = incoming.recv().await {
            debug!(
                method = %tx.original.method,
                uri = %tx.original.uri,
                "Received transaction"
            );

            // Check if this is an in-dialog request (has to-tag)
            if let Ok(to_header) = tx.original.to_header() {
                if let Ok(tag) = to_header.tag() {
                    if tag.as_ref().is_some() {
                        // In-dialog request - route to existing dialog
                        match dialog_layer.match_dialog(&tx) {
                            Some(mut dialog) => {
                                tokio::spawn(async move {
                                    if let Err(e) = dialog.handle(&mut tx).await {
                                        warn!("Dialog handle error: {:?}", e);
                                    }
                                });
                                continue;
                            }
                            None => {
                                info!("Dialog not found for in-dialog request");
                                tx.reply(rsip::StatusCode::CallTransactionDoesNotExist)
                                    .await?;
                                continue;
                            }
                        }
                    }
                }
            }

            // Out-of-dialog request
            match tx.original.method {
                rsip::Method::Invite => {
                    // Create server dialog for new INVITE
                    let mut dialog = match dialog_layer.get_or_create_server_invite(
                        &tx,
                        state_sender.clone(),
                        None,
                        Some(contact.clone()),
                    ) {
                        Ok(d) => d,
                        Err(e) => {
                            warn!("Failed to create dialog: {:?}", e);
                            tx.reply(rsip::StatusCode::ServerInternalError).await?;
                            continue;
                        }
                    };

                    tokio::spawn(async move {
                        if let Err(e) = dialog.handle(&mut tx).await {
                            warn!("Dialog handle error: {:?}", e);
                        }
                    });
                }
                rsip::Method::Options => {
                    // Reply to OPTIONS with OK (basic keep-alive support)
                    tx.reply(rsip::StatusCode::OK).await?;
                }
                rsip::Method::Register => {
                    // We don't support registration, reject it
                    tx.reply(rsip::StatusCode::MethodNotAllowed).await?;
                }
                _ => {
                    // Reject other methods
                    tx.reply(rsip::StatusCode::MethodNotAllowed).await?;
                }
            }
        }

        Ok(())
    }

    /// Process dialog state changes
    async fn process_dialog_states(
        server_state: Arc<ServerState>,
        dialog_layer: Arc<DialogLayer>,
        mut state_receiver: DialogStateReceiver,
        handler_factory: Arc<F>,
    ) -> Result<()> {
        while let Some(state) = state_receiver.recv().await {
            match state {
                DialogState::Calling(id) => {
                    info!(dialog_id = %id, "New incoming call");

                    let dialog = match dialog_layer.get_dialog(&id) {
                        Some(d) => d,
                        None => {
                            warn!(dialog_id = %id, "Dialog not found");
                            continue;
                        }
                    };

                    match dialog {
                        Dialog::ServerInvite(server_dialog) => {
                            let state = server_state.clone();
                            let factory = handler_factory.clone();
                            tokio::spawn(async move {
                                let audio_handler = factory.create();
                                let handler = CallHandler::new(state, server_dialog, audio_handler);
                                if let Err(e) = handler.handle_call().await {
                                    error!("Call handler error: {:?}", e);
                                }
                            });
                        }
                        _ => {
                            warn!(dialog_id = %id, "Unexpected dialog type");
                        }
                    }
                }
                DialogState::Confirmed(id, _) => {
                    info!(dialog_id = %id, "Call confirmed");
                }
                DialogState::Terminated(id, reason) => {
                    info!(dialog_id = %id, reason = ?reason, "Call terminated");
                    dialog_layer.remove_dialog(&id);
                }
                DialogState::Early(id, _) => {
                    debug!(dialog_id = %id, "Early dialog state");
                }
                _ => {
                    debug!(state = %state, "Dialog state change");
                }
            }
        }

        Ok(())
    }
}

/// Get the first non-loopback network interface IP address
fn get_first_non_loopback_interface() -> Result<IpAddr> {
    for iface in get_if_addrs::get_if_addrs()? {
        if !iface.is_loopback() {
            if let get_if_addrs::IfAddr::V4(ref addr) = iface.addr {
                return Ok(IpAddr::V4(addr.ip));
            }
        }
    }
    Err(Error::Error("No IPv4 interface found".to_string()))
}
