//! Hard-coded meetings for the Phase 0 shell. Delete once `meet_core::db` serves
//! the real meeting list (Phase 1/2).

use meet_core::{Meeting, MeetingStatus, Source};
use serde_json::json;

const MIN: i64 = 60_000;

pub fn meetings() -> Vec<Meeting> {
    vec![
        meeting(
            "01M4XHD730F5H0D5FN7KSFWCWC",
            "AMI ES2002a (sample)",
            Source::File,
            Some("data/samples/ES2002a.Mix-Headset.wav"),
            1_791_992_700_000, // 2026-10-14 15:45 UTC
            21 * MIN + 13_000,
            MeetingStatus::Processing,
        ),
        meeting(
            "01M4R35N40EDQTDS4YYJEYE4T9",
            "Design review: onboarding flow",
            Source::Live,
            None,
            1_791_810_000_000, // 2026-10-12 13:00 UTC
            32 * MIN + 40_000,
            MeetingStatus::Done,
        ),
        meeting(
            "01M4DA4CT03NDT3GM5GW3TJMEN",
            "Weekly product sync",
            Source::File,
            Some("C:/Users/me/Recordings/weekly-sync.m4a"),
            1_791_448_200_000, // 2026-10-08 08:30 UTC
            64 * MIN + 5_000,
            MeetingStatus::Done,
        ),
    ]
}

fn meeting(
    id: &str,
    title: &str,
    source: Source,
    source_path: Option<&str>,
    started_at: i64,
    duration_ms: i64,
    status: MeetingStatus,
) -> Meeting {
    let ended_at = started_at + duration_ms;
    let done = status == MeetingStatus::Done;
    Meeting {
        id: id.to_owned(),
        title: title.to_owned(),
        source,
        source_path: source_path.map(str::to_owned),
        started_at,
        ended_at: Some(ended_at),
        duration_ms: Some(duration_ms),
        status,
        engine_info: done.then(|| {
            json!({
                "runtime": "nemo-speech 0.2.0",
                "asr": "parakeet-tdt-0.6b-v3",
                "diarizer": "nemotron-3-diarization",
                "fixture": true
            })
        }),
        created_at: started_at,
        updated_at: ended_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_fixtures_with_unique_ulid_ids() {
        let list = meetings();
        assert_eq!(list.len(), 3);
        let mut ids: Vec<_> = list.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| id.len() == 26));
        // Newest first, like the real meeting list will be.
        assert!(list.windows(2).all(|w| w[0].started_at > w[1].started_at));
    }
}
