//! The live transcript so far, for a view that opens after words arrived.

use std::collections::BTreeMap;

use super::{LiveWord, WordsUpdate};
use crate::store::{RunStore, StoreResult};
use crate::transcript::{ME_LABEL, UNKNOWN_LABEL};
use crate::types::{Channel, LiveUtterance, MeetingId, RunKind, SpeakerId};

/// The newest revision of every utterance of the meeting's latest live run,
/// in `live:words` shape and utterance order per channel (empty without a
/// live run). Labels and speaker rows follow the recorder's rules: an
/// undiarized mic is `me`, untagged loopback words `spk_unknown`. Every
/// stored word is in it; [`snapshot_except`] leaves out what the live echo
/// gate hides.
pub fn snapshot(store: &dyn RunStore, meeting: MeetingId) -> StoreResult<Vec<WordsUpdate>> {
    snapshot_except(store, meeting, &|_, _, _, _| false)
}

/// [`snapshot`] without the words for which `hide(channel, utt, rev, word
/// index)` is true: the running recorder's live echo gate
/// ([`super::Recorder::echo_hidden`]), so a view that opens mid-recording
/// shows what the sink showed. An utterance whose words are all hidden
/// stays, empty, as the sink sent it.
pub fn snapshot_except(
    store: &dyn RunStore,
    meeting: MeetingId,
    hide: &dyn Fn(Channel, u32, u32, usize) -> bool,
) -> StoreResult<Vec<WordsUpdate>> {
    let runs = store.list_runs(meeting)?;
    let Some(run) = runs.iter().rev().find(|r| r.kind == RunKind::Live) else {
        return Ok(Vec::new());
    };
    let diarize_mic = store.get_meeting(meeting)?.meeting.diarize_mic;
    let speakers: BTreeMap<(Channel, String), SpeakerId> = store
        .run_speakers(run.id)?
        .into_iter()
        .map(|s| ((s.channel, s.engine_label), s.id))
        .collect();
    let mut latest: BTreeMap<(Channel, u32), LiveUtterance> = BTreeMap::new();
    for u in store.list_live_utterances(run.id)? {
        let k = (u.channel, u.utt);
        if latest.get(&k).is_none_or(|old| u.rev >= old.rev) {
            latest.insert(k, u);
        }
    }
    Ok(latest
        .into_values()
        .map(|u| {
            let words = u
                .words
                .iter()
                .enumerate()
                .filter(|(i, _)| !hide(u.channel, u.utt, u.rev, *i))
                .map(|(_, w)| {
                    let label = if u.channel == Channel::Mic && !diarize_mic {
                        ME_LABEL.to_owned()
                    } else {
                        w.spk.clone().unwrap_or_else(|| {
                            if u.channel == Channel::Mic {
                                ME_LABEL
                            } else {
                                UNKNOWN_LABEL
                            }
                            .to_owned()
                        })
                    };
                    LiveWord {
                        start_ms: (w.t0_us / 1_000) as i64,
                        end_ms: (w.t1_us / 1_000) as i64,
                        text: w.text.clone(),
                        speaker_id: speakers.get(&(u.channel, label.clone())).copied(),
                        label,
                    }
                })
                .collect();
            WordsUpdate {
                meeting_id: meeting,
                run_id: run.id,
                channel: u.channel,
                utt: u.utt,
                rev: u.rev,
                words,
            }
        })
        .collect())
}
