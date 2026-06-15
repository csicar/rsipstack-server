# rsipstack-server

A generic VoIP server library built with Rust using [rsipstack](https://crates.io/crates/rsipstack). Define custom audio processing via an actor-style interface with channels.

## Features

- **SIP Protocol Support**: Full SIP call handling via rsipstack
- **Codec Support**: Opus, PCMU (G.711 μ-law), PCMA (G.711 A-law)
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
cargo run --example echo_server -- --port 5060 --first-rtp-port 10000 --last-rtcp-port 10099
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
