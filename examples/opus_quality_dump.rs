//! THROWAWAY diagnostic — renders real audio through our Opus encode/decode path at
//! several encoder configs so their quality can be A/B compared by ear.
//!
//! Usage:
//!   cargo run --release --features opus --example opus_quality_dump -- <in.s16> <out_dir>
//! where <in.s16> is raw 48 kHz mono little-endian s16 (as fed to the RTP send task).
//! Writes one raw .s16 per config into <out_dir>; convert to wav with ffmpeg.

use std::fs;
use std::path::Path;

use opus::{Application, Bitrate, Channels, Decoder, Encoder, Signal};

const FRAME: usize = 960; // 20 ms @ 48 kHz

/// Encoder config toggles (mirrors benches/rtp_codec.rs).
struct Cfg {
    complexity: Option<i32>,
    voice: bool,
    bitrate_24k: bool,
}

fn make_encoder(cfg: &Cfg) -> Encoder {
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

/// Encode then decode every frame, returning the reconstructed samples.
fn roundtrip(samples: &[i16], cfg: &Cfg) -> Vec<i16> {
    let mut enc = make_encoder(cfg);
    let mut dec = Decoder::new(48000, Channels::Mono).unwrap();
    let mut payload = vec![0u8; 4000];
    let mut frame_out = vec![0i16; FRAME];
    let mut out = Vec::with_capacity(samples.len());
    for chunk in samples.chunks_exact(FRAME) {
        let len = enc.encode(chunk, &mut payload).unwrap();
        let n = dec.decode(&payload[..len], &mut frame_out, false).unwrap();
        out.extend_from_slice(&frame_out[..n]);
    }
    out
}

fn write_s16(dir: &Path, name: &str, samples: &[i16]) {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let path = dir.join(format!("{name}.s16"));
    fs::write(&path, bytes).unwrap();
    println!("wrote {}", path.display());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let in_path = args.next().expect("arg1: input .s16 (48k mono le)");
    let out_dir = args.next().expect("arg2: output dir");
    let dir = Path::new(&out_dir);
    fs::create_dir_all(dir).unwrap();

    let bytes = fs::read(&in_path).unwrap();
    let samples: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let usable = (samples.len() / FRAME) * FRAME;
    let samples = &samples[..usable];
    println!(
        "input: {} samples ({:.1}s), {} frames",
        samples.len(),
        samples.len() as f32 / 48000.0,
        samples.len() / FRAME
    );

    // 00: reference (no codec).
    write_s16(dir, "00_reference", samples);

    let configs: [(&str, Cfg); 3] = [
        // Pre-change production encoder (no CTLs).
        ("01_default", Cfg { complexity: None, voice: false, bitrate_24k: false }),
        // Shipped config.
        ("02_c5_voice_24k", Cfg { complexity: Some(5), voice: true, bitrate_24k: true }),
        // Capacity-emergency lever under test (only complexity differs from 02).
        ("03_c0_voice_24k", Cfg { complexity: Some(0), voice: true, bitrate_24k: true }),
    ];
    for (name, cfg) in &configs {
        let decoded = roundtrip(samples, cfg);
        write_s16(dir, name, &decoded);
    }
    println!("done");
}
