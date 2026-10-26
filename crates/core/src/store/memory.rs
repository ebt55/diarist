//! In-memory [`Store`] and [`RunStore`]: the UI mock (`--demo`) and a
//! reference implementation for the conformance suite. Not persistent; one
//! mutex around all state.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use super::carry_over::{
    carry_over, carry_over_identity, group_bounds, CarryBackend, CarryOverReport, OwnedWord,
    SpeakerUpdate,
};
use super::live::{latest_revisions, live_row, transcript_from_live};
use super::{
    is_hex_color, pick_playback_track, NewAudioTrack, NewMeeting, NewTranscript, RunStore,
    ScannedModelFile, Store, StoreError, StoreResult,
};
use crate::protocol::EngineWord;
use crate::transcript::{join_text, SegmentOptions};
use crate::types::{
    new_id, AudioTrack, Channel, ClockAnchor, EchoDroppedWord, Edit, EditKind, LiveUtterance,
    Meeting, MeetingId, MeetingStatus, ModelFile, ModelFileId, ReassignTarget, RunId, RunKind,
    RunStatus, Segment, SegmentId, Speaker, SpeakerId, TrackId, TranscriptRun, Word, WordId,
};
use crate::views::{
    default_speaker_color, default_speaker_name, MeetingDetail, MeetingSummary, SearchHit,
    SegmentView, Settings, SnippetPart, SpeakerView,
};

/// Characters of transcript text in [`MeetingSummary::preview`].
const PREVIEW_CHARS: usize = 200;
/// Characters of context before and after the first match in a snippet.
const SNIPPET_BEFORE: usize = 60;
const SNIPPET_AFTER: usize = 100;

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// In-memory implementation of [`Store`]. See [`MemoryStore::demo`] for the
/// seeded UI mock.
#[derive(Default)]
pub struct MemoryStore {
    pub(super) state: Mutex<State>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn lock(&self) -> StoreResult<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| StoreError::Backend("memory store lock poisoned".into()))
    }
}

/// Words of a segment: stored, or synthesized from the text (seeded data, so
/// a 500-meeting mock does not hold half a million word rows).
pub(super) enum Words {
    Stored(Vec<Word>),
    /// One word per whitespace-separated token, ids `first_id..`, times spread
    /// evenly over the segment.
    Synthetic {
        first_id: i64,
    },
}

pub(super) struct SegRow {
    pub seg: Segment,
    pub words: Words,
}

#[derive(Default)]
pub(super) struct State {
    next_id: i64,
    pub meetings: BTreeMap<MeetingId, Meeting>,
    pub runs: BTreeMap<RunId, TranscriptRun>,
    pub tracks: BTreeMap<TrackId, AudioTrack>,
    pub speakers: BTreeMap<SpeakerId, Speaker>,
    pub segments: HashMap<SegmentId, SegRow>,
    /// Segment ids of each run in `seq` order; `seq` always equals the index.
    pub run_segments: HashMap<RunId, Vec<SegmentId>>,
    pub edits: Vec<Edit>,
    pub settings: BTreeMap<String, String>,
    pub model_files: BTreeMap<ModelFileId, ModelFile>,
    /// Live utterance revisions in insertion order.
    pub live: Vec<LiveUtterance>,
    /// `(meeting, channel, sample_index) -> qpc_100ns`.
    pub anchors: BTreeMap<(MeetingId, Channel, i64), i64>,
    /// Words the echo gate removed, per run (`transcript_runs.echo_dropped`).
    pub echo_dropped: HashMap<RunId, Vec<EchoDroppedWord>>,
}

impl State {
    /// Next id; one counter for all tables keeps ids unique across kinds.
    pub fn alloc(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    /// Reserves `n` consecutive ids and returns the first.
    pub fn alloc_range(&mut self, n: usize) -> i64 {
        let first = self.next_id + 1;
        self.next_id += n as i64;
        first
    }

    fn meeting(&self, id: MeetingId) -> StoreResult<&Meeting> {
        self.meetings
            .get(&id)
            .ok_or_else(|| StoreError::not_found("meeting", id))
    }

    fn meeting_mut(&mut self, id: MeetingId) -> StoreResult<&mut Meeting> {
        self.meetings
            .get_mut(&id)
            .ok_or_else(|| StoreError::not_found("meeting", id))
    }

    fn run(&self, id: RunId) -> StoreResult<&TranscriptRun> {
        self.runs
            .get(&id)
            .ok_or_else(|| StoreError::not_found("run", id))
    }

    fn speaker(&self, id: SpeakerId) -> StoreResult<&Speaker> {
        self.speakers
            .get(&id)
            .ok_or_else(|| StoreError::not_found("speaker", id))
    }

    fn segment(&self, id: SegmentId) -> StoreResult<&SegRow> {
        self.segments
            .get(&id)
            .ok_or_else(|| StoreError::not_found("segment", id))
    }

    fn root(&self, id: SpeakerId) -> SpeakerId {
        self.speakers
            .get(&id)
            .and_then(|s| s.merged_into)
            .unwrap_or(id)
    }

    fn current_segments(&self, meeting: &Meeting) -> &[SegmentId] {
        meeting
            .current_run_id
            .and_then(|r| self.run_segments.get(&r))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn words_of(&self, row: &SegRow) -> Vec<Word> {
        match &row.words {
            Words::Stored(words) => words.clone(),
            Words::Synthetic { first_id } => synthesize_words(&row.seg, *first_id),
        }
    }

    fn segment_view(&self, seg: &Segment) -> SegmentView {
        let root = self.root(seg.speaker_id);
        let (name, color, needs_review) = match self.speakers.get(&root) {
            Some(s) => (display_name(s), display_color(s), s.needs_review),
            None => (
                "Unknown speaker".to_owned(),
                default_speaker_color("").into(),
                false,
            ),
        };
        let channel = self
            .speakers
            .get(&seg.speaker_id)
            .map(|s| s.channel)
            .unwrap_or(crate::types::Channel::Source);
        SegmentView {
            id: seg.id,
            seq: seg.seq,
            start_ms: seg.start_ms,
            end_ms: seg.end_ms,
            text: seg.text.clone(),
            speaker_id: root,
            speaker_name: name,
            speaker_color: color,
            channel,
            needs_review,
        }
    }

    fn summary(&self, m: &Meeting) -> MeetingSummary {
        let segs = self.current_segments(m);
        let mut speakers = HashSet::new();
        let mut preview = String::new();
        let mut preview_chars = 0;
        for sid in segs {
            let Some(row) = self.segments.get(sid) else {
                continue;
            };
            speakers.insert(self.root(row.seg.speaker_id));
            if preview_chars < PREVIEW_CHARS {
                if !preview.is_empty() {
                    preview.push(' ');
                    preview_chars += 1;
                }
                for c in row.seg.text.chars() {
                    if preview_chars >= PREVIEW_CHARS {
                        break;
                    }
                    preview.push(c);
                    preview_chars += 1;
                }
            }
        }
        MeetingSummary {
            id: m.id,
            uid: m.uid.clone(),
            title: m.title.clone(),
            source: m.source,
            status: m.status,
            started_at: m.started_at,
            duration_ms: m.duration_ms,
            speaker_count: speakers.len() as u32,
            preview,
            audio_keep: m.audio_keep,
        }
    }

    fn speaker_views(&self, run: RunId) -> Vec<SpeakerView> {
        let mut own: HashMap<SpeakerId, (u32, i64)> = HashMap::new();
        for sid in self
            .run_segments
            .get(&run)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            if let Some(row) = self.segments.get(sid) {
                let e = own.entry(row.seg.speaker_id).or_default();
                e.0 += 1;
                e.1 += row.seg.end_ms - row.seg.start_ms;
            }
        }
        let run_speakers: Vec<&Speaker> =
            self.speakers.values().filter(|s| s.run_id == run).collect();
        let mut views: Vec<SpeakerView> = run_speakers
            .iter()
            .map(|s| {
                let (mut count, mut talk) = own.get(&s.id).copied().unwrap_or_default();
                if s.merged_into.is_none() {
                    for child in run_speakers.iter().filter(|c| c.merged_into == Some(s.id)) {
                        let (c, t) = own.get(&child.id).copied().unwrap_or_default();
                        count += c;
                        talk += t;
                    }
                }
                SpeakerView {
                    id: s.id,
                    channel: s.channel,
                    engine_label: s.engine_label.clone(),
                    display_name: s.display_name.clone(),
                    name: display_name(s),
                    color: display_color(s),
                    merged_into: s.merged_into,
                    needs_review: s.needs_review,
                    segment_count: count,
                    talk_ms: talk,
                }
            })
            .collect();
        views.sort_by(|a, b| {
            (a.merged_into.is_some(), -a.talk_ms, a.id).cmp(&(
                b.merged_into.is_some(),
                -b.talk_ms,
                b.id,
            ))
        });
        views
    }

    fn sorted_model_files(&self) -> Vec<ModelFile> {
        let mut files: Vec<ModelFile> = self.model_files.values().cloned().collect();
        files.sort_by(|a, b| (a.role, &a.display_name, a.id).cmp(&(b.role, &b.display_name, b.id)));
        files
    }

    /// Inserts or updates one scanned file.
    pub fn upsert_model_file(&mut self, f: &ScannedModelFile, now_ms: i64) {
        let existing = self
            .model_files
            .values()
            .find(|m| m.path == f.path)
            .map(|m| (m.id, m.first_seen_at));
        let (id, first_seen_at) = match existing {
            Some(e) => e,
            None => (ModelFileId(self.alloc()), now_ms),
        };
        self.model_files.insert(
            id,
            ModelFile {
                id,
                path: f.path.clone(),
                role: f.role,
                architecture: f.architecture.clone(),
                display_name: f.display_name.clone(),
                quant: f.quant.clone(),
                size_bytes: f.size_bytes,
                sha256: f.sha256.clone(),
                source: f.source,
                capabilities: f.capabilities.clone(),
                manifest_name: f.manifest_name.clone(),
                first_seen_at,
                last_seen_at: now_ms,
                missing: false,
            },
        );
    }

    /// Removes a run and everything hanging off it; clears `current_run_id`
    /// where it pointed there.
    fn remove_run(&mut self, run: RunId) {
        self.runs.remove(&run);
        self.echo_dropped.remove(&run);
        for sid in self.run_segments.remove(&run).unwrap_or_default() {
            self.segments.remove(&sid);
        }
        self.speakers.retain(|_, s| s.run_id != run);
        self.live.retain(|u| u.run_id != run);
        for m in self.meetings.values_mut() {
            if m.current_run_id == Some(run) {
                m.current_run_id = None;
            }
        }
    }

    /// Words of a segment in `(start, id)` order.
    fn sorted_words(&self, row: &SegRow) -> Vec<Word> {
        let mut words = self.words_of(row);
        words.sort_by_key(|w| (w.start_ms, w.id));
        words
    }

    /// The channel a segment's words are on (its speaker's).
    fn segment_channel(&self, seg: &Segment) -> Channel {
        self.speakers
            .get(&seg.speaker_id)
            .map(|s| s.channel)
            .unwrap_or(Channel::Source)
    }
}

fn display_name(s: &Speaker) -> String {
    s.display_name
        .clone()
        .unwrap_or_else(|| default_speaker_name(&s.engine_label))
}

fn display_color(s: &Speaker) -> String {
    s.color
        .clone()
        .unwrap_or_else(|| default_speaker_color(&s.engine_label).to_owned())
}

pub(super) fn synthesize_words(seg: &Segment, first_id: i64) -> Vec<Word> {
    let tokens: Vec<&str> = seg.text.split_whitespace().collect();
    let n = tokens.len().max(1) as i64;
    let span = (seg.end_ms - seg.start_ms).max(n);
    tokens
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let i = i as i64;
            Word {
                id: WordId(first_id + i),
                segment_id: seg.id,
                meeting_id: seg.meeting_id,
                start_ms: seg.start_ms + span * i / n,
                end_ms: seg.start_ms + span * (i + 1) / n,
                text: (*text).to_owned(),
                confidence: None,
                raw_label: None,
                margin: None,
                overlap: false,
                raw_text: None,
            }
        })
        .collect()
}

impl CarryBackend for State {
    fn run(&mut self, run: RunId) -> StoreResult<TranscriptRun> {
        State::run(self, run).cloned()
    }

    fn speakers(&mut self, run: RunId) -> StoreResult<Vec<Speaker>> {
        Ok(self
            .speakers
            .values()
            .filter(|s| s.run_id == run)
            .cloned()
            .collect())
    }

    fn words(&mut self, run: RunId) -> StoreResult<BTreeMap<Channel, Vec<OwnedWord>>> {
        let mut all: Vec<(Channel, i64, WordId, OwnedWord)> = Vec::new();
        for sid in self
            .run_segments
            .get(&run)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let Some(row) = self.segments.get(sid) else {
                continue;
            };
            let channel = self.segment_channel(&row.seg);
            for w in self.words_of(row) {
                all.push((
                    channel,
                    w.start_ms,
                    w.id,
                    OwnedWord {
                        start: w.start_ms,
                        end: w.end_ms.max(w.start_ms),
                        speaker: row.seg.speaker_id,
                    },
                ));
            }
        }
        all.sort_by_key(|(_, start, id, _)| (*start, *id));
        let mut out: BTreeMap<Channel, Vec<OwnedWord>> = BTreeMap::new();
        for (channel, _, _, w) in all {
            out.entry(channel).or_default().push(w);
        }
        Ok(out)
    }

    fn update_speaker(&mut self, id: SpeakerId, u: &SpeakerUpdate) -> StoreResult<()> {
        let sp = self
            .speakers
            .get_mut(&id)
            .ok_or_else(|| StoreError::not_found("speaker", id))?;
        if let Some(name) = &u.name {
            sp.display_name = Some(name.clone());
        }
        if let Some((a0, a1)) = u.anchor {
            sp.name_anchor_start_ms = Some(a0);
            sp.name_anchor_end_ms = Some(a1);
        }
        if let Some(m) = u.merged_into {
            sp.merged_into = Some(m);
        }
        if let Some(c) = &u.color {
            sp.color = Some(c.clone());
        }
        if let Some(g) = u.global_id {
            sp.global_id = Some(g);
        }
        sp.needs_review = u.review;
        Ok(())
    }

    fn edits(&mut self, meeting: MeetingId) -> StoreResult<Vec<Edit>> {
        let mut edits: Vec<Edit> = self
            .edits
            .iter()
            .filter(|e| e.meeting_id == meeting)
            .cloned()
            .collect();
        edits.sort_by_key(|e| (e.created_at, e.id));
        Ok(edits)
    }

    fn covered_words(
        &mut self,
        run: RunId,
        channel: Channel,
        start: i64,
        end: i64,
    ) -> StoreResult<Vec<(i64, i64)>> {
        let mut out = Vec::new();
        for sid in self
            .run_segments
            .get(&run)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let Some(row) = self.segments.get(sid) else {
                continue;
            };
            if self.segment_channel(&row.seg) != channel {
                continue;
            }
            for w in self.words_of(row) {
                let mid2 = w.start_ms + w.end_ms;
                if mid2 >= 2 * start && mid2 < 2 * end {
                    out.push((w.id.0, sid.0));
                }
            }
        }
        Ok(out)
    }

    fn segment_words(&mut self, segment: i64) -> StoreResult<(SpeakerId, Vec<i64>)> {
        let row = self.segment(SegmentId(segment))?;
        let ids = self.sorted_words(row).iter().map(|w| w.id.0).collect();
        Ok((row.seg.speaker_id, ids))
    }

    fn regroup(&mut self, segment: i64, groups: &[(Range<usize>, SpeakerId)]) -> StoreResult<()> {
        let seg_id = SegmentId(segment);
        let row = self.segment(seg_id)?;
        let seg = row.seg.clone();
        let words = self.sorted_words(row);
        let n = groups.len();
        let mut rows: Vec<SegRow> = Vec::with_capacity(n);
        for (i, (range, speaker)) in groups.iter().enumerate() {
            let id = if i == 0 {
                seg_id
            } else {
                SegmentId(self.alloc())
            };
            let mut ws: Vec<Word> = words[range.clone()].to_vec();
            for w in &mut ws {
                w.segment_id = id;
            }
            let spans: Vec<(i64, i64)> = ws.iter().map(|w| (w.start_ms, w.end_ms)).collect();
            let (start_ms, end_ms) = group_bounds(i, n, seg.start_ms, seg.end_ms, &spans);
            rows.push(SegRow {
                seg: Segment {
                    id,
                    run_id: seg.run_id,
                    meeting_id: seg.meeting_id,
                    speaker_id: *speaker,
                    start_ms,
                    end_ms,
                    text: join_text(ws.iter().map(|w| w.text.as_str())),
                    seq: seg.seq,
                },
                words: Words::Stored(ws),
            });
        }
        let ids: Vec<SegmentId> = rows.iter().map(|r| r.seg.id).collect();
        for r in rows {
            self.segments.insert(r.seg.id, r);
        }
        let State {
            run_segments,
            segments,
            ..
        } = self;
        let list = run_segments.entry(seg.run_id).or_default();
        let pos = list.iter().position(|s| *s == seg_id).unwrap_or(list.len());
        for (k, id) in ids.iter().enumerate().skip(1) {
            list.insert(pos + k, *id);
        }
        for (i, sid) in list.iter().enumerate().skip(pos) {
            if let Some(row) = segments.get_mut(sid) {
                row.seg.seq = i as i64;
            }
        }
        Ok(())
    }

    fn create_speaker(
        &mut self,
        run: RunId,
        channel: Channel,
        engine_label: &str,
        display_name: Option<&str>,
    ) -> StoreResult<SpeakerId> {
        let meeting_id = State::run(self, run)?.meeting_id;
        let id = SpeakerId(self.alloc());
        self.speakers.insert(
            id,
            Speaker {
                id,
                run_id: run,
                meeting_id,
                channel,
                engine_label: engine_label.to_owned(),
                display_name: display_name.map(str::to_owned),
                color: None,
                global_id: None,
                merged_into: None,
                needs_review: true,
                name_anchor_start_ms: None,
                name_anchor_end_ms: None,
            },
        );
        Ok(id)
    }
}

fn join_words(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Byte ranges of case-insensitive matches of `needle` (already lowercased
/// chars) in `hay`.
fn find_matches(hay: &[(usize, char)], hay_len: usize, needle: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    for start in 0..=hay.len() - needle.len() {
        if hay[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|((_, c), n)| c == n)
        {
            let end_idx = start + needle.len();
            let end = hay.get(end_idx).map(|(b, _)| *b).unwrap_or(hay_len);
            out.push((hay[start].0, end));
        }
    }
    out
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Returns snippet parts if every term matches `text`.
fn match_and_snippet(text: &str, terms: &[Vec<char>]) -> Option<Vec<SnippetPart>> {
    let hay: Vec<(usize, char)> = text.char_indices().map(|(b, c)| (b, lower(c))).collect();
    let mut ranges = Vec::new();
    for term in terms {
        let found = find_matches(&hay, text.len(), term);
        if found.is_empty() {
            return None;
        }
        ranges.extend(found);
    }
    ranges.sort_unstable();
    // Merge overlapping or touching ranges.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    // Window around the first match, on char boundaries.
    let first = merged[0];
    let char_starts: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let first_char = char_starts.partition_point(|&b| b < first.0);
    let last_char = char_starts.partition_point(|&b| b < first.1);
    let win_start_char = first_char.saturating_sub(SNIPPET_BEFORE);
    let win_end_char = (last_char + SNIPPET_AFTER).min(char_starts.len());
    let win_start = char_starts[win_start_char];
    let win_end = char_starts.get(win_end_char).copied().unwrap_or(text.len());

    let mut parts = Vec::new();
    let mut push = |s: &str, hit: bool| {
        if !s.is_empty() {
            parts.push(SnippetPart {
                text: s.to_owned(),
                hit,
            });
        }
    };
    if win_start > 0 {
        push("…", false);
    }
    let mut cursor = win_start;
    for (s, e) in merged {
        if e <= win_start || s >= win_end {
            continue;
        }
        let (s, e) = (s.max(win_start), e.min(win_end));
        push(&text[cursor..s], false);
        push(&text[s..e], true);
        cursor = e;
    }
    push(&text[cursor..win_end], false);
    if win_end < text.len() {
        push("…", false);
    }
    Some(parts)
}

impl MemoryStore {
    /// Writes a transcript into `run`. With `reuse_speakers` (live
    /// materialisation) the run may already hold speaker rows from
    /// `live_speaker`; they are reused, and only segments make it a conflict.
    fn write_transcript(
        &self,
        run: RunId,
        t: &NewTranscript,
        reuse_speakers: bool,
    ) -> StoreResult<()> {
        let mut st = self.lock()?;
        let meeting_id = st.run(run)?.meeting_id;
        let has_rows = st.run_segments.get(&run).is_some_and(|v| !v.is_empty())
            || (!reuse_speakers && st.speakers.values().any(|s| s.run_id == run));
        if has_rows {
            return Err(StoreError::Conflict(format!(
                "run {run} already has a transcript"
            )));
        }
        let mut labels = HashSet::new();
        for s in &t.speakers {
            if !labels.insert((s.channel, s.engine_label.as_str())) {
                return Err(StoreError::Invalid(format!(
                    "duplicate speaker {} on channel {}",
                    s.engine_label, s.channel
                )));
            }
            if let Some(c) = &s.color {
                if !is_hex_color(c) {
                    return Err(StoreError::Invalid(format!("bad colour {c:?}")));
                }
            }
        }
        for (i, seg) in t.segments.iter().enumerate() {
            if seg.speaker >= t.speakers.len() {
                return Err(StoreError::Invalid(format!(
                    "segment {i} refers to speaker index {}",
                    seg.speaker
                )));
            }
            if seg.end_ms < seg.start_ms {
                return Err(StoreError::Invalid(format!(
                    "segment {i} ends before it starts"
                )));
            }
        }
        for (i, w) in t.words.iter().enumerate() {
            if w.segment >= t.segments.len() {
                return Err(StoreError::Invalid(format!(
                    "word {i} refers to segment index {}",
                    w.segment
                )));
            }
        }

        let existing: HashMap<(Channel, String), SpeakerId> = if reuse_speakers {
            st.speakers
                .values()
                .filter(|s| s.run_id == run)
                .map(|s| ((s.channel, s.engine_label.clone()), s.id))
                .collect()
        } else {
            HashMap::new()
        };
        let speaker_ids: Vec<SpeakerId> = t
            .speakers
            .iter()
            .map(|s| {
                if let Some(id) = existing.get(&(s.channel, s.engine_label.clone())) {
                    return *id;
                }
                let id = SpeakerId(st.alloc());
                st.speakers.insert(
                    id,
                    Speaker {
                        id,
                        run_id: run,
                        meeting_id,
                        channel: s.channel,
                        engine_label: s.engine_label.clone(),
                        display_name: s.display_name.clone(),
                        color: s.color.clone(),
                        global_id: None,
                        merged_into: None,
                        needs_review: s.needs_review,
                        name_anchor_start_ms: None,
                        name_anchor_end_ms: None,
                    },
                );
                id
            })
            .collect();
        let segment_ids: Vec<SegmentId> =
            t.segments.iter().map(|_| SegmentId(st.alloc())).collect();
        let mut words_by_segment: Vec<Vec<Word>> = vec![Vec::new(); t.segments.len()];
        for w in &t.words {
            let id = WordId(st.alloc());
            words_by_segment[w.segment].push(Word {
                id,
                segment_id: segment_ids[w.segment],
                meeting_id,
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                text: w.text.clone(),
                confidence: w.confidence,
                raw_label: w.raw_label.clone(),
                margin: w.margin,
                overlap: w.overlap,
                raw_text: w.raw_text.clone(),
            });
        }
        for ((i, seg), words) in t.segments.iter().enumerate().zip(words_by_segment) {
            let id = segment_ids[i];
            st.segments.insert(
                id,
                SegRow {
                    seg: Segment {
                        id,
                        run_id: run,
                        meeting_id,
                        speaker_id: speaker_ids[seg.speaker],
                        start_ms: seg.start_ms,
                        end_ms: seg.end_ms,
                        text: seg.text.clone(),
                        seq: i as i64,
                    },
                    words: Words::Stored(words),
                },
            );
        }
        st.run_segments.insert(run, segment_ids);
        Ok(())
    }
}

/// Sets (trimmed) or clears (blank) a display name with its name anchor.
fn set_speaker_name(
    st: &mut State,
    id: SpeakerId,
    name: &str,
    anchor: Option<(i64, i64)>,
) -> StoreResult<()> {
    let name = name.trim();
    let sp = st
        .speakers
        .get_mut(&id)
        .ok_or_else(|| StoreError::not_found("speaker", id))?;
    if name.is_empty() {
        sp.display_name = None;
        sp.name_anchor_start_ms = None;
        sp.name_anchor_end_ms = None;
    } else {
        sp.display_name = Some(name.to_owned());
        // A rename without an anchor (from the legend) keeps the one an
        // earlier rename set: carry-over still knows where the speaker talks.
        if let Some((start, end)) = anchor {
            sp.name_anchor_start_ms = Some(start);
            sp.name_anchor_end_ms = Some(end);
        }
    }
    Ok(())
}

impl Store for MemoryStore {
    fn list_meetings(&self) -> StoreResult<Vec<MeetingSummary>> {
        let st = self.lock()?;
        let mut meetings: Vec<&Meeting> = st.meetings.values().collect();
        meetings.sort_by_key(|m| (std::cmp::Reverse(m.started_at), std::cmp::Reverse(m.id)));
        Ok(meetings.into_iter().map(|m| st.summary(m)).collect())
    }

    fn get_meeting(&self, id: MeetingId) -> StoreResult<MeetingDetail> {
        let st = self.lock()?;
        let meeting = st.meeting(id)?.clone();
        let current_run = meeting
            .current_run_id
            .and_then(|r| st.runs.get(&r).cloned());
        let speakers = meeting
            .current_run_id
            .map(|r| st.speaker_views(r))
            .unwrap_or_default();
        Ok(MeetingDetail {
            meeting,
            current_run,
            speakers,
        })
    }

    fn get_segments(
        &self,
        id: MeetingId,
        offset: u32,
        limit: u32,
    ) -> StoreResult<Vec<SegmentView>> {
        let st = self.lock()?;
        let meeting = st.meeting(id)?;
        Ok(st
            .current_segments(meeting)
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .filter_map(|sid| st.segments.get(sid))
            .map(|row| st.segment_view(&row.seg))
            .collect())
    }

    fn get_words(&self, id: MeetingId, from_ms: i64, to_ms: i64) -> StoreResult<Vec<Word>> {
        let st = self.lock()?;
        let meeting = st.meeting(id)?;
        let mut words: Vec<Word> = st
            .current_segments(meeting)
            .iter()
            .filter_map(|sid| st.segments.get(sid))
            .filter(|row| row.seg.end_ms > from_ms && row.seg.start_ms < to_ms)
            .flat_map(|row| st.words_of(row))
            .filter(|w| w.end_ms > from_ms && w.start_ms < to_ms)
            .collect();
        words.sort_by_key(|w| (w.start_ms, w.id));
        Ok(words)
    }

    fn search(&self, query: &str, limit: u32) -> StoreResult<Vec<SearchHit>> {
        let terms: Vec<Vec<char>> = query
            .split_whitespace()
            .map(|t| t.chars().map(lower).collect())
            .collect();
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let st = self.lock()?;
        let mut meetings: Vec<&Meeting> = st.meetings.values().collect();
        meetings.sort_by_key(|m| (std::cmp::Reverse(m.started_at), std::cmp::Reverse(m.id)));
        let mut hits = Vec::new();
        for m in meetings {
            for sid in st.current_segments(m) {
                let Some(row) = st.segments.get(sid) else {
                    continue;
                };
                if let Some(snippet) = match_and_snippet(&row.seg.text, &terms) {
                    let root = st.root(row.seg.speaker_id);
                    hits.push(SearchHit {
                        meeting_id: m.id,
                        meeting_title: m.title.clone(),
                        meeting_started_at: m.started_at,
                        segment_id: row.seg.id,
                        start_ms: row.seg.start_ms,
                        speaker_name: st.speakers.get(&root).map(display_name).unwrap_or_default(),
                        snippet,
                    });
                    if hits.len() >= limit as usize {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }

    fn list_audio_tracks(&self, id: MeetingId) -> StoreResult<Vec<AudioTrack>> {
        let st = self.lock()?;
        st.meeting(id)?;
        Ok(st
            .tracks
            .values()
            .filter(|t| t.meeting_id == id)
            .cloned()
            .collect())
    }

    fn playback_track(&self, id: MeetingId) -> StoreResult<Option<AudioTrack>> {
        let source = self.lock()?.meeting(id)?.source;
        Ok(pick_playback_track(source, &self.list_audio_tracks(id)?))
    }

    fn rename_speaker(
        &self,
        id: SpeakerId,
        name: &str,
        anchor_segment: Option<SegmentId>,
    ) -> StoreResult<()> {
        let mut st = self.lock()?;
        let meeting_id = st.speaker(id)?.meeting_id;
        let anchor = match anchor_segment {
            Some(sid) => {
                let seg = &st.segment(sid)?.seg;
                if seg.meeting_id != meeting_id {
                    return Err(StoreError::Invalid(format!(
                        "segment {sid} is not in the speaker's meeting"
                    )));
                }
                Some((seg.start_ms, seg.end_ms))
            }
            None => None,
        };
        set_speaker_name(&mut st, id, name, anchor)
    }

    fn rename_speaker_at(
        &self,
        id: SpeakerId,
        name: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> StoreResult<()> {
        if end_ms < start_ms {
            return Err(StoreError::Invalid(format!(
                "name anchor ends before it starts ({start_ms} > {end_ms} ms)"
            )));
        }
        let mut st = self.lock()?;
        set_speaker_name(&mut st, id, name, Some((start_ms, end_ms)))
    }

    fn set_speaker_color(&self, id: SpeakerId, color: &str) -> StoreResult<()> {
        if !is_hex_color(color) {
            return Err(StoreError::Invalid(format!(
                "colour must be #rrggbb, got {color:?}"
            )));
        }
        let mut st = self.lock()?;
        let sp = st
            .speakers
            .get_mut(&id)
            .ok_or_else(|| StoreError::not_found("speaker", id))?;
        sp.color = Some(color.to_ascii_lowercase());
        Ok(())
    }

    fn merge_speakers(&self, from: SpeakerId, into: SpeakerId) -> StoreResult<()> {
        let mut st = self.lock()?;
        let from_run = st.speaker(from)?.run_id;
        let into_run = st.speaker(into)?.run_id;
        if from_run != into_run {
            return Err(StoreError::Invalid(
                "speakers belong to different runs".into(),
            ));
        }
        let target = st.root(into);
        if target == from {
            return Err(StoreError::Invalid(format!(
                "merging speaker {from} into {into} would create a cycle"
            )));
        }
        for s in st.speakers.values_mut() {
            if s.id == from || s.merged_into == Some(from) {
                s.merged_into = Some(target);
            }
        }
        Ok(())
    }

    fn unmerge_speaker(&self, id: SpeakerId) -> StoreResult<()> {
        let mut st = self.lock()?;
        let sp = st
            .speakers
            .get_mut(&id)
            .ok_or_else(|| StoreError::not_found("speaker", id))?;
        sp.merged_into = None;
        Ok(())
    }

    fn mark_reviewed(&self, id: SpeakerId) -> StoreResult<()> {
        let mut st = self.lock()?;
        st.speakers
            .get_mut(&id)
            .ok_or_else(|| StoreError::not_found("speaker", id))?
            .needs_review = false;
        Ok(())
    }

    fn reassign_segment(&self, segment: SegmentId, to: SpeakerId) -> StoreResult<()> {
        let mut st = self.lock()?;
        let seg = st.segment(segment)?.seg.clone();
        if st.speaker(to)?.run_id != seg.run_id {
            return Err(StoreError::Invalid(format!(
                "speaker {to} is not in the segment's run"
            )));
        }
        let channel = st.speaker(seg.speaker_id)?.channel;
        let target = st.speaker(st.root(to))?.clone();
        let edit = Edit {
            id: st.alloc(),
            meeting_id: seg.meeting_id,
            kind: EditKind::Reassign,
            channel,
            start_ms: seg.start_ms,
            end_ms: seg.end_ms,
            payload: ReassignTarget {
                to_display_name: target.display_name,
                to_engine_label: Some(target.engine_label),
            },
            created_at: now_ms(),
        };
        st.edits.push(edit);
        if let Some(row) = st.segments.get_mut(&segment) {
            row.seg.speaker_id = to;
        }
        Ok(())
    }

    fn split_segment(&self, segment: SegmentId, at_word: WordId) -> StoreResult<SegmentId> {
        let mut st = self.lock()?;
        let row = st.segment(segment)?;
        let old = row.seg.clone();
        let words = st.words_of(row);
        let idx = words.iter().position(|w| w.id == at_word).ok_or_else(|| {
            StoreError::Invalid(format!("word {at_word} is not in segment {segment}"))
        })?;
        if idx == 0 {
            return Err(StoreError::Invalid(
                "cannot split before the first word".into(),
            ));
        }
        let new_id = SegmentId(st.alloc());
        let (first, second) = words.split_at(idx);
        let mut second = second.to_vec();
        for w in &mut second {
            w.segment_id = new_id;
        }
        let new_seg = Segment {
            id: new_id,
            run_id: old.run_id,
            meeting_id: old.meeting_id,
            speaker_id: old.speaker_id,
            start_ms: second[0].start_ms,
            end_ms: old.end_ms,
            text: join_words(&second),
            seq: old.seq + 1,
        };
        if let Some(row) = st.segments.get_mut(&segment) {
            row.seg.end_ms = first[first.len() - 1].end_ms;
            row.seg.text = join_words(first);
            row.words = Words::Stored(first.to_vec());
        }
        st.segments.insert(
            new_id,
            SegRow {
                seg: new_seg,
                words: Words::Stored(second),
            },
        );
        let State {
            run_segments,
            segments,
            ..
        } = &mut *st;
        let list = run_segments.entry(old.run_id).or_default();
        let pos = list
            .iter()
            .position(|s| *s == segment)
            .unwrap_or(list.len());
        list.insert(pos + 1, new_id);
        for (i, sid) in list.iter().enumerate().skip(pos + 1) {
            if let Some(row) = segments.get_mut(sid) {
                row.seg.seq = i as i64;
            }
        }
        Ok(new_id)
    }

    fn set_meeting_title(&self, id: MeetingId, title: &str) -> StoreResult<()> {
        let mut st = self.lock()?;
        let m = st.meeting_mut(id)?;
        m.title = title.trim().to_owned();
        m.updated_at = now_ms();
        Ok(())
    }

    fn set_audio_keep(&self, id: MeetingId, keep: bool) -> StoreResult<()> {
        let mut st = self.lock()?;
        let m = st.meeting_mut(id)?;
        m.audio_keep = keep;
        m.updated_at = now_ms();
        Ok(())
    }

    fn delete_meeting(&self, id: MeetingId) -> StoreResult<()> {
        let mut st = self.lock()?;
        st.meetings
            .remove(&id)
            .ok_or_else(|| StoreError::not_found("meeting", id))?;
        let runs: Vec<RunId> = st
            .runs
            .values()
            .filter(|r| r.meeting_id == id)
            .map(|r| r.id)
            .collect();
        for run in runs {
            st.remove_run(run);
        }
        st.speakers.retain(|_, s| s.meeting_id != id);
        st.tracks.retain(|_, t| t.meeting_id != id);
        st.edits.retain(|e| e.meeting_id != id);
        st.anchors.retain(|(m, _, _), _| *m != id);
        Ok(())
    }

    fn get_settings(&self) -> StoreResult<Settings> {
        Ok(Settings(self.lock()?.settings.clone()))
    }

    fn set_setting(&self, key: &str, value: &str) -> StoreResult<()> {
        if key.trim().is_empty() {
            return Err(StoreError::Invalid("setting key must not be empty".into()));
        }
        self.lock()?
            .settings
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn remove_setting(&self, key: &str) -> StoreResult<()> {
        if key.trim().is_empty() {
            return Err(StoreError::Invalid("setting key must not be empty".into()));
        }
        self.lock()?.settings.remove(key);
        Ok(())
    }

    fn list_model_files(&self) -> StoreResult<Vec<ModelFile>> {
        Ok(self.lock()?.sorted_model_files())
    }

    fn sync_model_files(
        &self,
        found: &[ScannedModelFile],
        now_ms: i64,
    ) -> StoreResult<Vec<ModelFile>> {
        let mut st = self.lock()?;
        let seen: HashSet<&str> = found.iter().map(|f| f.path.as_str()).collect();
        if seen.len() != found.len() {
            return Err(StoreError::Invalid("duplicate path in scan".into()));
        }
        for f in found {
            st.upsert_model_file(f, now_ms);
        }
        for m in st.model_files.values_mut() {
            if !seen.contains(m.path.as_str()) {
                m.missing = true;
            }
        }
        Ok(st.sorted_model_files())
    }

    fn create_meeting(&self, new: NewMeeting) -> StoreResult<Meeting> {
        let mut st = self.lock()?;
        let now = now_ms();
        let meeting = Meeting {
            id: MeetingId(st.alloc()),
            uid: new_id(),
            title: new.title,
            source: new.source,
            source_path: new.source_path,
            language: new.language,
            diarize_mic: new.diarize_mic,
            started_at: new.started_at,
            ended_at: None,
            duration_ms: None,
            status: new.status,
            current_run_id: None,
            audio_keep: false,
            created_at: now,
            updated_at: now,
        };
        st.meetings.insert(meeting.id, meeting.clone());
        Ok(meeting)
    }

    fn set_meeting_status(&self, id: MeetingId, status: MeetingStatus) -> StoreResult<()> {
        let mut st = self.lock()?;
        let m = st.meeting_mut(id)?;
        m.status = status;
        m.updated_at = now_ms();
        Ok(())
    }

    fn finish_meeting(&self, id: MeetingId, ended_at: i64, duration_ms: i64) -> StoreResult<()> {
        let mut st = self.lock()?;
        let m = st.meeting_mut(id)?;
        m.ended_at = Some(ended_at);
        m.duration_ms = Some(duration_ms);
        m.updated_at = now_ms();
        Ok(())
    }

    fn add_audio_track(&self, track: NewAudioTrack) -> StoreResult<AudioTrack> {
        let mut st = self.lock()?;
        st.meeting(track.meeting_id)?;
        let row = AudioTrack {
            id: TrackId(st.alloc()),
            meeting_id: track.meeting_id,
            channel: track.channel,
            path: track.path,
            format: track.format,
            sample_rate: track.sample_rate,
            channels: track.channels,
            duration_ms: track.duration_ms,
            size_bytes: track.size_bytes,
        };
        st.tracks.insert(row.id, row.clone());
        Ok(row)
    }

    fn create_run(
        &self,
        meeting: MeetingId,
        kind: RunKind,
        engine_info: serde_json::Value,
    ) -> StoreResult<TranscriptRun> {
        let mut st = self.lock()?;
        st.meeting(meeting)?;
        let run = TranscriptRun {
            id: RunId(st.alloc()),
            meeting_id: meeting,
            kind,
            engine_info,
            started_at: now_ms(),
            finished_at: None,
            status: RunStatus::Running,
        };
        st.runs.insert(run.id, run.clone());
        Ok(run)
    }

    fn insert_transcript(&self, run: RunId, t: &NewTranscript) -> StoreResult<()> {
        self.write_transcript(run, t, false)
    }

    fn finish_run(&self, run: RunId, status: RunStatus) -> StoreResult<()> {
        let mut st = self.lock()?;
        let r = st
            .runs
            .get_mut(&run)
            .ok_or_else(|| StoreError::not_found("run", run))?;
        r.status = status;
        r.finished_at = Some(now_ms());
        Ok(())
    }

    fn set_current_run(&self, meeting: MeetingId, run: RunId) -> StoreResult<()> {
        let mut st = self.lock()?;
        if st.run(run)?.meeting_id != meeting {
            return Err(StoreError::Invalid(format!(
                "run {run} does not belong to meeting {meeting}"
            )));
        }
        let m = st.meeting_mut(meeting)?;
        m.current_run_id = Some(run);
        m.updated_at = now_ms();
        Ok(())
    }
}

impl RunStore for MemoryStore {
    fn get_run(&self, run: RunId) -> StoreResult<TranscriptRun> {
        self.lock()?.run(run).cloned()
    }

    fn list_runs(&self, meeting: MeetingId) -> StoreResult<Vec<TranscriptRun>> {
        let st = self.lock()?;
        st.meeting(meeting)?;
        Ok(st
            .runs
            .values()
            .filter(|r| r.meeting_id == meeting)
            .cloned()
            .collect())
    }

    fn delete_run(&self, run: RunId) -> StoreResult<()> {
        let mut st = self.lock()?;
        st.run(run)?;
        st.remove_run(run);
        Ok(())
    }

    fn run_speakers(&self, run: RunId) -> StoreResult<Vec<Speaker>> {
        let mut st = self.lock()?;
        st.run(run)?;
        CarryBackend::speakers(&mut *st, run)
    }

    fn list_edits(&self, meeting: MeetingId) -> StoreResult<Vec<Edit>> {
        let mut st = self.lock()?;
        st.meeting(meeting)?;
        CarryBackend::edits(&mut *st, meeting)
    }

    /// Applied under the store's lock. The rules validate both runs before
    /// changing anything; nothing after that point can fail on valid state.
    fn carry_over(&self, from_run: RunId, to_run: RunId) -> StoreResult<CarryOverReport> {
        let mut st = self.lock()?;
        carry_over(&mut *st, from_run, to_run)
    }

    fn carry_over_identity(&self, from_run: RunId, to_run: RunId) -> StoreResult<CarryOverReport> {
        let mut st = self.lock()?;
        carry_over_identity(&mut *st, from_run, to_run)
    }

    fn append_live_utterance(
        &self,
        run: RunId,
        channel: Channel,
        utt: u32,
        rev: u32,
        words: &[EngineWord],
    ) -> StoreResult<i64> {
        let row = live_row(words);
        let mut st = self.lock()?;
        st.run(run)?;
        if st
            .live
            .iter()
            .any(|u| u.run_id == run && u.channel == channel && u.utt == utt && u.rev == rev)
        {
            return Err(StoreError::Conflict(format!(
                "utterance {utt} rev {rev} on {channel} already stored for run {run}"
            )));
        }
        let id = st.alloc();
        st.live.push(LiveUtterance {
            id,
            run_id: run,
            channel,
            utt,
            rev,
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            raw_label: row.raw_label,
            words: row.words.iter().map(|w| w.to_engine()).collect(),
        });
        Ok(id)
    }

    fn list_live_utterances(&self, run: RunId) -> StoreResult<Vec<LiveUtterance>> {
        let st = self.lock()?;
        st.run(run)?;
        Ok(st
            .live
            .iter()
            .filter(|u| u.run_id == run)
            .cloned()
            .collect())
    }

    fn materialise_live_run_except(
        &self,
        run: RunId,
        opts: &SegmentOptions,
        skip: &dyn Fn(&LiveUtterance, usize) -> bool,
    ) -> StoreResult<()> {
        let transcript = {
            let st = self.lock()?;
            st.run(run)?;
            let rows: Vec<LiveUtterance> = st
                .live
                .iter()
                .filter(|u| u.run_id == run)
                .cloned()
                .collect();
            transcript_from_live(&latest_revisions(&rows), opts, skip)
        };
        // Conflict (segments already there) and validation as for any insert;
        // speaker rows from `live_speaker` are kept.
        self.write_transcript(run, &transcript, true)
    }

    fn live_speaker(
        &self,
        run: RunId,
        channel: Channel,
        engine_label: &str,
    ) -> StoreResult<Speaker> {
        let mut st = self.lock()?;
        let r = st.run(run)?;
        if r.kind != RunKind::Live {
            return Err(StoreError::Invalid(format!("run {run} is not a live run")));
        }
        let meeting_id = r.meeting_id;
        if st.run_segments.get(&run).is_some_and(|v| !v.is_empty()) {
            return Err(StoreError::Conflict(format!(
                "run {run} is already materialised"
            )));
        }
        if let Some(s) = st
            .speakers
            .values()
            .find(|s| s.run_id == run && s.channel == channel && s.engine_label == engine_label)
        {
            return Ok(s.clone());
        }
        let id = SpeakerId(st.alloc());
        let row = Speaker {
            id,
            run_id: run,
            meeting_id,
            channel,
            engine_label: engine_label.to_owned(),
            display_name: None,
            color: None,
            global_id: None,
            merged_into: None,
            needs_review: false,
            name_anchor_start_ms: None,
            name_anchor_end_ms: None,
        };
        st.speakers.insert(id, row.clone());
        Ok(row)
    }

    fn delete_audio_track(&self, track: TrackId) -> StoreResult<()> {
        self.lock()?
            .tracks
            .remove(&track)
            .map(|_| ())
            .ok_or_else(|| StoreError::not_found("audio track", track))
    }

    fn set_run_engine_info(&self, run: RunId, engine_info: serde_json::Value) -> StoreResult<()> {
        self.lock()?
            .runs
            .get_mut(&run)
            .ok_or_else(|| StoreError::not_found("run", run))?
            .engine_info = engine_info;
        Ok(())
    }

    fn echo_dropped(&self, run: RunId) -> StoreResult<Option<Vec<EchoDroppedWord>>> {
        let st = self.lock()?;
        st.run(run)?;
        Ok(st.echo_dropped.get(&run).cloned())
    }

    fn set_echo_dropped(&self, run: RunId, words: &[EchoDroppedWord]) -> StoreResult<()> {
        let mut st = self.lock()?;
        st.run(run)?;
        st.echo_dropped.insert(run, words.to_vec());
        Ok(())
    }

    fn add_clock_anchors(&self, anchors: &[ClockAnchor]) -> StoreResult<()> {
        let mut st = self.lock()?;
        for a in anchors {
            st.meeting(a.meeting_id)?;
        }
        for a in anchors {
            st.anchors
                .insert((a.meeting_id, a.channel, a.sample_index), a.qpc_100ns);
        }
        Ok(())
    }

    fn clock_anchors(&self, meeting: MeetingId, channel: Channel) -> StoreResult<Vec<ClockAnchor>> {
        let st = self.lock()?;
        st.meeting(meeting)?;
        Ok(st
            .anchors
            .range((meeting, channel, i64::MIN)..=(meeting, channel, i64::MAX))
            .map(
                |(&(meeting_id, channel, sample_index), &qpc_100ns)| ClockAnchor {
                    meeting_id,
                    channel,
                    sample_index,
                    qpc_100ns,
                },
            )
            .collect())
    }

    fn meeting_id_by_uid(&self, uid: &str) -> StoreResult<Option<MeetingId>> {
        Ok(self
            .lock()?
            .meetings
            .values()
            .find(|m| m.uid == uid)
            .map(|m| m.id))
    }

    fn backup(&self, _dest: &Path) -> StoreResult<()> {
        Err(StoreError::Invalid(
            "an in-memory store has nothing to back up".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_marks_all_terms_case_insensitively() {
        let terms = vec!["budget".chars().collect(), "q3".chars().collect()];
        let parts = match_and_snippet("The Q3 budget is fine, budget again", &terms).unwrap();
        let hits: Vec<&str> = parts
            .iter()
            .filter(|p| p.hit)
            .map(|p| p.text.as_str())
            .collect();
        assert_eq!(hits, ["Q3", "budget", "budget"]);
        let joined: String = parts.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(joined, "The Q3 budget is fine, budget again");
        assert!(match_and_snippet("nothing here", &terms).is_none());
    }

    #[test]
    fn snippet_windows_long_text_on_char_boundaries() {
        let text = format!("{}needle{}", "é".repeat(200), "ü".repeat(200));
        let terms = vec!["NEEDLE".to_lowercase().chars().collect()];
        let parts = match_and_snippet(&text, &terms).unwrap();
        assert_eq!(parts.first().unwrap().text, "…");
        assert_eq!(parts.last().unwrap().text, "…");
        assert!(parts.iter().any(|p| p.hit && p.text == "needle"));
    }

    #[test]
    fn synthetic_words_cover_the_segment() {
        let seg = Segment {
            id: SegmentId(1),
            run_id: RunId(1),
            meeting_id: MeetingId(1),
            speaker_id: SpeakerId(1),
            start_ms: 1_000,
            end_ms: 2_000,
            text: "one two three four".into(),
            seq: 0,
        };
        let words = synthesize_words(&seg, 100);
        assert_eq!(words.len(), 4);
        assert_eq!(words[0].start_ms, 1_000);
        assert_eq!(words[3].end_ms, 2_000);
        assert_eq!(words[2].id, WordId(102));
    }
}
