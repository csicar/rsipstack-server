# SIP Echo Server

A SIP server built with Rust that accepts all incoming calls and echoes audio back to the caller. Useful for testing VoIP clients, network connectivity, and audio quality.

## Features

- **SIP Protocol Support**: Full SIP call handling via [rsipstack](https://crates.io/crates/rsipstack)
- **G.711 Codec Support**: PCMU (payload type 0) and PCMA (payload type 8)
- **Audio Echo**: Echoes received RTP audio back to the caller in real-time
- **Concurrent Calls**: Handles multiple simultaneous calls
- **NAT Traversal**: Optional external IP configuration for NAT environments

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      SIP Echo Server                        │
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
│                              │  EchoHandler  │              │
│                              └───────────────┘              │
└─────────────────────────────────────────────────────────────┘
```

### Components

| Component | Description |
|-----------|-------------|
| **SIP Endpoint** | Handles UDP transport and SIP message parsing |
| **Dialog Layer** | Manages SIP dialogs (call sessions) and state transitions |
| **Call Handler** | Processes INVITE requests, negotiates SDP, sets up media |
| **Media Session** | Manages RTP sockets and audio frame channels |
| **EchoHandler** | Receives audio frames and sends them back (echo effect) |

### Call Flow

```
Caller                          Echo Server
  │                                  │
  │  INVITE (SDP offer)              │
  │─────────────────────────────────▶│
  │                                  │ Parse SDP, allocate RTP port
  │              100 Trying          │
  │◀─────────────────────────────────│
  │              180 Ringing         │
  │◀─────────────────────────────────│
  │              200 OK (SDP answer) │
  │◀─────────────────────────────────│
  │  ACK                             │
  │─────────────────────────────────▶│
  │                                  │
  │   location RTP location─────────────────────▶│ Echo audio back
  │◀─────────────────────────────────│
  │                                  │
  │  BYE                             │
  │─────────────────────────────────▶│
  │              200 OK              │
  │◀─────────────────────────────────│
```

## Building

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release
```

## Usage

```bash
# Run with default settings (port 5060, RTP starting at 10000)
cargo run --release

# Custom SIP port
cargo run --release -- --port 5080

# With external IP for NAT traversal
cargo run --release -- --external-ip 203.0.113.1

# Full options
cargo run --release -- \
    --port 5060 \
    --bind 0.0.0.0 \
    --external-ip 203.0.113.1 \
    --rtp-start-port 20000 \
    --log-level debug
```

### Command-line Options

| Option | Default | Description |
|--------|---------|-------------|
| `--port` | 5060 | SIP listening port |
| `--bind` | auto | Bind address (defaults to first non-loopback interface) |
| `--external-ip` | none | External IP for NAT traversal |
| `--rtp-start-port` | 10000 | Starting port for RTP media (even number) |
| `--log-level` | info | Log level: trace, debug, info, warn, error |

## Testing

### With sipp

```bash
# Basic UAC test (single call)
sipp -sn uac <server-ip>:5060 -m 1

# Multiple calls
sipp -sn uac <server-ip>:5060 -m 10 -r 1 -l 1

# With longer call duration (5 seconds)
sipp -sn uac <server-ip>:5060 -m 1 -d 5000
```

### With a SIP softphone

1. Configure your softphone to register (optional) or direct call
2. Call `sip:echo@<server-ip>:5060`
3. Speak into the microphone - you should hear your voice echoed back

## Project Structure

```
rsipstack-server/
├── Cargo.toml
├── src/
│   ├── main.rs           # Entry point, CLI argument parsing
│   ├── server.rs         # SIP server setup and request routing
│   ├── call_handler.rs   # INVITE handling, media session setup
│   ├── media/
│   │   ├── mod.rs
│   │   ├── session.rs    # RTP socket management, audio channels
│   │   ├── rtp.rs        # RTP packet parsing and building
│   │   └── sdp.rs        # SDP offer/answer generation
│   └── audio/
│       ├── mod.rs
│       ├── handler.rs    # AudioHandler trait definition
│       └── echo.rs       # Echo implementation
└── test/
    └── uac_pcap.xml      # sipp test scenario
```

## Extending

The `AudioHandler` trait allows implementing custom audio processing:

```rust
#[async_trait]
pub trait AudioHandler: Send + Sync {
    async fn process(
        &self,
        audio_in: mpsc::UnboundedReceiver<AudioFrame>,
        audio_out: mpsc::UnboundedSender<AudioFrame>,
        cancel_token: CancellationToken,
    );
}
```

Example use cases:
- Audio recording
- Text-to-speech playback
- Audio mixing/conferencing
- Voice activity detection

## Dependencies

- [rsipstack](https://crates.io/crates/rsipstack) - SIP protocol stack
- [tokio](https://crates.io/crates/tokio) - Async runtime
- [rtp-rs](https://crates.io/crates/rtp-rs) - RTP packet handling
- [sdp-rs](https://crates.io/crates/sdp-rs) - SDP parsing

## License

MIT
