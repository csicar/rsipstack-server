# rsipstack-server

A generic VoIP server library built with Rust using [rsipstack](https://crates.io/crates/rsipstack). Define custom audio processing via an actor-style interface with channels.

## Features

- **SIP Protocol Support**: Full SIP call handling via rsipstack
- **Codec Support**: Opus, PCMU (G.711 μ-law), PCMA (G.711 A-law), G.722 (opt-in via the `g722` feature)
- **Actor-Style Audio Interface**: Receive/send audio via tokio channels
- **Pluggable Audio Handlers**: Implement the `AudioHandler` trait for custom processing
- **Concurrent Calls**: Handles multiple simultaneous calls
- **NAT Traversal**: Optional external IP configuration

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                       SIP Server                            │
├─────────────────────────────────────────────────────────────┤
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐   │
│  │ SIP Endpoint │───▶│ Dialog Layer │───▶│ Call Handler │   │
│  │  (rsipstack) │    │  (rsipstack) │    │              │   │
│  └──────────────┘    └──────────────┘    └───────┬──────┘   │
│                                                  │          │
│                                          ┌───────▼───────┐  │
│                                          │ Media Session │  │
│                                          │   (per call)  │  │
│                                          └───────┬───────┘  │
│                                                  │          │
│                           ┌──────────────────────┼──────────┤
│                           │                      │          │
│                     ┌─────▼─────┐          ┌─────▼─────┐    │
│                     │ audio_in  │          │ audio_out │    │
│                     │ (channel) │          │ (channel) │    │
│                     └─────┬─────┘          └─────▲─────┘    │
│                           │                      │          │
│                           └──────────┬───────────┘          │
│                                      │                      │
│                              ┌───────▼───────┐              │
│                              │ AudioHandler  │              │
│                              │ (your impl)   │              │
│                              └───────────────┘              │
└─────────────────────────────────────────────────────────────┘
```

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
rsipstack-server = "0.1"
```

### Basic Example

```rust
use rsipstack_server::{
    async_trait, mpsc, AudioFrame, AudioHandler,
    CancellationToken, ServerConfig, SipHeaders, SipServer
};

// Define your audio handler
struct EchoHandler;

#[async_trait]
impl AudioHandler for EchoHandler {
    async fn process(
        &self,
        mut audio_in: mpsc::UnboundedReceiver<AudioFrame>,
        audio_out: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
        _headers: SipHeaders,
    ) {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                frame = audio_in.recv() => {
                    match frame {
                        Some(f) => { let _ = audio_out.send(f); }
                        None => break,
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ServerConfig {
        port: 5060,
        min_port: 10000,
        max_port: 10099,
        ..Default::default()
    };

    // Pass a factory closure that creates handlers for each call
    let server = SipServer::new(config, || EchoHandler).await?;
    let cancel_token = server.cancel_token.clone();
    let drain_token = server.drain_token.clone();

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("Ctrl-C received. Cancelling ...");
        cancel_token.cancel();
    });

    let mut sigterm_stream = signal(SignalKind::terminate())?;
    tokio::spawn(async move {
        sigterm_stream.recv().await;
        info!("Received SIGTERM. Draining ...");
        drain_token.start_drain();
    });
    server.run().await?;
    Ok(())
}
```

### AudioHandler Trait

The `AudioHandler` trait is the core interface for custom audio processing:

```rust
#[async_trait]
pub trait AudioHandler: Send + Sync {
    async fn process(
        &self,
        audio_in: mpsc::UnboundedReceiver<AudioFrame>,
        audio_out: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
        headers: SipHeaders,
    );
}
```

- `audio_in`: Receives `AudioFrame`s from the remote peer
- `audio_out`: Send `AudioFrame`s to the remote peer
- `cancel_token`: Signals when the call ends
- `headers`: SIP headers from the INVITE request

### AudioFrame

```rust
pub struct AudioFrame {
    /// Decoded PCM samples at 48kHz, 960 samples per frame (20ms)
    pub samples: Vec<i16>,
}
```

The library handles all RTP details (timestamps, sequence numbers, SSRC) internally.
You only work with decoded PCM audio samples.

## Examples

### Echo Server

A complete echo server example is included:

```bash
cargo run --example echo_server -- --port 5060 --min-port 10000 --max-port 10099
```

### Testing with sipp

```bash
sipp -sn uac <server-ip>:5060 -m 1
```

### Testing with a SIP softphone

1. Configure your softphone for direct calling (no registration)
2. Call `sip:server@<server-ip>:5060`
3. The server will process audio according to your handler

## Configuration

| Option | Default | Description |
|--------|---------|-------------|
| `port` | 5060 | SIP listening port |
| `bind_addr` | auto | Bind address (defaults to first non-loopback interface) |
| `external_ip` | none | External IP for NAT traversal |
| `min_port` | 10000 | Starting RTP port (even number) |
| `max_port` | 10099 | Last RTCP port (odd number) |


## Project Structure

```
rsipstack-server/
├── src/
│   ├── lib.rs            # Library entry point, public API
│   ├── server.rs         # SIP server setup and request routing
│   ├── call_handler.rs   # INVITE handling, media session setup
│   ├── media/
│   │   ├── session.rs    # RTP socket management, audio channels
│   │   ├── rtp.rs        # RTP packet parsing and building
│   │   └── sdp.rs        # SDP offer/answer generation
│   └── audio/
│       └── handler.rs    # AudioHandler trait definition
└── examples/
    └── echo_server.rs    # Echo server example
```

## Use Cases

- Audio echo/loopback testing
- Audio recording
- Text-to-speech playback
- Audio mixing/conferencing
- Voice activity detection
- AI voice assistants

## License

MIT

## Metrics

The following metrics are exported using the [`metrics`](https://crates.io/crates/metrics) facade.
To collect them, register a backend such as [`metrics-exporter-prometheus`](https://crates.io/crates/metrics-exporter-prometheus) in your application, before constructing the `SipServer`. All counters below are pre-registered at zero during `SipServer::new`, so they appear in scrapes before the events they track have occurred.

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rsipstack_server.calls.accepted_total` | counter | — | Total calls successfully accepted |
| `rsipstack_server.calls.rejected_total` | counter | `reason`: `SdpOfferInvalid`, `RtpPortPoolExhausted`, `UdpConnectFailed` | Calls rejected before being accepted |
| `rsipstack_server.calls.active` | gauge | — | Currently active calls |
| `rsipstack_server.calls.terminated_total` | counter | `reason`: variant name of [`TerminatedReason`](https://docs.rs/rsipstack/latest/rsipstack/dialog/dialog/enum.TerminatedReason.html) (e.g. `Timeout`, `ProxyError`) — status codes carried by `ProxyError`/`UacOther`/`UasOther` are not included in the label | Calls terminated after being accepted |
| `rsipstack_server.calls.dialog_not_found_total` | counter | — | Requests received for unknown dialogs |
| `rsipstack_server.ports.capacity` | gauge | — | Upper bound of allocatable RTP/RTCP port pairs. OS may have some ports already bound. |
| `rsipstack_server.ports.allocation_attempts` | histogram | — | Number of attempts before a free port pair was found |
| `rsipstack_server.ports.allocation_failures` | counter | — | Port allocation failures (pool exhausted) |
| `rsipstack_server.drain_active` | gauge | — | 1 when the server is in drain mode, 0 otherwise |