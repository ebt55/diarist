//! Transcript export: Markdown, plain text, JSON, SRT, WebVTT and RTTM.
//!
//! [`ExportDoc::from_store`] reads a meeting's current run through the
//! [`Store`] trait (speakers resolved through merges, so exports show what
//! the UI shows); [`render`] turns it into one of the [`ExportFormat`]s.
//! RTTM is for the eval harness: one line per segment with the speaker's
//! engine label (no spaces), times in seconds.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::str::FromStr;

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::store::Store;
use crate::types::{Meeting, MeetingId, SegmentId, Word};
use crate::views::{SegmentView, SpeakerView};

/// Output formats of [`render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Markdown,
    Text,
    Json,
    Srt,
    Vtt,
    Rttm,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [
        ExportFormat::Markdown,
        ExportFormat::Text,
        ExportFormat::Json,
        ExportFormat::Srt,
        ExportFormat::Vtt,
        ExportFormat::Rttm,
    ];

    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Text => "txt",
            ExportFormat::Json => "json",
            ExportFormat::Srt => "srt",
            ExportFormat::Vtt => "vtt",
            ExportFormat::Rttm => "rttm",
        }
    }
}

impl FromStr for ExportFormat {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "md" | "markdown" => ExportFormat::Markdown,
            "txt" | "text" => ExportFormat::Text,
            "json" => ExportFormat::Json,
            "srt" => ExportFormat::Srt,
            "vtt" | "webvtt" => ExportFormat::Vtt,
            "rttm" => ExportFormat::Rttm,
            _ => {
                return Err(CoreError::InvalidEnum {
                    kind: "export format",
                    value: s.to_owned(),
                })
            }
        })
    }
}

/// Everything an export needs: the meeting, its current run's speakers,
/// segments (in `seq` order) and words.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDoc {
    pub meeting: Meeting,
    pub speakers: Vec<SpeakerView>,
    pub segments: Vec<SegmentView>,
    pub words: Vec<Word>,
}

impl ExportDoc {
    /// Reads the meeting's current run (empty when it has none).
    pub fn from_store(store: &dyn Store, id: MeetingId) -> Result<Self> {
        let detail = store.get_meeting(id)?;
        let mut segments = Vec::new();
        loop {
            let page = store.get_segments(id, segments.len() as u32, 5_000)?;
            let n = page.len();
            segments.extend(page);
            if n < 5_000 {
                break;
            }
        }
        let words = store.get_words(id, i64::MIN, i64::MAX)?;
        Ok(Self {
            meeting: detail.meeting,
            speakers: detail.speakers,
            segments,
            words,
        })
    }

    fn label_of(&self, speaker: crate::types::SpeakerId) -> &str {
        self.speakers
            .iter()
            .find(|s| s.id == speaker)
            .map(|s| s.engine_label.as_str())
            .unwrap_or("spk_unknown")
    }

    /// Distinct resolved speaker names in order of first appearance.
    fn speaker_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for s in &self.segments {
            if !names.contains(&s.speaker_name.as_str()) {
                names.push(&s.speaker_name);
            }
        }
        names
    }
}

/// Renders `doc` as `format`.
pub fn render(doc: &ExportDoc, format: ExportFormat) -> String {
    match format {
        ExportFormat::Markdown => to_markdown(doc),
        ExportFormat::Text => to_text(doc),
        ExportFormat::Json => to_json(doc),
        ExportFormat::Srt => to_srt(doc),
        ExportFormat::Vtt => to_vtt(doc),
        ExportFormat::Rttm => to_rttm(doc),
    }
}

/// `h:mm:ss` for durations, `hh:mm:ss` for positions.
fn hms(ms: i64, pad_hours: bool) -> String {
    let s = ms.max(0) / 1_000;
    let (h, m, s) = (s / 3_600, (s / 60) % 60, s % 60);
    if pad_hours {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h}:{m:02}:{s:02}")
    }
}

fn cue_time(ms: i64, sep: char) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1_000) % 60,
        ms % 1_000
    )
}

/// `YYYY-MM-DD HH:MM UTC` for unix milliseconds (proleptic Gregorian).
pub fn utc_datetime(ms: i64) -> String {
    let secs = ms.div_euclid(1_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3_600,
        (rem / 60) % 60
    )
}

/// Markdown: title, a facts list, then the transcript with a speaker heading
/// at every change of speaker.
pub fn to_markdown(doc: &ExportDoc) -> String {
    let m = &doc.meeting;
    let mut out = String::new();
    let title = if m.title.trim().is_empty() {
        "Untitled meeting"
    } else {
        m.title.trim()
    };
    let _ = writeln!(out, "# {title}\n");
    let _ = writeln!(out, "- Date: {}", utc_datetime(m.started_at));
    if let Some(d) = m.duration_ms {
        let _ = writeln!(out, "- Duration: {}", hms(d, false));
    }
    let names = doc.speaker_names();
    if !names.is_empty() {
        let _ = writeln!(out, "- Speakers: {}", names.join(", "));
    }
    let _ = writeln!(out, "- Language: {}", m.language);
    let mut last: Option<crate::types::SpeakerId> = None;
    for s in &doc.segments {
        if last != Some(s.speaker_id) {
            let _ = write!(
                out,
                "\n**{}** [{}]\n\n",
                s.speaker_name,
                hms(s.start_ms, true)
            );
            last = Some(s.speaker_id);
        } else {
            out.push('\n');
        }
        let _ = writeln!(out, "{}", s.text);
    }
    out
}

/// One line per segment: `[hh:mm:ss] Name: text`.
pub fn to_text(doc: &ExportDoc) -> String {
    let mut out = String::new();
    for s in &doc.segments {
        let _ = writeln!(
            out,
            "[{}] {}: {}",
            hms(s.start_ms, true),
            s.speaker_name,
            s.text
        );
    }
    out
}

#[derive(Serialize)]
struct JsonDoc<'a> {
    meeting: JsonMeeting<'a>,
    speakers: Vec<JsonSpeaker<'a>>,
    segments: Vec<JsonSegment<'a>>,
}

#[derive(Serialize)]
struct JsonMeeting<'a> {
    uid: &'a str,
    title: &'a str,
    source: &'a str,
    source_path: Option<&'a str>,
    language: &'a str,
    started_at: i64,
    duration_ms: Option<i64>,
}

#[derive(Serialize)]
struct JsonSpeaker<'a> {
    id: i64,
    label: &'a str,
    name: &'a str,
    color: &'a str,
    channel: &'a str,
    merged_into: Option<i64>,
    talk_ms: i64,
    segment_count: u32,
}

#[derive(Serialize)]
struct JsonSegment<'a> {
    seq: i64,
    start_ms: i64,
    end_ms: i64,
    speaker_id: i64,
    speaker: &'a str,
    text: &'a str,
    words: Vec<JsonWord<'a>>,
}

#[derive(Serialize)]
struct JsonWord<'a> {
    start_ms: i64,
    end_ms: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_label: Option<&'a str>,
}

/// Pretty JSON with meeting facts, speakers, and segments with their words.
pub fn to_json(doc: &ExportDoc) -> String {
    let m = &doc.meeting;
    let mut words_by_seg: HashMap<SegmentId, Vec<&Word>> = HashMap::new();
    for w in &doc.words {
        words_by_seg.entry(w.segment_id).or_default().push(w);
    }
    let value = JsonDoc {
        meeting: JsonMeeting {
            uid: &m.uid,
            title: &m.title,
            source: m.source.as_str(),
            source_path: m.source_path.as_deref(),
            language: &m.language,
            started_at: m.started_at,
            duration_ms: m.duration_ms,
        },
        speakers: doc
            .speakers
            .iter()
            .map(|s| JsonSpeaker {
                id: s.id.0,
                label: &s.engine_label,
                name: &s.name,
                color: &s.color,
                channel: s.channel.as_str(),
                merged_into: s.merged_into.map(|m| m.0),
                talk_ms: s.talk_ms,
                segment_count: s.segment_count,
            })
            .collect(),
        segments: doc
            .segments
            .iter()
            .map(|s| JsonSegment {
                seq: s.seq,
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                speaker_id: s.speaker_id.0,
                speaker: &s.speaker_name,
                text: &s.text,
                words: words_by_seg
                    .get(&s.id)
                    .map(|ws| {
                        ws.iter()
                            .map(|w| JsonWord {
                                start_ms: w.start_ms,
                                end_ms: w.end_ms,
                                text: &w.text,
                                raw_label: w.raw_label.as_deref(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
    };
    let mut s = serde_json::to_string_pretty(&value).unwrap_or_default();
    s.push('\n');
    s
}

/// SubRip: one cue per segment, `Name: text`.
pub fn to_srt(doc: &ExportDoc) -> String {
    let mut out = String::new();
    for (i, s) in doc.segments.iter().enumerate() {
        let _ = write!(
            out,
            "{}\n{} --> {}\n{}: {}\n\n",
            i + 1,
            cue_time(s.start_ms, ','),
            cue_time(s.end_ms.max(s.start_ms), ','),
            s.speaker_name,
            s.text
        );
    }
    out
}

fn vtt_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// WebVTT: one cue per segment with a `<v Name>` voice span.
pub fn to_vtt(doc: &ExportDoc) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (i, s) in doc.segments.iter().enumerate() {
        let _ = write!(
            out,
            "{}\n{} --> {}\n<v {}>{}\n\n",
            i + 1,
            cue_time(s.start_ms, '.'),
            cue_time(s.end_ms.max(s.start_ms), '.'),
            vtt_escape(&s.speaker_name),
            vtt_escape(&s.text)
        );
    }
    out
}

/// RTTM (NIST): `SPEAKER <uid> 1 <start> <dur> <NA> <NA> <label> <NA> <NA>`.
pub fn to_rttm(doc: &ExportDoc) -> String {
    let mut out = String::new();
    for s in &doc.segments {
        let dur = (s.end_ms - s.start_ms).max(0);
        let _ = writeln!(
            out,
            "SPEAKER {} 1 {:.3} {:.3} <NA> <NA> {} <NA> <NA>",
            doc.meeting.uid,
            s.start_ms as f64 / 1_000.0,
            dur as f64 / 1_000.0,
            doc.label_of(s.speaker_id)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Channel, MeetingStatus, RunId, Source, SpeakerId, WordId};

    /// A fixed document (no random uids or clocks) for golden files.
    fn doc() -> ExportDoc {
        let meeting = Meeting {
            id: MeetingId(1),
            uid: "01JA0000000000000000000000".into(),
            title: "Budget review".into(),
            source: Source::File,
            source_path: Some("C:/rec/budget.m4a".into()),
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_792_054_800_000,
            ended_at: Some(1_792_054_865_000),
            duration_ms: Some(65_000),
            status: MeetingStatus::Done,
            current_run_id: Some(RunId(2)),
            audio_keep: false,
            created_at: 0,
            updated_at: 0,
        };
        let sv = |id: i64, label: &str, name: &str, color: &str| SpeakerView {
            id: SpeakerId(id),
            channel: Channel::Source,
            engine_label: label.into(),
            display_name: None,
            name: name.into(),
            color: color.into(),
            merged_into: None,
            needs_review: false,
            segment_count: 0,
            talk_ms: 0,
        };
        let speakers = vec![
            sv(10, "spk_0", "Priya", "#4e79a7"),
            sv(11, "spk_1", "Speaker 2", "#f28e2b"),
        ];
        let seg = |id: i64, seq: i64, start: i64, end: i64, spk: i64, name: &str, text: &str| {
            SegmentView {
                id: SegmentId(id),
                seq,
                start_ms: start,
                end_ms: end,
                text: text.into(),
                speaker_id: SpeakerId(spk),
                speaker_name: name.into(),
                speaker_color: String::new(),
                channel: Channel::Source,
                needs_review: false,
            }
        };
        let segments = vec![
            seg(100, 0, 1_200, 3_400, 10, "Priya", "Hello budget team."),
            seg(101, 1, 3_900, 4_500, 10, "Priya", "Shall we start?"),
            seg(
                102,
                2,
                5_000,
                9_250,
                11,
                "Speaker 2",
                "Yes <finally> & fast",
            ),
            seg(103, 3, 3_725_000, 3_726_500, 10, "Priya", "Thanks"),
        ];
        let words = [
            (100, 1_200, 1_700, "Hello"),
            (100, 1_800, 2_400, "budget"),
            (100, 2_500, 3_400, "team."),
            (101, 3_900, 4_100, "Shall"),
            (101, 4_100, 4_300, "we"),
            (101, 4_300, 4_500, "start?"),
            (102, 5_000, 5_400, "Yes"),
            (102, 5_500, 6_000, "<finally>"),
            (102, 6_100, 6_200, "&"),
            (102, 8_000, 9_250, "fast"),
            (103, 3_725_000, 3_726_500, "Thanks"),
        ]
        .iter()
        .enumerate()
        .map(|(i, &(seg, s, e, t))| Word {
            id: WordId(1_000 + i as i64),
            segment_id: SegmentId(seg),
            meeting_id: MeetingId(1),
            start_ms: s,
            end_ms: e,
            text: t.into(),
            confidence: None,
            raw_label: Some(if seg == 102 { "spk_1" } else { "spk_0" }.into()),
            margin: None,
            overlap: false,
        })
        .collect();
        ExportDoc {
            meeting,
            speakers,
            segments,
            words,
        }
    }

    fn golden(format: ExportFormat) {
        let path = format!(
            "{}/tests/golden/export.{}",
            env!("CARGO_MANIFEST_DIR"),
            format.extension()
        );
        let got = render(&doc(), format);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).unwrap();
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{path}: {e} (run with UPDATE_GOLDEN=1)"))
            .replace("\r\n", "\n");
        assert_eq!(got, want, "{format:?} differs from {path}");
    }

    #[test]
    fn golden_markdown() {
        golden(ExportFormat::Markdown);
    }

    #[test]
    fn golden_text() {
        golden(ExportFormat::Text);
    }

    #[test]
    fn golden_json() {
        golden(ExportFormat::Json);
    }

    #[test]
    fn golden_srt() {
        golden(ExportFormat::Srt);
    }

    #[test]
    fn golden_vtt() {
        golden(ExportFormat::Vtt);
    }

    #[test]
    fn golden_rttm() {
        golden(ExportFormat::Rttm);
    }

    #[test]
    fn formats_parse_and_dates_render() {
        assert_eq!(
            "MD".parse::<ExportFormat>().unwrap(),
            ExportFormat::Markdown
        );
        assert_eq!("webvtt".parse::<ExportFormat>().unwrap(), ExportFormat::Vtt);
        assert!("docx".parse::<ExportFormat>().is_err());
        assert_eq!(utc_datetime(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_datetime(951_782_400_000), "2000-02-29 00:00 UTC");
        assert_eq!(utc_datetime(1_792_054_800_000), "2026-10-15 09:00 UTC");
        assert_eq!(hms(3_725_000, true), "01:02:05");
        assert_eq!(cue_time(3_725_042, ','), "01:02:05,042");
    }

    #[test]
    fn from_store_reads_the_current_run() {
        use crate::store::{
            MemoryStore, NewMeeting, NewSegment, NewSpeaker, NewTranscript, NewWord,
        };
        use crate::types::RunKind;
        let store = MemoryStore::new();
        let m = store
            .create_meeting(NewMeeting {
                title: "T".into(),
                source: Source::File,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: 0,
                status: MeetingStatus::Done,
            })
            .unwrap();
        let run = store
            .create_run(m.id, RunKind::File, serde_json::json!({}))
            .unwrap();
        let t = NewTranscript {
            speakers: vec![NewSpeaker {
                channel: Channel::Source,
                engine_label: "spk_0".into(),
                display_name: None,
                color: None,
                needs_review: false,
            }],
            segments: vec![NewSegment {
                speaker: 0,
                start_ms: 0,
                end_ms: 500,
                text: "hi there".into(),
            }],
            words: vec![NewWord {
                segment: 0,
                start_ms: 0,
                end_ms: 200,
                text: "hi".into(),
                confidence: None,
                raw_label: None,
                margin: None,
                overlap: false,
            }],
        };
        store.insert_transcript(run.id, &t).unwrap();
        store.set_current_run(m.id, run.id).unwrap();
        let doc = ExportDoc::from_store(&store, m.id).unwrap();
        assert_eq!(doc.segments.len(), 1);
        assert_eq!(doc.words.len(), 1);
        assert_eq!(to_text(&doc), "[00:00:00] Speaker 1: hi there\n");
        assert!(to_rttm(&doc).contains(" spk_0 "));
    }
}
