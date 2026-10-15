//! Minimal transcript export (md, txt, json, srt, vtt) for the mock phase.
//! The real exporter moves to `meet_core::export` later; keep this small.

use meet_core::views::{MeetingDetail, SegmentView};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Md,
    Txt,
    Json,
    Srt,
    Vtt,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Md => "md",
            ExportFormat::Txt => "txt",
            ExportFormat::Json => "json",
            ExportFormat::Srt => "srt",
            ExportFormat::Vtt => "vtt",
        }
    }

    pub fn filter_name(self) -> &'static str {
        match self {
            ExportFormat::Md => "Markdown",
            ExportFormat::Txt => "Plain text",
            ExportFormat::Json => "JSON",
            ExportFormat::Srt => "SubRip subtitles",
            ExportFormat::Vtt => "WebVTT subtitles",
        }
    }
}

/// `h:mm:ss` (txt, md), or `hh:mm:ss<sep>mmm` for subtitles.
fn clock(ms: i64, millis_sep: Option<char>) -> String {
    let ms = ms.max(0);
    let (h, m, s) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60);
    match millis_sep {
        Some(sep) => format!("{h:02}:{m:02}:{s:02}{sep}{:03}", ms % 1000),
        None => format!("{h}:{m:02}:{s:02}"),
    }
}

/// A file name for the save dialog: the title without characters Windows rejects.
pub fn file_name(title: &str, format: ExportFormat) -> String {
    let clean: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let clean = clean.trim().trim_end_matches('.');
    let stem = if clean.is_empty() { "meeting" } else { clean };
    format!("{stem}.{}", format.extension())
}

pub fn render(format: ExportFormat, detail: &MeetingDetail, segments: &[SegmentView]) -> String {
    let title = if detail.meeting.title.is_empty() {
        "Untitled meeting"
    } else {
        detail.meeting.title.as_str()
    };
    let mut out = String::new();
    match format {
        ExportFormat::Txt => {
            out.push_str(&format!("{title}\n\n"));
            for s in segments {
                out.push_str(&format!(
                    "[{}] {}: {}\n",
                    clock(s.start_ms, None),
                    s.speaker_name,
                    s.text
                ));
            }
        }
        ExportFormat::Md => {
            out.push_str(&format!("# {title}\n"));
            let mut last_speaker = None;
            for s in segments {
                if last_speaker != Some(s.speaker_id) {
                    out.push_str(&format!(
                        "\n**{}** `{}`\n\n",
                        s.speaker_name,
                        clock(s.start_ms, None)
                    ));
                    last_speaker = Some(s.speaker_id);
                }
                out.push_str(&format!("{}\n", s.text));
            }
        }
        ExportFormat::Json => {
            let doc = serde_json::json!({
                "meeting": detail.meeting,
                "speakers": detail.speakers,
                "segments": segments,
            });
            out = serde_json::to_string_pretty(&doc).unwrap_or_default();
            out.push('\n');
        }
        ExportFormat::Srt => {
            for (i, s) in segments.iter().enumerate() {
                out.push_str(&format!(
                    "{}\n{} --> {}\n{}: {}\n\n",
                    i + 1,
                    clock(s.start_ms, Some(',')),
                    clock(s.end_ms, Some(',')),
                    s.speaker_name,
                    s.text
                ));
            }
        }
        ExportFormat::Vtt => {
            out.push_str("WEBVTT\n\n");
            for s in segments {
                out.push_str(&format!(
                    "{} --> {}\n<v {}>{}\n\n",
                    clock(s.start_ms, Some('.')),
                    clock(s.end_ms, Some('.')),
                    s.speaker_name,
                    s.text
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_and_file_names() {
        assert_eq!(clock(3_723_045, None), "1:02:03");
        assert_eq!(clock(3_723_045, Some(',')), "01:02:03,045");
        assert_eq!(clock(-5, Some('.')), "00:00:00.000");
        assert_eq!(
            file_name("Design review: onboarding", ExportFormat::Srt),
            "Design review- onboarding.srt"
        );
        assert_eq!(file_name("  ", ExportFormat::Md), "meeting.md");
    }
}
