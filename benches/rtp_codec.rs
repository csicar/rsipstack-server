//! Microbenchmarks for the CPU-bound body of the RTP send / receive tasks.
//!
//! These measure the per-frame (20 ms) CPU work that `MediaSession::rtp_send_task`
//! and `rtp_receive_task` do inside a single `poll`, so the numbers are directly
//! comparable to the per-task **busy** time reported by tokio-console
//! (busy = time spent inside poll, i.e. CPU — not wall clock).
//!
//!   send    = opus.encode  ->  build RTP packet  ->  UDP sendmsg (connected socket)
//!   receive = parse RTP     ->  opus.decode
//!
//! What is intentionally excluded, and why it is small:
//!   * the receive-side `recvmsg` syscall (~a few µs, comparable to the send-side
//!     syscall that IS included),
//!   * the metrics histogram record + `select!` / channel machinery (sub-µs).
//! The dominant term on the send side is the Opus encode, which is exactly what the
//! `complexity` / `signal` / `bitrate` knobs move — so this bench isolates that.
//!
//! The `default (unset)` send config reconstructs the pre-change production encoder
//! (`Encoder::new(48k, Mono, Voip)` with no CTLs), so its row should land near the
//! ~240 µs/frame the send task showed in tokio-console. Absolute numbers are
//! machine-dependent: compare rows against each other and against that baseline row.
//!
//! Requires the default `opus` feature. Run with: `cargo bench --bench rtp_codec`.
//!
//! For trustworthy rankings, point it at REAL call audio (Opus mode decisions are
//! signal-adaptive, so synthetic input can invert the results):
//!   ffmpeg -i call.mp3 -ac 1 -ar 48000 -f s16le -acodec pcm_s16le real.s16
//!   RTP_BENCH_AUDIO=real.s16 cargo bench --bench rtp_codec

use std::f32::consts::PI;
use std::hint::black_box;
use std::net::UdpSocket;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use opus::{Application, Bitrate, Channels, Decoder, Encoder, Signal};
use rtp_rs::{RtpPacketBuilder, RtpReader};

/// 20 ms at 48 kHz mono.
const FRAME: usize = 960;
/// A 1 second bank of distinct frames, cycled through so the stateful Opus encoder
/// sees varying input (as in a real call) rather than adapting to one repeated frame.
const BANK_FRAMES: usize = 50;

/// Deterministic speech-like audio: a handful of voiced formant-ish tones plus a
/// little noise, under a slow amplitude envelope. Not silence (encodes far too
/// cheaply) and not a pure tone (unrealistically easy) — aims to cost roughly what
/// real speech costs libopus. `n` seeds phase/envelope so successive frames differ.
fn speech_like_frame(n: usize) -> Vec<i16> {
    let mut lcg: u32 = 0x1234_5678u32.wrapping_add(n as u32).wrapping_mul(2654435761);
    let base = n as f32 * FRAME as f32 / 48000.0;
    (0..FRAME)
        .map(|i| {
            let t = base + i as f32 / 48000.0;
            let env = 0.5 + 0.5 * (2.0 * PI * 3.0 * t).sin();
            let tone = (2.0 * PI * 150.0 * t).sin()
                + 0.7 * (2.0 * PI * 450.0 * t).sin()
                + 0.5 * (2.0 * PI * 900.0 * t).sin()
                + 0.3 * (2.0 * PI * 1800.0 * t).sin();
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((lcg >> 16) as i16) as f32 / 32768.0; // ~[-1, 1)
            let s = env * (0.8 * tone + 0.2 * noise);
            (s * 6000.0).clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// Build the frame bank the benches cycle through.
///
/// If the `RTP_BENCH_AUDIO` env var points at a raw 48 kHz **mono little-endian
/// s16** file, the bank is that real call audio, split into consecutive
/// 960-sample (20 ms) frames **in order** — so the stateful Opus encoder sees the
/// true temporal evolution of speech (pauses, onsets, level changes) that drives
/// its adaptive mode/complexity decisions. This is what makes the config-sweep
/// rankings trustworthy; synthetic constant-energy tones can invert them.
///
/// When the var is unset it falls back to the synthetic `speech_like_frame` bank
/// so the bench still runs standalone.
///
/// Regenerate the real-audio file from any recording with:
///   ffmpeg -i input.mp3 -ac 1 -ar 48000 -f s16le -acodec pcm_s16le out.s16
fn frame_bank() -> Vec<Vec<i16>> {
    if let Ok(path) = std::env::var("RTP_BENCH_AUDIO") {
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("RTP_BENCH_AUDIO={path:?}: {e}"));
        let bank: Vec<Vec<i16>> = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<i16>>()
            .chunks_exact(FRAME)
            .map(<[i16]>::to_vec)
            .collect();
        assert!(
            !bank.is_empty(),
            "RTP_BENCH_AUDIO file holds < one 960-sample frame"
        );
        eprintln!(
            "rtp_codec bench: REAL audio {path:?} — {} frames ({:.1}s)",
            bank.len(),
            (bank.len() * FRAME) as f32 / 48000.0
        );
        return bank;
    }
    eprintln!("rtp_codec bench: SYNTHETIC audio — {BANK_FRAMES} frames (set RTP_BENCH_AUDIO for real)");
    (0..BANK_FRAMES).map(speech_like_frame).collect()
}

/// A named encoder configuration. Each knob is applied only if set, so knobs can be
/// isolated. `Cfg::default()` (all `None`/`false`) reproduces the pre-change
/// production encoder (`Encoder::new` with no CTLs).
#[derive(Clone, Copy, Default)]
struct Cfg {
    complexity: Option<i32>,
    voice: bool,
    bitrate_24k: bool,
}

fn make_encoder(cfg: Cfg) -> Encoder {
    let mut enc = Encoder::new(48000, Channels::Mono, Application::Voip).unwrap();
    if let Some(c) = cfg.complexity {
        enc.set_complexity(c).unwrap();
    }
    if cfg.voice {
        enc.set_signal(Signal::Voice).unwrap();
    }
    if cfg.bitrate_24k {
        enc.set_bitrate(Bitrate::Bits(24000)).unwrap();
    }
    enc
}

fn bench_send(c: &mut Criterion) {
    let bank = frame_bank();

    // Real UDP send target: a bound loopback sink we never read from. On Linux a
    // full peer buffer just drops the datagram; sendmsg still returns immediately,
    // so this captures the syscall cost without blocking.
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    tx.connect(sink.local_addr().unwrap()).unwrap();

    let configs: [(&str, Cfg); 9] = [
        // Baseline: exactly the pre-change production encoder.
        ("00_before_unset", Cfg::default()),
        // Isolated single knobs (everything else default/auto).
        ("01_complexity5_only", Cfg { complexity: Some(5), ..Cfg::default() }),
        ("02_complexity0_only", Cfg { complexity: Some(0), ..Cfg::default() }),
        ("03_voice_only", Cfg { voice: true, ..Cfg::default() }),
        ("04_bitrate24k_only", Cfg { bitrate_24k: true, ..Cfg::default() }),
        // The shipped "after" and its neighbours (all three knobs together).
        ("05_c10_voice_24k", Cfg { complexity: Some(10), voice: true, bitrate_24k: true }),
        ("06_c5_voice_24k_AFTER", Cfg { complexity: Some(5), voice: true, bitrate_24k: true }),
        ("07_c3_voice_24k", Cfg { complexity: Some(3), voice: true, bitrate_24k: true }),
        ("08_c0_voice_24k", Cfg { complexity: Some(0), voice: true, bitrate_24k: true }),
    ];

    let mut group = c.benchmark_group("rtp_send_frame");
    for (name, cfg) in configs {
        let mut enc = make_encoder(cfg);
        let mut payload = vec![0u8; 4000];
        let mut packet = vec![0u8; 4000];
        let mut idx = 0usize;
        let mut seq: u16 = 0;
        let mut ts: u32 = 0;

        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                let frame = &bank[idx];
                idx = (idx + 1) % bank.len();

                // encode
                let len = enc.encode(black_box(frame), &mut payload).unwrap();

                // build RTP packet (mirrors media::rtp::build_rtp_packet)
                let builder = RtpPacketBuilder::new()
                    .payload_type(96)
                    .ssrc(0xDEAD_BEEF)
                    .sequence(seq.into())
                    .timestamp(ts)
                    .payload(&payload[..len]);
                let n = builder.target_length();
                packet.resize(n, 0);
                builder.build_into(&mut packet).unwrap();

                // sendmsg on the connected socket
                let _ = tx.send(&packet);

                seq = seq.wrapping_add(1);
                ts = ts.wrapping_add(FRAME as u32);
                black_box(len)
            })
        });
    }
    group.finish();
}

fn bench_receive(c: &mut Criterion) {
    // Stage a bank of RTP packets as they would arrive from a peer (encoded with the
    // tuned encoder; decode cost is set by the remote's stream, not our settings).
    let bank = frame_bank();
    let mut enc = make_encoder(Cfg { complexity: Some(5), voice: true, bitrate_24k: true });
    let packets: Vec<Vec<u8>> = bank
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            let mut payload = vec![0u8; 4000];
            let len = enc.encode(frame, &mut payload).unwrap();
            RtpPacketBuilder::new()
                .payload_type(96)
                .ssrc(0xDEAD_BEEF)
                .sequence((i as u16).into())
                .timestamp((i as u32).wrapping_mul(FRAME as u32))
                .payload(&payload[..len])
                .build()
                .unwrap()
        })
        .collect();

    let mut dec = Decoder::new(48000, Channels::Mono).unwrap();
    let mut out = vec![0i16; FRAME];
    let mut idx = 0usize;

    c.bench_function("rtp_receive_frame(parse+decode)", |b| {
        b.iter(|| {
            let pkt = &packets[idx];
            idx = (idx + 1) % packets.len();
            let reader = RtpReader::new(black_box(pkt)).unwrap();
            let n = dec.decode(reader.payload(), &mut out, false).unwrap();
            black_box(n)
        })
    });
}

criterion_group!(benches, bench_send, bench_receive);
criterion_main!(benches);
