# Eval data

Status: v0.2, 26 Oct 2026 (folders C and D, the broad and talks groups, the duplicate check); v0.1, 15 Oct 2026.

## Sources (the owner's own recordings, never committed to the repo or uploaded anywhere, because the owner has no right to distribute them; local paths live in tools/eval/sessions.toml, which is gitignored; committed results carry numbers per shorthand label only)

| Folder | Count | Content |
|---|---|---|
| Folder A (private, Aug 2026 archive) | 15 sessions | Zoom recordings, Aug 2026: talks with host intro and Q&A; one in-room multi-speaker session; one logistics presentation with Q&A |
| Folder B (private, Sep 2026) | 8 sessions | Zoom recordings, Sep 2026: same shape, one logistics presentation |
| Folder C (private, Jul 2026) | 5 sessions, 224 min | Zoom recordings, Jul 2026: hackathon sessions, 2 or 3 Zoom labels, 37 to 56 speaker changes each. Given on the first days with A and B but left out of the v0.1 survey; added 26 Oct 2026 |
| Folder D (private, Oct 2026) | 7 sessions, 356 min | Zoom recordings, Oct 2026: research talks with a host introduction and Q&A, 2 to 5 Zoom labels, 36 to 68 speaker changes each; added 26 Oct 2026 |

In all: 35 recordings, 1,676 minutes (A 748, B 348, C 224, D 356).

Each session has: `*.m4a` audio (use this), `*_WxH.mp4` video (ignore), `*.transcript.vtt` (Zoom transcript with speaker names per cue), `*.cc.vtt` (closed captions without speakers), `*newChat.txt`.

## Why the Zoom transcript is a good reference

Zoom attributes speakers from each participant's own audio stream, not from acoustic diarization. Speaker identity per cue is therefore near ground truth. Cue boundaries are coarse (a cue is a phrase, not a word), and overlapping speech is serialized, so references derived from it are good for WDER (words to wrong speaker) and speaker-count checks, and approximate for DER at collar 0.25. Hand-correcting 3 to 5 of them to word-level is Phase 4 work.

## Shape of the data (what it tests)

Mostly one speaker for long stretches (a talk), then dense turn-taking in Q&A. This is the hard case for a diarizer in exactly one place: boundaries at the start and end of the Q&A, short questions, and the host interjecting. That is where WDER near speaker changes is the headline metric. It does not test 4 to 8 simultaneous active speakers; AMI ES2002a covers that.

## Survey (15 Oct 2026, from transcript.vtt: distinct Zoom labels, speaker changes between consecutive cues, m4a minutes)

| Zoom labels | Changes | Min | Session |
|---|---|---|---|
| 6 | 97 | 44 | Aug 2 (talk, 6 labels) |
| 6 | 83 | 77 | Aug 14 (in-room kickoff talks) |
| 3 | 78 | 44 | Aug 9 (talk, 3 labels) |
| 3 | 68 | 35 | Sep 1 |
| 3 | 67 | 43 | Sep 4 |
| 3 | 60 | 52 | Aug 3 |
| 5 | 50 | 48 | Sep 7 |
| 5 | 49 | 45 | Sep 5 |
| 5 | 45 | 47 | Sep 6 |
| 5 | 42 | 55 | Aug 7 |
| 1 | 0 | 46 | Sep Logistics (single speaker) |

Caveat: in the Aug 14 session two labels are shared room devices, so one Zoom label can hide several real speakers. Treat its reference as speaker-count lower bound only.

## Selection for the first eval set (to confirm in Phase 4)

1. Aug 2 (6 labels, 97 changes) and Aug 9 (3 labels, 78 changes): dense Q&A, clean per-person labels. Primary WDER set.
2. Sep 7 and Sep 5 (5 labels each): held-out set after tuning.
3. Sep Logistics (1 speaker, 46 min): false-speaker control. The diarizer must report one speaker.
4. Aug 14 (in-room kickoff talks): in-room multi-speaker stress test, reported separately because the reference is coarse.
5. AMI ES2002a Mix-Headset (public, 4 speakers, reference RTTM): dense-overlap control and the only one with a word-level reference.

`ffprobe` and `ffmpeg` are available on this machine via Chocolatey for the eval harness; the shipped app does not depend on them.

## Pipeline

`tools/eval` (Python, dev-only): parse transcript.vtt to SegLST and RTTM, run the app's `diarist-cli transcribe` on the m4a, export per-word JSON and RTTM, compute DER (collar 0.25 and 0), JER, speaker-count error, WDER overall and within 1 s of a reference speaker change, cpWER and tcpWER via meeteval. Results table committed to `docs/eval/`.

First configs to compare: Nemotron 3.5 (default) versus Nemotron EN on word coverage and cpWER. The runtime's `--verbatim` flag does not bring the dropped fillers back: in nemo-speech 0.2.0 it only skips inverse text normalisation, and with no ITN grammars configured its output is identical to the default on AMI, Aug 2 and Sep 5 (`docs/research/12-accuracy-levers.md` section 6). The Zoom captions are machine output, so word accuracy is judged only on AMI ES2002a; the Zoom set judges speaker attribution.

## Groups (26 Oct 2026)

| Group | Sessions | Minutes | Use |
|---|---|---|---|
| Gate (`tuning`, `heldout`, `control`, `stress`, `overlap`) | Aug 2, Aug 9, Sep 5, Sep 7, Sep Logistics, Aug 14, AMI ES2002a | about 324 | Every change, every release; unchanged since v0.1, so results compare across releases |
| `broad` | the other 12 of A, 5 of B and all 5 of C (labels "Aug N", "Sep N", "Jul N" by recording date) | about 1,014 | Rare events that a small set cannot show (stretches of speech decoded as nothing, deletions), and the release confirmation run |
| `talks` | the 7 of D ("Talk 1" to "Talk 7") | 356 | Held out from all tuning; reported separately |

The beta.4 baseline, the shipped configuration on every group, is `docs/eval/2026-10-26-baseline-beta4.md`.

Duplicate check, 26 Oct 2026, over all 35 recordings: no two `*.m4a` files have the same SHA-256, no Zoom recording ID (`GMT<date>-<time>`) appears twice, no two durations are within 1 s, and no pair of transcripts shares 2 percent or more of its 8-word runs (measured against the shorter transcript). Each recording counts once.
