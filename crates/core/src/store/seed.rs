//! Deterministic demo data for [`MemoryStore`]: the Phase 2 UI mock.

use super::memory::{MemoryStore, SegRow, State, Words};
use super::{ScannedModelFile, StoreResult};
use crate::types::{
    new_id, AudioFormat, AudioTrack, Channel, Meeting, MeetingId, MeetingStatus, ModelCapabilities,
    ModelRole, ModelSource, RunId, RunKind, RunStatus, Segment, SegmentId, Source, Speaker,
    SpeakerId, TrackChannel, TrackId, TranscriptRun,
};
use crate::views::selection_key;

/// Shape of the generated data. `Default` is the Phase 2 mock: 500 meetings,
/// three of them with 2,000 segments, the rest with 40.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedOptions {
    pub meetings: usize,
    pub segments_per_meeting: usize,
    /// How many meetings (near the top of the list) get `large_meeting_segments`.
    pub large_meetings: usize,
    pub large_meeting_segments: usize,
    /// PRNG seed; the same options always produce the same data.
    pub rng_seed: u64,
}

impl Default for SeedOptions {
    fn default() -> Self {
        Self {
            meetings: 500,
            segments_per_meeting: 40,
            large_meetings: 3,
            large_meeting_segments: 2_000,
            rng_seed: 0x5eed_2026,
        }
    }
}

/// 2026-10-15 09:00 UTC: the newest seeded meeting starts a few hours before.
const ANCHOR_MS: i64 = 1_792_054_800_000;
const DAY_MS: i64 = 86_400_000;

const TITLES: &[&str] = &[
    "Weekly product sync",
    "Design review: onboarding flow",
    "1:1 with Priya",
    "Customer call: Northwind",
    "Sprint planning",
    "Interview: backend engineer",
    "All hands",
    "Retro",
    "Incident review: search outage",
    "Standup",
    "Roadmap brainstorm",
    "Budget review Q4",
    "Vendor demo: transcription APIs",
    "Hiring debrief",
    "Architecture deep dive",
];
const LARGE_TITLES: &[&str] = &[
    "Quarterly planning offsite, day 1",
    "Quarterly planning offsite, day 2",
    "Board prep marathon",
];
const NAMES: &[&str] = &[
    "Priya", "Marcus", "Elena", "Tomás", "Aiko", "Sam", "Fatima", "Jonas", "Grace", "Ravi",
];
const WORDS: &[&str] = &[
    "we",
    "should",
    "ship",
    "the",
    "release",
    "next",
    "week",
    "budget",
    "customer",
    "feedback",
    "roadmap",
    "latency",
    "search",
    "users",
    "onboarding",
    "design",
    "think",
    "maybe",
    "data",
    "really",
    "important",
    "timeline",
    "risk",
    "team",
    "decision",
    "follow",
    "up",
    "on",
    "that",
    "agree",
    "question",
    "about",
    "metrics",
    "quarter",
    "plan",
    "hiring",
    "review",
    "launch",
    "bug",
    "fix",
    "priority",
    "scope",
    "estimate",
    "meeting",
    "notes",
    "action",
    "items",
    "transcript",
    "speaker",
    "model",
    "accuracy",
    "and",
    "so",
    "yeah",
    "okay",
];

/// SplitMix64: tiny, fast, deterministic; no `rand` dependency for mock data.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn sentence(rng: &mut Rng) -> String {
    let n = rng.range(5, 28) as usize;
    let mut s = String::new();
    for i in 0..n {
        let w = rng.pick(WORDS);
        if i == 0 {
            let mut c = w.chars();
            if let Some(f) = c.next() {
                s.extend(f.to_uppercase());
                s.push_str(c.as_str());
            }
        } else {
            s.push(' ');
            s.push_str(w);
        }
    }
    s.push(if rng.chance(15) { '?' } else { '.' });
    s
}

impl MemoryStore {
    /// An empty store filled with the default demo data (500 meetings, three
    /// with 2,000 segments).
    pub fn demo() -> Self {
        let store = Self::new();
        store
            .seed_with(&SeedOptions::default())
            .expect("seeding a fresh store cannot fail");
        store
    }

    /// Adds `n_meetings` meetings with `segments_per_meeting` segments each,
    /// three of which (fewer if `n_meetings < 3`) get 2,000 segments.
    pub fn seed(&self, n_meetings: usize, segments_per_meeting: usize) -> StoreResult<()> {
        self.seed_with(&SeedOptions {
            meetings: n_meetings,
            segments_per_meeting,
            ..SeedOptions::default()
        })
    }

    /// Adds generated meetings, runs, speakers, segments and audio tracks.
    /// Words are synthesized on demand from segment text.
    pub fn seed_with(&self, opts: &SeedOptions) -> StoreResult<()> {
        let mut rng = Rng(opts.rng_seed);
        let mut st = self.lock()?;
        // The large meetings sit at positions 1, 4, 9 (newest first) so they are on
        // screen; small seeds fall back to later positions, then position 0.
        let large_slots: Vec<usize> = [1usize, 4, 9]
            .into_iter()
            .chain(10..opts.meetings)
            .chain([0])
            .filter(|&i| i < opts.meetings)
            .take(opts.large_meetings)
            .collect();
        let mut started = ANCHOR_MS - 3 * 3_600_000;

        for i in 0..opts.meetings {
            let large = large_slots.iter().position(|&s| s == i);
            let n_segments = match large {
                Some(_) => opts.large_meeting_segments,
                None => opts.segments_per_meeting,
            };
            let source = if rng.chance(60) {
                Source::Live
            } else {
                Source::File
            };
            let status = match i {
                0 => MeetingStatus::Processing,
                7 => MeetingStatus::Failed,
                _ => MeetingStatus::Done,
            };
            let title = match large {
                Some(k) => LARGE_TITLES[k % LARGE_TITLES.len()].to_owned(),
                None => rng.pick(TITLES).to_owned(),
            };
            let meeting_id = MeetingId(st.alloc());
            let uid = new_id();

            // Speakers: "me" plus loopback speakers for live meetings; source speakers for imports.
            let n_others = rng.range(1, 5) as usize;
            let run_id = RunId(st.alloc());
            let mut speaker_ids = Vec::new();
            let add_speaker = |st: &mut State, rng: &mut Rng, channel: Channel, label: String| {
                let id = SpeakerId(st.alloc());
                let named = label != "me" && rng.chance(40);
                st.speakers.insert(
                    id,
                    Speaker {
                        id,
                        run_id,
                        meeting_id,
                        channel,
                        engine_label: label,
                        display_name: named.then(|| rng.pick(NAMES).to_owned()),
                        color: None,
                        global_id: None,
                        merged_into: None,
                        needs_review: rng.chance(5),
                        name_anchor_start_ms: None,
                        name_anchor_end_ms: None,
                    },
                );
                id
            };
            let other_channel = if source == Source::Live {
                speaker_ids.push(add_speaker(&mut st, &mut rng, Channel::Mic, "me".into()));
                Channel::Loopback
            } else {
                Channel::Source
            };
            for k in 0..n_others {
                speaker_ids.push(add_speaker(
                    &mut st,
                    &mut rng,
                    other_channel,
                    format!("spk_{k}"),
                ));
            }

            // Segments: random turns with short gaps.
            let mut t = 0i64;
            let mut seg_ids = Vec::with_capacity(n_segments);
            let mut speaker = speaker_ids[0];
            for seq in 0..n_segments {
                if rng.chance(55) {
                    speaker = speaker_ids[rng.range(0, speaker_ids.len() as i64 - 1) as usize];
                }
                let text = sentence(&mut rng);
                let n_words = text.split_whitespace().count();
                let len = n_words as i64 * rng.range(280, 420);
                t += rng.range(150, 1_500);
                let id = SegmentId(st.alloc());
                let first_id = st.alloc_range(n_words);
                st.segments.insert(
                    id,
                    SegRow {
                        seg: Segment {
                            id,
                            run_id,
                            meeting_id,
                            speaker_id: speaker,
                            start_ms: t,
                            end_ms: t + len,
                            text,
                            seq: seq as i64,
                        },
                        words: Words::Synthetic { first_id },
                    },
                );
                seg_ids.push(id);
                t += len;
            }
            let duration_ms = t + 2_000;

            let has_run = status != MeetingStatus::Failed;
            if has_run {
                st.runs.insert(
                    run_id,
                    TranscriptRun {
                        id: run_id,
                        meeting_id,
                        kind: if source == Source::Live {
                            RunKind::PostPass
                        } else {
                            RunKind::File
                        },
                        engine_info: serde_json::json!({
                            "runtime": "nemo-speech 0.2.0",
                            "asr": "nemotron-speech-streaming-en-0.6b.q8_0.gguf",
                            "diarizer": "Nemotron-3-Diarization.q8_0.gguf",
                            "seeded": true
                        }),
                        started_at: started + duration_ms,
                        finished_at: (status == MeetingStatus::Done)
                            .then_some(started + duration_ms + duration_ms / 27),
                        status: if status == MeetingStatus::Done {
                            RunStatus::Done
                        } else {
                            RunStatus::Running
                        },
                    },
                );
                st.run_segments.insert(run_id, seg_ids);
            } else {
                for id in seg_ids {
                    st.segments.remove(&id);
                }
                st.speakers.retain(|_, s| s.run_id != run_id);
            }

            let track_id = TrackId(st.alloc());
            st.tracks.insert(
                track_id,
                AudioTrack {
                    id: track_id,
                    meeting_id,
                    channel: TrackChannel::Playback,
                    path: if source == Source::Live {
                        format!("audio/{uid}/playback.flac")
                    } else {
                        format!(
                            "C:/Users/me/Recordings/{}.m4a",
                            title.replace([' ', ':', ','], "-")
                        )
                    },
                    format: if source == Source::Live {
                        AudioFormat::Flac
                    } else {
                        AudioFormat::Original
                    },
                    sample_rate: if source == Source::Live {
                        16_000
                    } else {
                        48_000
                    },
                    channels: 2,
                    duration_ms: Some(duration_ms),
                    size_bytes: Some(duration_ms * 25),
                },
            );

            st.meetings.insert(
                meeting_id,
                Meeting {
                    id: meeting_id,
                    uid,
                    title,
                    source,
                    source_path: None,
                    language: "en".into(),
                    diarize_mic: false,
                    started_at: started,
                    ended_at: Some(started + duration_ms),
                    duration_ms: Some(duration_ms),
                    status,
                    current_run_id: has_run.then_some(run_id),
                    audio_keep: rng.chance(10),
                    created_at: started,
                    updated_at: started + duration_ms,
                },
            );

            // Next (older) meeting: 0.3 to 3 days earlier.
            started -= rng.range(DAY_MS * 3 / 10, DAY_MS * 3);
        }
        if st.model_files.is_empty() {
            seed_model_files(&mut st);
        }
        Ok(())
    }
}

struct SeedFile {
    path: &'static str,
    role: ModelRole,
    arch: &'static str,
    name: &'static str,
    quant: Option<&'static str>,
    size_bytes: i64,
    source: ModelSource,
    streaming: bool,
    diarize: bool,
    languages: &'static [&'static str],
    vram_measured: Option<u32>,
    vram_estimate: Option<u32>,
    manifest: Option<&'static str>,
}

/// What a typical install has under the data root, plus one unknown drop-in.
const SEED_FILES: &[SeedFile] = &[
    SeedFile {
        path: "engine/nemo-speech-0.2.0-windows-x86_64-cuda",
        role: ModelRole::Engine,
        arch: "0.2.0",
        name: "NeMo-Speech.cpp 0.2.0 (CUDA)",
        quant: None,
        size_bytes: 140_509_184,
        source: ModelSource::Manifest,
        streaming: false,
        diarize: false,
        languages: &[],
        vram_measured: None,
        vram_estimate: None,
        manifest: Some("nemo-speech-0.2.0-windows-x86_64-cuda.zip"),
    },
    SeedFile {
        path: "models/nemotron-3.5-asr-streaming-0.6b.q8_0.gguf",
        role: ModelRole::Asr,
        arch: "nemotron_asr",
        name: "Nemotron 3.5 ASR Streaming 0.6B",
        quant: Some("Q8_0"),
        size_bytes: 712_000_000,
        source: ModelSource::Manifest,
        streaming: true,
        diarize: true,
        languages: &["en", "de", "es", "fr", "it"],
        vram_measured: Some(1_150),
        vram_estimate: None,
        manifest: Some("nemotron-3.5-asr-streaming-0.6b.q8_0.gguf"),
    },
    SeedFile {
        path: "models/nemotron-speech-streaming-en-0.6b.q8_0.gguf",
        role: ModelRole::Asr,
        arch: "nemotron_asr",
        name: "Nemotron Speech Streaming EN 0.6B",
        quant: Some("Q8_0"),
        size_bytes: 670_000_000,
        source: ModelSource::Pull,
        streaming: true,
        diarize: true,
        languages: &["en"],
        vram_measured: Some(1_000),
        vram_estimate: None,
        manifest: Some("nemotron-speech-streaming-en-0.6b.q8_0.gguf"),
    },
    SeedFile {
        path: "models/parakeet-tdt-0.6b-v3.q8_0.gguf",
        role: ModelRole::Asr,
        arch: "parakeet_tdt",
        name: "Parakeet TDT 0.6B v3 (experimental)",
        quant: Some("Q8_0"),
        size_bytes: 690_000_000,
        source: ModelSource::Pull,
        streaming: false,
        diarize: false,
        languages: &["en", "de", "es", "fr"],
        vram_measured: Some(4_900),
        vram_estimate: None,
        manifest: Some("parakeet-tdt-0.6b-v3.q8_0.gguf"),
    },
    SeedFile {
        path: "models/Nemotron-3-Diarization.q8_0.gguf",
        role: ModelRole::Diarizer,
        arch: "sortformer",
        name: "Nemotron 3 Diarization",
        quant: Some("Q8_0"),
        size_bytes: 118_000_000,
        source: ModelSource::Manifest,
        streaming: true,
        diarize: true,
        languages: &[],
        vram_measured: Some(300),
        vram_estimate: None,
        manifest: Some("Nemotron-3-Diarization.q8_0.gguf"),
    },
    SeedFile {
        path: "models/user/whisper-large-v3.q5_0.gguf",
        role: ModelRole::Unknown,
        arch: "whisper",
        name: "whisper-large-v3.q5_0",
        quant: Some("Q5_0"),
        size_bytes: 1_080_000_000,
        source: ModelSource::User,
        streaming: false,
        diarize: false,
        languages: &[],
        vram_measured: None,
        vram_estimate: Some(1_600),
        manifest: None,
    },
];

/// Seeds `SEED_FILES` and selects the recommended engine, ASR and diarizer.
fn seed_model_files(st: &mut State) {
    for f in SEED_FILES {
        let scanned = ScannedModelFile {
            path: f.path.into(),
            role: f.role,
            architecture: Some(f.arch.into()),
            display_name: f.name.into(),
            quant: f.quant.map(str::to_owned),
            size_bytes: f.size_bytes,
            sha256: None,
            source: f.source,
            capabilities: ModelCapabilities {
                streaming: f.streaming,
                diarize: f.diarize,
                languages: f.languages.iter().map(|l| (*l).to_owned()).collect(),
                vram_mb_measured: f.vram_measured,
                vram_mb_estimate: f.vram_estimate,
            },
            manifest_name: f.manifest.map(str::to_owned),
        };
        st.upsert_model_file(&scanned, ANCHOR_MS);
    }
    for (role, index) in [
        (ModelRole::Engine, 0),
        (ModelRole::Asr, 1),
        (ModelRole::Diarizer, 4),
    ] {
        let path = SEED_FILES[index].path;
        if let Some(m) = st.model_files.values().find(|m| m.path == path) {
            let id = m.id.to_string();
            st.settings.insert(selection_key(role), id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn demo_has_500_meetings_and_three_large_ones() {
        let store = MemoryStore::demo();
        let list = store.list_meetings().unwrap();
        assert_eq!(list.len(), 500);
        assert!(list.windows(2).all(|w| w[0].started_at >= w[1].started_at));
        let large: Vec<_> = list
            .iter()
            .filter(|m| store.get_segments(m.id, 0, 5_000).unwrap().len() == 2_000)
            .collect();
        assert_eq!(large.len(), 3);
        assert_eq!(list[0].status, MeetingStatus::Processing);
        assert!(list.iter().any(|m| m.status == MeetingStatus::Failed));
        assert!(list[1].preview.chars().count() <= 200 && !list[1].preview.is_empty());

        let files = store.list_model_files().unwrap();
        assert_eq!(files.len(), 6);
        let settings = store.get_settings().unwrap();
        let status = crate::views::EngineStatus::from_parts(
            "data".into(),
            files,
            &settings,
            &["0.2.0"],
            None,
            None,
        );
        assert_eq!(status.engines.len(), 1);
        assert!(status.engines[0].tested);
        assert!(status.selected.engine.is_some() && status.selected.asr.is_some());
        assert!(status.selected.diarizer.is_some());
        assert_eq!(status.models_by_role[&ModelRole::Asr].len(), 3);
    }

    #[test]
    fn seeding_is_deterministic() {
        let a = MemoryStore::new();
        let b = MemoryStore::new();
        a.seed(20, 10).unwrap();
        b.seed(20, 10).unwrap();
        let strip = |s: &MemoryStore| -> Vec<(String, i64, u32, String)> {
            s.list_meetings()
                .unwrap()
                .into_iter()
                .map(|m| (m.title, m.started_at, m.speaker_count, m.preview))
                .collect()
        };
        assert_eq!(strip(&a), strip(&b));
    }

    #[test]
    fn seeded_words_and_search_work() {
        let store = MemoryStore::new();
        store.seed(5, 30).unwrap();
        let m = &store.list_meetings().unwrap()[2];
        let words = store.get_words(m.id, 0, 30_000).unwrap();
        assert!(!words.is_empty());
        assert!(words.windows(2).all(|w| w[0].start_ms <= w[1].start_ms));
        assert!(!store.search("budget", 10).unwrap().is_empty());
    }
}
