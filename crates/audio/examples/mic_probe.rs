//! Diagnose microphone capture: record N seconds from the default mic through `AudioCapture`
//! and write the raw device stream and the aligned 48 kHz stream as WAV files.
//! Usage: cargo run --release -p screencraft-audio --example mic_probe -- <out_dir> [seconds]

use screencraft_audio::{AudioCapture, CaptureConfig, Clock, StreamAligner};
use std::io::Write;

fn wav(path: &std::path::Path, rate: u32, ch: u16, data: &[f32]) {
    let mut f = std::fs::File::create(path).unwrap();
    let bytes = (data.len() * 2) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&ch.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * ch as u32 * 2).to_le_bytes()).unwrap();
    f.write_all(&(ch * 2).to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&bytes.to_le_bytes()).unwrap();
    for s in data {
        f.write_all(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).cloned().unwrap_or(".".into()));
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let clock = Clock::start();
    let (tx, rx) = std::sync::mpsc::channel();
    let cfg = CaptureConfig { use_microphone: true, ..Default::default() };
    let cap = AudioCapture::start(&cfg, &clock, tx).unwrap();
    let (rate, ch) = cap.microphone_format.unwrap();
    println!("mic format: {rate} Hz, {ch} ch");
    let origin = clock.now_ns();
    let mut al = StreamAligner::new(origin, rate, ch);
    let (mut raw, mut out, mut blocks) = (Vec::new(), Vec::new(), 0);
    let mut sizes = std::collections::BTreeMap::new();
    while (clock.now_ns() - origin) as f64 / 1e9 < secs {
        if let Ok(b) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
            blocks += 1;
            *sizes.entry(b.data.len() / ch as usize).or_insert(0) += 1;
            raw.extend_from_slice(&b.data);
            out.extend(al.push(b.pts_ns, &b.data));
        }
    }
    cap.stop();
    println!("blocks {blocks}, frames per block {sizes:?}, stats {:?}", al.stats);
    wav(&dir.join("probe_raw.wav"), rate, ch, &raw);
    wav(&dir.join("probe_aligned.wav"), 48_000, 2, &out);
}
