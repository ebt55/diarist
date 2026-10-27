//! `diarist-cli punctuate` against a scratch data root with no punctuation
//! model (plan 16 chunk A): it prints the reason and exits 0, and nothing
//! changes.

use std::path::Path;
use std::process::{Command, Output};

use diarist_core::store::{NewMeeting, NewSegment, NewSpeaker, NewTranscript, NewWord};
use diarist_core::store::{RunStore, SqliteStore, Store};
use diarist_core::types::{Channel, MeetingId, MeetingStatus, RunKind, RunStatus, Source};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_diarist-cli"))
        .arg("--data-dir")
        .arg(root)
        .args(args)
        .env_remove("DIARIST_DATA_DIR")
        .output()
        .expect("run diarist-cli")
}

/// A finished import with three words and no punctuation.
fn meeting(root: &Path) -> MeetingId {
    let store = SqliteStore::open(root.join("diarist.db")).unwrap();
    let m = store
        .create_meeting(NewMeeting {
            title: "Fixture".into(),
            source: Source::File,
            source_path: None,
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_791_000_000_000,
            status: MeetingStatus::Done,
        })
        .unwrap();
    let run = store
        .create_run(m.id, RunKind::File, serde_json::json!({"runtime": "fake"}))
        .unwrap();
    let word = |i: i64, text: &str| NewWord {
        segment: 0,
        start_ms: i * 400,
        end_ms: i * 400 + 300,
        text: text.into(),
        confidence: None,
        raw_label: Some("spk_0".into()),
        margin: None,
        overlap: false,
        raw_text: None,
    };
    store
        .insert_transcript(
            run.id,
            &NewTranscript {
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
                    end_ms: 1_100,
                    text: "so we start".into(),
                }],
                words: vec![word(0, "so"), word(1, "we"), word(2, "start")],
            },
        )
        .unwrap();
    store.finish_run(run.id, RunStatus::Done).unwrap();
    store.set_current_run(m.id, run.id).unwrap();
    m.id
}

#[test]
fn without_a_model_punctuate_prints_the_reason_and_exits_0() {
    let root = tempfile::tempdir().unwrap();
    let m = meeting(root.path());
    let id = m.0.to_string();
    for args in [
        vec!["punctuate", id.as_str()],
        vec!["punctuate", "--all-missing"],
    ] {
        let out = cli(root.path(), &args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert!(stdout.contains("reason: no_model"), "{args:?}: {stdout}");
    }
    // Remove on a transcript without punctuation changes nothing either.
    let out = cli(root.path(), &["punctuate", "--remove", &id]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
    let store = SqliteStore::open(root.path().join("diarist.db")).unwrap();
    assert_eq!(store.list_runs(m).unwrap().len(), 1);
    // A meeting that does not exist is an error; no target is a usage error.
    assert_eq!(
        cli(root.path(), &["punctuate", "999"]).status.code(),
        Some(1)
    );
    assert_eq!(cli(root.path(), &["punctuate"]).status.code(), Some(2));
    // --remove works on one meeting; with --all-missing it is refused.
    let out = cli(root.path(), &["punctuate", "--all-missing", "--remove"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--remove")
            && stderr.contains("--all-missing")
            && stderr.contains("cannot be used with"),
        "{stderr}"
    );
    assert_eq!(store.list_runs(m).unwrap().len(), 1);
}
