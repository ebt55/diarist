//! Real-device capture checks, gated on `MEET_ASSIST_AUDIO_TEST=1` (they need
//! VB-Audio Virtual Cable: a render endpoint whose audio comes out of the
//! "CABLE Output" capture endpoint). A 440 Hz tone is played into the cable
//! with `play_wav` while `start_capture` records "CABLE Output"; the packets
//! must carry the tone with QPC timestamps that advance at 16 kHz.
//! Override the endpoints with `MEET_ASSIST_CABLE_RENDER` / `MEET_ASSIST_CABLE_CAPTURE`.

#![cfg(windows)]

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use diarist_core::audio::capture::{
    find_device, list_devices, play_wav, start_capture, CaptureConfig, DeviceFlow, Packet,
    PacketData,
};
use diarist_core::types::Channel;

fn enabled() -> bool {
    std::env::var("MEET_ASSIST_AUDIO_TEST").is_ok_and(|v| v == "1")
}

/// The cable's render side: an explicit override, else any render endpoint
/// whose name mentions "VB-Audio Virtual Cable" and is not the 16-channel one.
pub fn cable_render() -> String {
    if let Ok(q) = std::env::var("MEET_ASSIST_CABLE_RENDER") {
        return q;
    }
    let devs = list_devices().unwrap();
    for q in [
        "CABLE Input",
        "Speakers (2- VB-Audio",
        "VB-Audio Virtual Cable",
    ] {
        if let Some(d) = find_device(&devs, DeviceFlow::Render, q) {
            if !d.name.contains("16 Ch") {
                return d.id;
            }
        }
    }
    panic!("no VB-Audio cable render endpoint");
}

fn tone(path: &std::path::Path, seconds: f32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(16_000.0 * seconds) as usize {
        let v = (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 12_000.0;
        w.write_sample(v as i16).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn cable_tone_is_captured() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    tone(&wav, 2.0);
    let devs = list_devices().unwrap();
    let capture_q =
        std::env::var("MEET_ASSIST_CABLE_CAPTURE").unwrap_or_else(|_| "CABLE Output".into());
    let mic = find_device(&devs, DeviceFlow::Capture, &capture_q).expect("CABLE Output");
    let render = cable_render();
    eprintln!("render {render} -> capture {}", mic.name);
    let (prod, mut cons) = rtrb::RingBuffer::<Packet>::new(4_096);
    let src = start_capture(CaptureConfig::new(Channel::Mic, Some(mic.id), true), prod).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = Arc::clone(&stop);
    let player = std::thread::spawn(move || play_wav(Some(&render), &wav, &s2));
    let start = Instant::now();
    let mut samples = 0usize;
    let mut loud = 0usize;
    let mut first_qpc = None;
    let mut last = (0i64, 0usize);
    let mut controls = 0;
    while start.elapsed() < Duration::from_millis(3_000) {
        while let Ok(p) = cons.pop() {
            match p {
                Packet::Audio {
                    qpc_100ns, data, ..
                } => {
                    let PacketData::Pcm16(s) = data else {
                        panic!("autoconvert gives PCM16")
                    };
                    first_qpc.get_or_insert(qpc_100ns);
                    last = (qpc_100ns, s.len());
                    samples += s.len();
                    loud += s.iter().filter(|v| v.unsigned_abs() > 3_000).count();
                }
                Packet::Control { .. } => controls += 1,
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    player.join().unwrap().unwrap();
    src.stop();
    let span = (last.0 - first_qpc.unwrap()) as f64 / 1e7 + last.1 as f64 / 16_000.0;
    eprintln!(
        "samples {samples} over {span:.3} s ({:.1} Hz), loud {loud}, controls {controls}",
        samples as f64 / span
    );
    assert!(controls >= 1, "the first open is reported");
    assert!(
        (samples as f64 / span - 16_000.0).abs() < 50.0,
        "16 kHz timeline"
    );
    assert!(loud > 16_000, "the tone arrived ({loud} loud samples)");
}
