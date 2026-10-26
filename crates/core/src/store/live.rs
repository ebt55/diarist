//! Live preview helpers shared by both stores: the `words_json` row format,
//! the utterance label, and the transcript built from the latest revisions
//! (`docs/02`, "Run lifecycle").

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::NewTranscript;
use crate::protocol::EngineWord;
use crate::transcript::{
    build_transcript_channels, us_to_ms, SegmentOptions, TranscriptWord, ME_LABEL, UNKNOWN_LABEL,
};
use crate::types::{Channel, LiveUtterance};

/// One word in `live_utterances.words_json` (`[{t0,t1,text,conf,spk}]`, ms).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LiveWordJson {
    pub t0: i64,
    pub t1: i64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conf: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spk: Option<String>,
}

impl LiveWordJson {
    pub fn from_engine(w: &EngineWord) -> Self {
        Self {
            t0: us_to_ms(w.t0_us),
            t1: us_to_ms(w.t1_us.max(w.t0_us)),
            text: w.text.clone(),
            conf: w.conf,
            spk: w.spk.clone(),
        }
    }

    pub fn to_engine(&self) -> EngineWord {
        EngineWord {
            t0_us: self.t0.max(0) as u64 * 1_000,
            t1_us: self.t1.max(self.t0).max(0) as u64 * 1_000,
            text: self.text.clone(),
            conf: self.conf,
            spk: self.spk.clone(),
        }
    }
}

/// What a stored revision row holds besides its keys.
pub(crate) struct LiveRow {
    pub start_ms: i64,
    pub end_ms: i64,
    pub raw_label: Option<String>,
    pub words: Vec<LiveWordJson>,
}

/// Converts engine words to the stored form: ms times, start and end, and
/// the utterance's label (the most frequent word label, first on ties).
pub(crate) fn live_row(words: &[EngineWord]) -> LiveRow {
    let words: Vec<LiveWordJson> = words.iter().map(LiveWordJson::from_engine).collect();
    let start_ms = words.first().map(|w| w.t0).unwrap_or(0);
    let end_ms = words.iter().map(|w| w.t1).max().unwrap_or(start_ms);
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for w in &words {
        if let Some(s) = w.spk.as_deref() {
            match counts.iter_mut().find(|(l, _)| *l == s) {
                Some(e) => e.1 += 1,
                None => counts.push((s, 1)),
            }
        }
    }
    let raw_label = counts
        .iter()
        .fold(None::<(&str, usize)>, |best, &(l, n)| match best {
            Some((_, bn)) if bn >= n => best,
            _ => Some((l, n)),
        })
        .map(|(l, _)| l.to_owned());
    LiveRow {
        start_ms,
        end_ms,
        raw_label,
        words,
    }
}

/// Keeps the highest `rev` of every `(channel, utt)`.
pub(crate) fn latest_revisions(rows: &[LiveUtterance]) -> Vec<&LiveUtterance> {
    let mut best: BTreeMap<(Channel, u32), &LiveUtterance> = BTreeMap::new();
    for r in rows {
        match best.get(&(r.channel, r.utt)) {
            Some(b) if b.rev >= r.rev => {}
            _ => {
                best.insert((r.channel, r.utt), r);
            }
        }
    }
    let mut out: Vec<&LiveUtterance> = best.into_values().collect();
    out.sort_by_key(|u| (u.start_ms, u.utt));
    out
}

/// The transcript of a live run from the latest revision of each utterance
/// (already filtered by [`latest_revisions`]): each channel segmented on its
/// own and the segments ordered by start time, mic words without a label
/// become `me`, other untagged words `spk_unknown`.
pub(crate) fn transcript_from_live(
    latest: &[&LiveUtterance],
    opts: &SegmentOptions,
    skip: &dyn Fn(&LiveUtterance, usize) -> bool,
) -> NewTranscript {
    let mut per_channel: BTreeMap<Channel, Vec<TranscriptWord>> = BTreeMap::new();
    for u in latest {
        let default = if u.channel == Channel::Mic {
            ME_LABEL
        } else {
            UNKNOWN_LABEL
        };
        per_channel.entry(u.channel).or_default().extend(
            u.words
                .iter()
                .enumerate()
                .filter(|(i, _)| !skip(u, *i))
                .map(|(_, w)| {
                    let j = LiveWordJson::from_engine(w);
                    TranscriptWord {
                        channel: u.channel,
                        start_ms: j.t0,
                        end_ms: j.t1,
                        text: j.text,
                        confidence: j.conf,
                        speaker: j.spk.clone().unwrap_or_else(|| default.to_owned()),
                        raw_label: j.spk,
                        raw_text: None,
                    }
                }),
        );
    }
    build_transcript_channels(per_channel.into_iter().collect(), opts)
}
