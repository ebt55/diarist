//! `--demo` only: the screenshot setup on `MemoryStore::demo()`. A generated
//! stereo WAV becomes the playback track of the large seeded meetings, and
//! the GPU line in Settings is a fixed RTX 3080. Nothing here runs without
//! `--demo`.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::thread;

use diarist_core::store::{NewAudioTrack, Store};
use diarist_core::types::{AudioFormat, MeetingId, TrackChannel};
use diarist_core::views::DeviceInfo;

/// Relative to the data root, like every stored track.
const DEMO_TRACK: &str = "audio/demo/playback.wav";
const DEMO_SECONDS: u32 = 60;

/// A fixed GPU for the Settings screen: an RTX 3080 with a browser and a
/// meeting app already on it.
pub fn gpu() -> (Option<u32>, Option<DeviceInfo>) {
    let free = 4_600;
    (
        Some(free),
        Some(DeviceInfo {
            name: "NVIDIA GeForce RTX 3080 (demo)".into(),
            vram_total_mb: Some(10_240),
            vram_free_mb: Some(free),
            driver_version: Some("610.88".into()),
        }),
    )
}

/// Registers the demo WAV as the playback track of the seeded meetings with
/// 2,000 segments and writes the file once (in the background).
pub fn attach_demo_audio(store: &dyn Store, data_root: &Path) {
    let file = data_root.join(DEMO_TRACK);
    let size = 44 + i64::from(DEMO_SECONDS) * 16_000 * 4;
    let large: Vec<MeetingId> = store
        .list_meetings()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            store
                .get_segments(m.id, 1_999, 1)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        })
        .map(|m| m.id)
        .collect();
    for meeting_id in &large {
        let track = NewAudioTrack {
            meeting_id: *meeting_id,
            channel: TrackChannel::Playback,
            path: DEMO_TRACK.into(),
            format: AudioFormat::Wav,
            sample_rate: 16_000,
            channels: 2,
            duration_ms: Some(i64::from(DEMO_SECONDS) * 1000),
            size_bytes: Some(size),
            channel_frames: None,
        };
        if let Err(e) = store.add_audio_track(track) {
            log::warn!("demo audio for meeting {meeting_id}: {e}");
        }
    }
    let fresh = std::fs::metadata(&file)
        .map(|m| m.len() == size as u64)
        .unwrap_or(false);
    if !fresh {
        thread::spawn(move || match write_demo_wav(&file, DEMO_SECONDS) {
            Ok(()) => log::info!("wrote demo playback track {}", file.display()),
            Err(e) => log::error!("demo playback track {}: {e}", file.display()),
        });
    }
}

/// `seconds` of 16 kHz stereo PCM16: 440 Hz bursts on the left (mic) and
/// 660 Hz on the right (loopback), taking turns like a conversation.
fn write_demo_wav(path: &Path, seconds: u32) -> io::Result<()> {
    const RATE: u32 = 16_000;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("wav.part");
    {
        let frames = RATE * seconds;
        let data_len = frames * 4;
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(b"RIFF")?;
        w.write_all(&(36 + data_len).to_le_bytes())?;
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&1u16.to_le_bytes())?; // PCM
        w.write_all(&2u16.to_le_bytes())?; // stereo
        w.write_all(&RATE.to_le_bytes())?;
        w.write_all(&(RATE * 4).to_le_bytes())?; // byte rate
        w.write_all(&4u16.to_le_bytes())?; // block align
        w.write_all(&16u16.to_le_bytes())?; // bits
        w.write_all(b"data")?;
        w.write_all(&data_len.to_le_bytes())?;
        let tau = std::f32::consts::TAU;
        for i in 0..frames {
            let t = i as f32 / RATE as f32;
            let phase = t % 4.0;
            let syllables = 0.55 + 0.45 * (tau * 3.5 * t).sin().abs();
            let envelope = |from: f32, to: f32| {
                if phase < from || phase > to {
                    0.0
                } else {
                    ((phase - from).min(to - phase) / 0.02).min(1.0) * syllables
                }
            };
            let left = 0.32 * envelope(0.2, 1.8) * (tau * 440.0 * t).sin();
            let right = 0.32 * envelope(2.2, 3.8) * (tau * 660.0 * t).sin();
            for s in [left, right] {
                w.write_all(&((s * 32_767.0) as i16).to_le_bytes())?;
            }
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, path)
}
