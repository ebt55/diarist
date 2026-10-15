# Eval data

Status: v0.1, 15 Oct 2026.

## Sources (user's own recordings, never committed to the repo)

| Folder | Count | Content |
|---|---|---|
| `C:\Users\ebin\Documents\digital-minds-meet-aug2026-archive` | 15 sessions | Zoom recordings, Aug 2026: talks with host intro and Q&A; one multi-speaker "CIMC House kickoff talks"; one "Logistics Presentation + Q&A" |
| `C:\Users\ebin\glm-ground\meets` | 8 sessions | Zoom recordings, Sep 2026: same shape, one "Logistics Presentation" |

Each session has: `*.m4a` audio (use this), `*_WxH.mp4` video (ignore), `*.transcript.vtt` (Zoom transcript with speaker names per cue), `*.cc.vtt` (closed captions without speakers), `*newChat.txt`.

## Why the Zoom transcript is a good reference

Zoom attributes speakers from each participant's own audio stream, not from acoustic diarization. Speaker identity per cue is therefore near ground truth. Cue boundaries are coarse (a cue is a phrase, not a word), and overlapping speech is serialized, so references derived from it are good for WDER (words to wrong speaker) and speaker-count checks, and approximate for DER at collar 0.25. Hand-correcting 3 to 5 of them to word-level is Phase 4 work.

## Shape of the data (what it tests)

Mostly one speaker for long stretches (a talk), then dense turn-taking in Q&A. This is the hard case for a diarizer in exactly one place: boundaries at the start and end of the Q&A, short questions, and the host interjecting. That is where WDER near speaker changes is the headline metric. It does not test 4 to 8 simultaneous active speakers; AMI ES2002a covers that.

## Survey (15 Oct 2026, from transcript.vtt: distinct Zoom labels, speaker changes between consecutive cues, m4a minutes)

| Zoom labels | Changes | Min | Session |
|---|---|---|---|
| 6 | 97 | 44 | Aug: 2 Justin Shenk |
| 6 | 83 | 77 | Aug: 14 CIMC House kickoff talks |
| 3 | 78 | 44 | Aug: 9 Ali Ladak |
| 3 | 68 | 35 | Sep: 1 Justin Shenk |
| 3 | 67 | 43 | Sep: 4 Isaak Mengesha |
| 3 | 60 | 52 | Aug: 3 Soenke Ziesche |
| 5 | 50 | 48 | Sep: 7 Alex Mallen |
| 5 | 49 | 45 | Sep: 5 Stephen Casper |
| 5 | 45 | 47 | Sep: 6 David Krueger |
| 5 | 42 | 55 | Aug: 7 Berlin Kickoffs |
| 1 | 0 | 46 | Sep: Logistics Presentation (single speaker) |

Caveat: in the CIMC session the labels "CIMC" and "Team Apart" are shared room devices, so one Zoom label can hide several real speakers. Treat its reference as speaker-count lower bound only.

## Selection for the first eval set (to confirm in Phase 4)

1. Aug "2 Justin Shenk" (6 labels, 97 changes) and Aug "9 Ali Ladak" (3 labels, 78 changes): dense Q&A, clean per-person labels. Primary WDER set.
2. Sep "7 Alex Mallen" and Sep "5 Stephen Casper" (5 labels each): held-out set after tuning.
3. Sep "Logistics Presentation" (1 speaker, 46 min): false-speaker control. The diarizer must report one speaker.
4. Aug "14 CIMC House kickoff talks": in-room multi-speaker stress test, reported separately because the reference is coarse.
5. AMI ES2002a Mix-Headset (public, 4 speakers, reference RTTM): dense-overlap control and the only one with a word-level reference.

`ffprobe` and `ffmpeg` are available on this machine via Chocolatey for the eval harness; the shipped app does not depend on them.

## Pipeline

`tools/eval` (Python, dev-only): parse transcript.vtt to SegLST and RTTM, run the app's `meet-cli transcribe` on the m4a, export per-word JSON and RTTM, compute DER (collar 0.25 and 0), JER, speaker-count error, WDER overall and within 1 s of a reference speaker change, cpWER and tcpWER via meeteval. Results table committed to `docs/eval/`.
