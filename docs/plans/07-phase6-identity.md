# Phase 6: cross-meeting speaker identity (v3)

Status: draft. Depends on Phase 5. Decisions from docs/research/04 section 4.

## Goal

When a post-pass finishes, the app proposes "Speaker 2 sounds like Alice (from 3 earlier meetings)". The user confirms or rejects. Confirmed names apply instantly; the voiceprint improves over time. Everything runs on CPU.

## Design

- Embedding model: TitaNet-L (CC-BY-4.0, EER 0.66) in ONNX via sherpa-onnx's `SpeakerEmbeddingExtractor` (C API through the `sherpa-rs` or hand-written bindings; CPU provider). A/B WeSpeaker ResNet293-LM behind a setting.
- Crops: for each speaker in the current run, pick up to ten 2 to 6 s windows where that speaker's words are contiguous and no other speaker has words within 300 ms (from the diarize segments; from `diar_frame_probs` once the FFI path exists). Embed each, L2-normalize, average.
- Match: cosine against `global_speakers` voiceprints (`speaker_embeddings` averaged per global speaker). Threshold calibrated on the eval set (same person across the two Justin Shenk sessions, different people elsewhere). Show a proposal only above the threshold; never auto-apply.
- Confirm: link `speakers.global_id`, append the new embedding to the voiceprint. Reject: remember the negative pair for this meeting.
- Storage: `speaker_embeddings.vector` as float32 BLOB; brute-force cosine is fine below 10,000 voiceprints; sqlite-vec statically linked only if that limit is reached.

## Owns

`crates/core/src/identity`, `apps/desktop` (proposal UI in the speaker legend, People page), `tools/eval` (identity calibration script).

## Acceptance

- Same person across two different meetings matched above threshold; different people below, on the eval set.
- Embedding 20 crops takes under 2 s on the CPU.
- No VRAM used.
