//! Fusion on real runtime output (fixtures captured in the spike, AMI
//! ES2002a 5-minute clip): Parakeet words without speakers, re-tagged with the
//! standalone `diarize` segments, must agree with the speakers the Nemotron
//! streaming pass tagged on the same words (above 95 percent).

use std::collections::HashMap;

use meet_core::fusion::{assign_words, AsrWord, DiarSegment};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ms(v: &Value) -> i64 {
    (v.as_f64().unwrap() * 1_000.0).round() as i64
}

fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Pairs words with the same normalized text whose starts are within 1 s,
/// greedily by time distance; returns (index in a, index in b). (Streaming
/// Nemotron word times run about 320 ms, up to 700 ms, behind Parakeet's.)
fn shared_words(a: &[(i64, String)], b: &[(i64, String)]) -> Vec<(usize, usize)> {
    let mut used = vec![false; b.len()];
    let mut pairs = Vec::new();
    for (i, (ta, wa)) in a.iter().enumerate() {
        if wa.is_empty() {
            continue;
        }
        let best = b
            .iter()
            .enumerate()
            .filter(|(j, (tb, wb))| !used[*j] && wb == wa && (tb - ta).abs() <= 1_000)
            .min_by_key(|(_, (tb, _))| (tb - ta).abs());
        if let Some((j, _)) = best {
            used[j] = true;
            pairs.push((i, j));
        }
    }
    pairs
}

/// Agreement under the best one-to-one speaker mapping (greedy on counts).
fn agreement(pairs: &[(u32, u32)]) -> f64 {
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for &p in pairs {
        *counts.entry(p).or_default() += 1;
    }
    let mut cells: Vec<((u32, u32), usize)> = counts.into_iter().collect();
    cells.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    let (mut used_a, mut used_b, mut agree) = (Vec::new(), Vec::new(), 0usize);
    for ((a, b), n) in cells {
        if !used_a.contains(&a) && !used_b.contains(&b) {
            used_a.push(a);
            used_b.push(b);
            agree += n;
        }
    }
    agree as f64 / pairs.len() as f64
}

#[test]
fn parakeet_retagged_with_diarize_agrees_with_nemotron() {
    let parakeet = fixture("parakeet_transcribe_5min.json");
    let nemotron = fixture("nemotron_transcribe_diarize_5min.json");
    let diarize = fixture("diarize_5min.json");

    let words: Vec<AsrWord> = parakeet["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| AsrWord {
            start_ms: ms(&w["start"]),
            end_ms: ms(&w["end"]),
            text: w["word"].as_str().unwrap().to_owned(),
            raw_label: None,
        })
        .collect();
    let segments: Vec<DiarSegment> = diarize["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| DiarSegment {
            start_ms: ms(&s["start"]),
            end_ms: ms(&s["end"]),
            speaker: s["speaker"].as_u64().unwrap() as u32,
        })
        .collect();
    let tagged = assign_words(&words, &segments);
    assert_eq!(tagged.len(), words.len());
    assert!(tagged.iter().all(|t| t.diar_speaker.is_some()));

    let nem: Vec<(i64, String, u32)> = nemotron["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                ms(&w["start"]),
                norm(w["word"].as_str().unwrap()),
                w["speaker"].as_u64().unwrap() as u32,
            )
        })
        .collect();
    let a: Vec<(i64, String)> = tagged.iter().map(|t| (t.start_ms, norm(&t.text))).collect();
    let b: Vec<(i64, String)> = nem.iter().map(|(t, w, _)| (*t, w.clone())).collect();
    let pairs = shared_words(&a, &b);
    assert!(
        pairs.len() > 400,
        "only {} shared words of {} / {}",
        pairs.len(),
        a.len(),
        b.len()
    );
    let speakers: Vec<(u32, u32)> = pairs
        .iter()
        .map(|&(i, j)| (tagged[i].diar_speaker.unwrap(), nem[j].2))
        .collect();
    let agree = agreement(&speakers);
    eprintln!(
        "fusion agreement: {:.1}% on {} shared words ({} parakeet, {} nemotron)",
        agree * 100.0,
        pairs.len(),
        a.len(),
        b.len()
    );
    assert!(agree > 0.95, "agreement {:.3}", agree);
}
