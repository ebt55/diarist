# Eval: beta.4 baseline on the public sets (27 Oct 2026)

The companion of `docs/eval/2026-10-26-baseline-beta4.md`: the same shipped configuration of Diarist 0.1.0-beta.4, measured on the seven public sets of docs/05 "Extend" and `docs/research/20-public-eval-data.md`. That is 98 sessions and 32.4 hours, every reference made by people. Shipped configuration only: NVIDIA's published q8_0 GGUFs at the manifest revisions, nemo-speech 0.2.0, and the app's file-pass settings (lookahead 13, `v3-offline` diarizer preset, `--stream --endpointing --diarize`). Nemotron 3.5 ran all 98 sessions. Nemotron EN, the default model, ran the 52 English ones (it does not cover zh, fr or hi). Numbers are percentages unless a column says otherwise.

On the Zoom sessions the reference is Zoom's own recogniser. Here every reference is human-made, so cpWER, tcpWER and WER measure word accuracy. The sets are held out (research/20 section 3): report them, never tune on them.

## Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 10.0.26200, RTX 3080 10 GB (WDDM), driver 610.88; GPU memory before each run 1,240 to 2,051 MiB (the desktop and browser) |
| diarist-cli | 0.1.0-beta.4, built with `cargo build --release --locked -p diarist-cli` from this branch (`claude/eval-public-data` at `3fcdae4`; `crates/`, `Cargo.lock` and `manifests/` are identical to the beta.4 tree `fc597ba`), SHA-256 `a016b1e725b3973fad36c969572bca22bbceffcedc696bea73dd6adbb18668c4` (`cli_sha256` in every run.json). A fresh build of the same source in another worktree, so its hash differs from the Zoom baseline's binary (`d736a0ae…`) |
| Engine | nemo-speech 0.2.0 (`engine_version` in every run.json), package `nemo-speech-0.2.0-windows-x86_64-cuda`: a byte copy of the installed app's engine (79 files, every SHA-256 equal; `nemo-speech.exe` `d9527ade84d41be92b4b92d406fd3e36aaf71e2458c2fbd0ab8e42cc98043272`). The app's folder under `%LOCALAPPDATA%\app.diarist.desktop` was only read |
| ASR, EN | `asr_file` `nemotron-speech-streaming-en-0.6b.q8_0`, `nvidia/nemotron-speech-streaming-en-0.6b` @ `ebe59e5a8171`, SHA-256 `d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d` |
| ASR, 3.5 | `asr_file` `nemotron-3.5-asr-streaming-0.6b.q8_0`, `nvidia/nemotron-3.5-asr-streaming-0.6b` @ `1c8deaecc64b`, SHA-256 `a5c435f294eea8f88ce68dd27b8c3bfea7f777cb2fbba04fcd30eaa555f429ae` |
| Diarizer | `diarizer_file` `Nemotron-3-Diarization.q8_0`, `nvidia/Nemotron-3-Diarization` @ `f667ed73aee5`, SHA-256 `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1` |
| Model files | Copies of the installed app's three GGUFs; every SHA-256 above was hashed on the copy and is equal to `manifests/models.json` |
| Settings recorded in run.json (all 150 runs) | `right_context` 13, `diar_preset` v3-offline, `device` cuda:0, `engine_args` empty (the app's own defaults from the manifest), `diar_pass` false; `language` en on the English sessions, zh-CN, fr or hi on the others |
| Engine command (from engine.log) | `nemo-speech transcribe <wav> --model <asr.gguf> [--language en, zh-CN, fr or hi: 3.5 only] --stream --endpointing --asr.streaming.rnnt_right_context 13 --diarize --diar-model <Nemotron-3-Diarization.q8_0.gguf> --asr.diar.preset v3-offline --device cuda:0 --json --output <out> --force`. Five distinct command lines over the 150 runs: EN without a tag (52), 3.5 with `en` (52), `zh-CN` (16), `fr` (10), `hi` (20) |
| Text normaliser | English: whisper EnglishTextNormalizer per token, filler removal, ok to okay (the harness's). French and Hindi: NFKC, casefold, punctuation and symbols removed, combining marks kept; French hesitations (euh, heu, hum, hm, mh, mm, mhm) dropped. Mandarin: the same, scored per character (a run of Latin letters or digits counts as one unit), 嗯 and 呃 dropped |
| Runs | 27 Oct 2026, 04:05 to 08:53 IST, one session per process, through the agents' GPU queue (`gpu-run.sh`, later `gpu-run2.sh`, tiers 2 then 3) with `--no-diar-pass` (no standalone diarize pass; it runs after the transcript is built and never feeds it) |

## Data

| Group | Sessions | Minutes | What it is |
|---|---|---|---|
| pub-meeting | 8 | 373 | AMI EN2002a-d through the Mix-Headset channel (natural meetings, 3 or 4 speakers, 24 to 30 % of speech overlapped; manual words) and ICSI Bed004, Bed016, Bmr019, Bro018 (5 to 10 participants, headset mix; forced-aligned words) |
| pub-far | 36 | 360 | The same four AMI meetings through one table-array microphone (Array1-01, same reference words), and 32 NOTSOFAR-1 eval-small meetings of about 6 minutes on single-channel commercial devices (3 to 7 people; words aligned on close-talk) |
| pub-phone | 8 | 274 | Eight Earnings-22 calls, accented English (India, China, South Korea, Australia, Brazil, Argentina, Russia, Canada), 4 to 13 speakers; forced-aligned words |
| pub-lang | 46 | 938 | AliMeeting Eval (Mandarin, 8 meetings of 2 to 4 people, each as the headset mix and as array channel 0; turn references), SUMM-RE ASRU (French, 10 meetings of 3 or 4 people, 2 on Zoom and 8 in a studio or lab, tracks summed; aligned words), Indic DiarBench Hindi (10 near-field and 10 far-field clips of 5 to 15 minutes, 2 to 4 speakers; turn references) |
| All | 98 | 1,944 | 32.4 hours; 52 English sessions (1,006 minutes) |

Public data only, so sessions are named by their corpus ids. Sources, slices, SHA-256 pins and licences: `docs/research/20-public-eval-data.md` and each set's `tools/eval/.work/public/<set>/meta.json`. Every set is CC BY 4.0 or CC BY-SA 4.0. **Earnings-22 audio:** the dataset's LICENSE.md names only "the transcripts and associated text files that are used for alignment" under CC BY-SA 4.0. It says nothing of the audio Rev ships in the same repository, and the repository has no top-level licence. The calls are public webcasts whose rights sit with the companies (research/20 section 2.3, an inference). The audio is used here for internal evaluation only and is never redistributed. For a strict reading, drop the pub-phone rows (or the set: `earnings22_fetch.py` is independent of the others).

## Metrics

- cpWER, tcpWER (collar 5 s), WDER and DER at collar 0.25 as in every docs/eval report (`tools/eval/metrics.py`). The scored region is each reference's annotated span plus 1 s.
- WER: speaker-agnostic, time-ordered normalised words, Levenshtein; deletions, insertions and substitutions are its counts. Mandarin rows count characters, so their cpWER, tcpWER and WER are character error rates. Where speakers overlap, ordering the reference words by time interleaves the speakers' words, which can cost this WER a little; on a few sessions (ICSI Bed004, Indic hindi_082 and hindi_084) WER is therefore slightly above cpWER.
- Count err: transcript speakers minus reference speakers.
- Gaps: stretches of at least 2 s (and of at least 5 s) without a hypothesis word in which the reference has at least 3 words, counted with a 0.5 s margin at both ends (research/19 section 0, now `metrics.py`). On the turn-level references (AliMeeting, Indic DiarBench) a turn's span is spread over its words, and AliMeeting counts characters, so gaps there are approximate.
- × real time: audio duration over the `nemo-speech transcribe` step's wall time (`asr_wall_s`, model load included). Shown only where the run held quiet.lock (no other agent started heavy work) and every 1-second `nvidia-smi pmon` sample during it showed no process other than nemo-speech.exe or claude.exe above 5 % SM. Otherwise "–".
- Pooled rows sum errors over sessions (cpWER, tcpWER, WER); WDER pools all aligned word pairs, and DER is weighted by scored reference speech. Speed is pooled over the timed sessions only, with their count in brackets when some are missing.

## Results

Speed: 131 of the 150 runs are timed (EN 40 of 52, 3.5 91 of 98). The other 19 overlapped GPU use by desktop apps: Brave (up to 84 % SM, 17 runs), Notion or Windows Explorer. One run (EN, ICSI Bed016) also overlapped a python.exe CUDA process at up to 73 % SM, at 04:20, while this run held gpu.lock. They are left out, not re-run. Untimed, EN: AMI EN2002a, b, c and d Mix-Headset; EN2002a and b Array1-01; ICSI Bed016 and Bmr019; NOTSOFAR MTG_32000, 32006, 32063 and 32071. Untimed, 3.5: AMI EN2002a, b, c and d Mix-Headset; ICSI Bed004; NOTSOFAR MTG_32063; SUMM-RE 027a. Pooled × real time on the timed runs is 77 for EN and 81 for 3.5, the same as on the Zoom sessions (76 and 82). The 6-minute NOTSOFAR sessions run at 66 to 79 because model load is a larger share of a short file.

### pub-meeting: open multi-party meetings, close-talk mix (AMI EN2002 Mix-Headset, ICSI)

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| AMI EN2002a Mix-Headset | EN | 36 | 34.6 | 35.1 | 31.5 | 2134 | 33 | 311 | 6.2 | 34.9 | +0 | 55 | 17 | – |
| AMI EN2002a Mix-Headset | 3.5 | 36 | 38.5 | 39.2 | 35.8 | 2413 | 37 | 370 | 6.5 | 37.0 | +0 | 73 | 18 | – |
| AMI EN2002b Mix-Headset | EN | 30 | 32.8 | 33.2 | 30.1 | 1570 | 45 | 272 | 5.5 | 34.0 | +0 | 44 | 7 | – |
| AMI EN2002b Mix-Headset | 3.5 | 30 | 36.2 | 36.5 | 33.6 | 1732 | 41 | 330 | 6.1 | 35.7 | +0 | 46 | 11 | – |
| AMI EN2002c Mix-Headset | EN | 50 | 27.2 | 27.5 | 24.3 | 2076 | 114 | 503 | 4.9 | 30.6 | +1 | 21 | 3 | – |
| AMI EN2002c Mix-Headset | 3.5 | 50 | 31.4 | 31.8 | 28.7 | 2510 | 90 | 572 | 5.4 | 33.3 | +1 | 45 | 9 | – |
| AMI EN2002d Mix-Headset | EN | 37 | 34.4 | 34.8 | 31.7 | 2230 | 47 | 297 | 5.6 | 39.9 | +0 | 62 | 16 | – |
| AMI EN2002d Mix-Headset | 3.5 | 37 | 38.1 | 38.6 | 35.9 | 2519 | 39 | 354 | 5.6 | 42.6 | +0 | 79 | 15 | – |
| ICSI Bed004 | EN | 57 | 14.1 | 19.6 | 16.0 | 796 | 101 | 485 | 3.8 | 34.9 | +0 | 25 | 11 | 76 |
| ICSI Bed004 | 3.5 | 57 | 16.9 | 22.6 | 18.6 | 990 | 80 | 531 | 4.3 | 35.5 | +0 | 34 | 11 | – |
| ICSI Bed016 | EN | 47 | 45.3 | 47.5 | 43.5 | 2771 | 82 | 282 | 8.4 | 43.6 | -2 | 92 | 54 | – |
| ICSI Bed016 | 3.5 | 47 | 44.6 | 46.6 | 42.9 | 2718 | 65 | 311 | 7.0 | 40.9 | -2 | 96 | 45 | 77 |
| ICSI Bmr019 | EN | 60 | 35.6 | 37.2 | 34.0 | 4279 | 81 | 434 | 5.1 | 41.9 | -1 | 97 | 44 | – |
| ICSI Bmr019 | 3.5 | 60 | 34.5 | 36.4 | 33.0 | 4040 | 49 | 568 | 5.7 | 38.5 | -1 | 91 | 15 | 79 |
| ICSI Bro018 | EN | 57 | 17.6 | 17.9 | 16.6 | 1181 | 92 | 332 | 1.4 | 12.3 | +0 | 9 | 2 | 78 |
| ICSI Bro018 | 3.5 | 57 | 21.4 | 21.5 | 20.4 | 1584 | 56 | 336 | 1.4 | 13.2 | +0 | 26 | 11 | 76 |

### pub-far: distant single microphone, English (AMI EN2002 Array1-01, NOTSOFAR-1)

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| AMI EN2002a Array1-01 | EN | 36 | 40.1 | 40.7 | 37.7 | 2555 | 28 | 384 | 6.8 | 38.7 | +0 | 83 | 25 | – |
| AMI EN2002a Array1-01 | 3.5 | 36 | 43.8 | 44.4 | 41.3 | 2755 | 25 | 472 | 7.3 | 39.8 | +0 | 88 | 26 | 73 |
| AMI EN2002b Array1-01 | EN | 30 | 40.1 | 41.1 | 37.2 | 1885 | 34 | 413 | 7.5 | 39.4 | +0 | 55 | 18 | – |
| AMI EN2002b Array1-01 | 3.5 | 30 | 43.7 | 44.8 | 40.9 | 2037 | 41 | 484 | 8.4 | 40.4 | +0 | 64 | 19 | 77 |
| AMI EN2002c Array1-01 | EN | 50 | 34.5 | 35.0 | 30.2 | 2607 | 100 | 640 | 6.5 | 33.7 | +1 | 55 | 9 | 81 |
| AMI EN2002c Array1-01 | 3.5 | 50 | 38.7 | 39.1 | 35.0 | 3066 | 65 | 740 | 7.2 | 36.6 | +1 | 78 | 16 | 80 |
| AMI EN2002d Array1-01 | EN | 37 | 41.5 | 42.2 | 39.2 | 2758 | 32 | 389 | 6.6 | 43.6 | +0 | 94 | 29 | 74 |
| AMI EN2002d Array1-01 | 3.5 | 37 | 45.9 | 46.6 | 43.4 | 2974 | 37 | 513 | 7.3 | 45.5 | +0 | 103 | 27 | 80 |
| NOTSOFAR MTG_32000 sc_meetup_0 | EN | 6 | 37.3 | 37.4 | 35.4 | 462 | 9 | 36 | 4.7 | 17.1 | +0 | 13 | 0 | – |
| NOTSOFAR MTG_32000 sc_meetup_0 | 3.5 | 6 | 40.2 | 40.3 | 37.6 | 474 | 11 | 54 | 6.7 | 15.3 | +0 | 7 | 0 | 76 |
| NOTSOFAR MTG_32006 sc_rockfall_2 | EN | 6 | 37.5 | 38.0 | 31.9 | 453 | 16 | 60 | 10.1 | 23.1 | +0 | 10 | 0 | – |
| NOTSOFAR MTG_32006 sc_rockfall_2 | 3.5 | 6 | 39.7 | 40.3 | 34.5 | 495 | 15 | 63 | 10.1 | 21.7 | +0 | 10 | 0 | 66 |
| NOTSOFAR MTG_32020 sc_plaza_0 | EN | 6 | 31.9 | 32.6 | 28.8 | 350 | 14 | 44 | 7.1 | 12.8 | +0 | 6 | 0 | 66 |
| NOTSOFAR MTG_32020 sc_plaza_0 | 3.5 | 6 | 34.2 | 34.7 | 30.8 | 359 | 17 | 61 | 7.2 | 11.2 | +0 | 4 | 0 | 69 |
| NOTSOFAR MTG_32024 sc_studio_0 | EN | 6 | 54.3 | 57.5 | 33.5 | 448 | 12 | 36 | 21.5 | 31.5 | +1 | 7 | 1 | 69 |
| NOTSOFAR MTG_32024 sc_studio_0 | 3.5 | 6 | 55.4 | 57.0 | 34.5 | 434 | 6 | 71 | 21.3 | 28.7 | +1 | 6 | 0 | 75 |
| NOTSOFAR MTG_32029 sc_rockfall_1 | EN | 6 | 49.8 | 50.5 | 44.8 | 587 | 5 | 66 | 13.3 | 29.5 | +0 | 18 | 6 | 68 |
| NOTSOFAR MTG_32029 sc_rockfall_1 | 3.5 | 6 | 50.1 | 51.0 | 45.2 | 611 | 2 | 50 | 13.4 | 25.0 | +0 | 16 | 4 | 76 |
| NOTSOFAR MTG_32042 sc_studio_0 | EN | 6 | 50.9 | 54.0 | 49.4 | 601 | 21 | 82 | 10.7 | 23.5 | +0 | 19 | 3 | 75 |
| NOTSOFAR MTG_32042 sc_studio_0 | 3.5 | 6 | 50.9 | 51.6 | 48.4 | 613 | 9 | 67 | 8.0 | 18.9 | +0 | 12 | 4 | 76 |
| NOTSOFAR MTG_32049 sc_rockfall_1 | EN | 6 | 68.8 | 80.1 | 43.9 | 493 | 14 | 47 | 33.7 | 51.6 | +1 | 21 | 4 | 73 |
| NOTSOFAR MTG_32049 sc_rockfall_1 | 3.5 | 6 | 64.6 | 75.6 | 40.0 | 414 | 9 | 82 | 33.2 | 44.6 | +1 | 11 | 1 | 73 |
| NOTSOFAR MTG_32063 sc_rockfall_0 | EN | 6 | 45.0 | 45.3 | 41.8 | 543 | 5 | 25 | 6.5 | 28.1 | +1 | 17 | 4 | – |
| NOTSOFAR MTG_32063 sc_rockfall_0 | 3.5 | 6 | 41.7 | 42.0 | 38.4 | 465 | 9 | 52 | 6.5 | 17.8 | +1 | 6 | 1 | – |
| NOTSOFAR MTG_32071 sc_meetup_0 | EN | 6 | 41.2 | 41.8 | 39.1 | 464 | 10 | 75 | 6.3 | 20.4 | +0 | 14 | 1 | – |
| NOTSOFAR MTG_32071 sc_meetup_0 | 3.5 | 6 | 42.1 | 42.6 | 39.9 | 470 | 8 | 82 | 6.0 | 19.2 | +0 | 11 | 1 | 74 |
| NOTSOFAR MTG_32075 sc_rockfall_2 | EN | 6 | 38.7 | 39.1 | 35.6 | 444 | 14 | 52 | 6.1 | 20.8 | +0 | 11 | 1 | 73 |
| NOTSOFAR MTG_32075 sc_rockfall_2 | 3.5 | 6 | 38.8 | 39.3 | 34.5 | 405 | 10 | 80 | 8.2 | 16.3 | +0 | 7 | 0 | 76 |
| NOTSOFAR MTG_32085 sc_rockfall_2 | EN | 6 | 65.5 | 66.6 | 59.4 | 897 | 3 | 53 | 20.4 | 47.9 | +0 | 25 | 7 | 66 |
| NOTSOFAR MTG_32085 sc_rockfall_2 | 3.5 | 6 | 64.3 | 65.4 | 58.2 | 835 | 3 | 95 | 20.2 | 42.1 | +0 | 25 | 2 | 73 |
| NOTSOFAR MTG_32090 sc_studio_0 | EN | 6 | 51.2 | 52.4 | 47.8 | 636 | 6 | 65 | 9.5 | 34.8 | +0 | 19 | 2 | 70 |
| NOTSOFAR MTG_32090 sc_studio_0 | 3.5 | 6 | 51.0 | 51.7 | 48.1 | 620 | 10 | 82 | 10.5 | 27.8 | +0 | 10 | 0 | 75 |
| NOTSOFAR MTG_32101 sc_meetup_0 | EN | 8 | 55.6 | 57.0 | 51.2 | 1000 | 10 | 77 | 10.9 | 31.5 | +0 | 36 | 7 | 77 |
| NOTSOFAR MTG_32101 sc_meetup_0 | 3.5 | 8 | 53.5 | 54.6 | 49.5 | 911 | 12 | 127 | 12.3 | 25.4 | +0 | 25 | 3 | 77 |
| NOTSOFAR MTG_32102 sc_rockfall_2 | EN | 8 | 56.0 | 58.2 | 49.9 | 1029 | 6 | 60 | 15.5 | 44.7 | +0 | 27 | 7 | 76 |
| NOTSOFAR MTG_32102 sc_rockfall_2 | 3.5 | 8 | 56.5 | 59.1 | 48.8 | 960 | 5 | 107 | 18.2 | 40.1 | +0 | 17 | 0 | 77 |
| NOTSOFAR MTG_32105 sc_meetup_0 | EN | 8 | 33.7 | 33.9 | 7.6 | 43 | 11 | 33 | 14.1 | 21.6 | -1 | 1 | 0 | 73 |
| NOTSOFAR MTG_32105 sc_meetup_0 | 3.5 | 8 | 36.4 | 36.5 | 10.3 | 55 | 19 | 44 | 14.4 | 20.6 | -1 | 1 | 0 | 75 |
| NOTSOFAR MTG_32106 sc_plaza_0 | EN | 8 | 48.4 | 49.5 | 41.5 | 819 | 12 | 74 | 12.0 | 29.9 | +0 | 23 | 1 | 77 |
| NOTSOFAR MTG_32106 sc_plaza_0 | 3.5 | 8 | 45.9 | 46.5 | 42.3 | 806 | 9 | 107 | 9.2 | 22.9 | +0 | 18 | 0 | 78 |
| NOTSOFAR MTG_32107 sc_meetup_0 | EN | 8 | 60.0 | 60.3 | 56.5 | 1483 | 4 | 60 | 12.1 | 43.0 | +0 | 32 | 4 | 76 |
| NOTSOFAR MTG_32107 sc_meetup_0 | 3.5 | 8 | 61.0 | 61.4 | 57.3 | 1458 | 7 | 106 | 14.8 | 39.0 | +0 | 30 | 2 | 77 |
| NOTSOFAR MTG_32108 sc_rockfall_1 | EN | 8 | 58.8 | 59.8 | 56.1 | 1209 | 4 | 48 | 10.7 | 43.8 | +0 | 40 | 11 | 78 |
| NOTSOFAR MTG_32108 sc_rockfall_1 | 3.5 | 8 | 54.4 | 56.6 | 50.5 | 1030 | 5 | 100 | 13.7 | 32.7 | +0 | 21 | 2 | 78 |
| NOTSOFAR MTG_32109 sc_plaza_0 | EN | 8 | 22.6 | 23.1 | 20.8 | 198 | 27 | 87 | 4.0 | 10.6 | +0 | 6 | 0 | 76 |
| NOTSOFAR MTG_32109 sc_plaza_0 | 3.5 | 8 | 23.1 | 23.5 | 20.8 | 238 | 17 | 57 | 3.6 | 9.3 | +0 | 6 | 2 | 79 |
| NOTSOFAR MTG_32110 sc_studio_0 | EN | 8 | 59.4 | 60.4 | 56.2 | 1410 | 7 | 60 | 13.0 | 41.2 | +0 | 34 | 5 | 77 |
| NOTSOFAR MTG_32110 sc_studio_0 | 3.5 | 8 | 58.7 | 59.7 | 55.3 | 1349 | 7 | 97 | 13.4 | 38.4 | +0 | 25 | 4 | 77 |
| NOTSOFAR MTG_32178 sc_meetup_0 | EN | 6 | 40.1 | 41.4 | 37.8 | 399 | 11 | 69 | 5.6 | 22.6 | +0 | 9 | 3 | 75 |
| NOTSOFAR MTG_32178 sc_meetup_0 | 3.5 | 6 | 39.9 | 41.2 | 38.2 | 368 | 9 | 107 | 5.5 | 16.4 | +0 | 7 | 1 | 76 |
| NOTSOFAR MTG_32179 sc_rockfall_0 | EN | 6 | 44.9 | 45.5 | 35.9 | 503 | 8 | 37 | 14.1 | 26.2 | +1 | 9 | 0 | 75 |
| NOTSOFAR MTG_32179 sc_rockfall_0 | 3.5 | 6 | 46.0 | 46.8 | 36.5 | 483 | 9 | 65 | 15.4 | 22.1 | +1 | 4 | 0 | 75 |
| NOTSOFAR MTG_32182 sc_panacast_0 | EN | 6 | 34.9 | 35.1 | 30.9 | 384 | 10 | 32 | 6.9 | 15.9 | +0 | 8 | 0 | 73 |
| NOTSOFAR MTG_32182 sc_panacast_0 | 3.5 | 6 | 37.6 | 37.8 | 33.8 | 422 | 9 | 35 | 8.0 | 15.9 | +0 | 6 | 0 | 74 |
| NOTSOFAR MTG_32183 sc_plaza_0 | EN | 6 | 43.9 | 45.1 | 41.7 | 598 | 4 | 37 | 5.8 | 29.5 | +0 | 15 | 3 | 76 |
| NOTSOFAR MTG_32183 sc_plaza_0 | 3.5 | 6 | 42.0 | 42.3 | 39.7 | 550 | 5 | 54 | 6.3 | 23.6 | +0 | 14 | 0 | 76 |
| NOTSOFAR MTG_32185 sc_panacast_0 | EN | 6 | 45.2 | 50.7 | 23.9 | 284 | 2 | 25 | 20.6 | 31.5 | -1 | 4 | 0 | 75 |
| NOTSOFAR MTG_32185 sc_panacast_0 | 3.5 | 6 | 45.1 | 50.3 | 23.6 | 274 | 2 | 31 | 20.0 | 29.1 | -1 | 2 | 0 | 75 |
| NOTSOFAR MTG_32190 sc_rockfall_0 | EN | 6 | 35.8 | 36.3 | 32.1 | 405 | 12 | 40 | 6.1 | 20.3 | +0 | 4 | 1 | 75 |
| NOTSOFAR MTG_32190 sc_rockfall_0 | 3.5 | 6 | 32.2 | 33.2 | 28.8 | 353 | 9 | 48 | 7.2 | 13.8 | +0 | 0 | 0 | 72 |
| NOTSOFAR MTG_32191 sc_panacast_0 | EN | 6 | 33.6 | 34.9 | 27.7 | 340 | 10 | 38 | 8.4 | 19.2 | +0 | 9 | 0 | 77 |
| NOTSOFAR MTG_32191 sc_panacast_0 | 3.5 | 6 | 36.9 | 37.9 | 27.8 | 334 | 6 | 49 | 10.8 | 19.3 | +0 | 5 | 0 | 78 |
| NOTSOFAR MTG_32252 sc_studio_0 | EN | 6 | 33.1 | 33.6 | 30.9 | 407 | 14 | 38 | 5.3 | 24.9 | +0 | 7 | 0 | 75 |
| NOTSOFAR MTG_32252 sc_studio_0 | 3.5 | 6 | 35.5 | 35.8 | 33.0 | 426 | 15 | 50 | 5.8 | 24.0 | +0 | 5 | 0 | 75 |
| NOTSOFAR MTG_32260 sc_rockfall_0 | EN | 6 | 35.6 | 36.0 | 33.5 | 383 | 15 | 58 | 6.3 | 19.4 | +0 | 12 | 0 | 73 |
| NOTSOFAR MTG_32260 sc_rockfall_0 | 3.5 | 6 | 39.3 | 39.7 | 35.7 | 402 | 10 | 73 | 7.9 | 17.9 | +0 | 9 | 0 | 73 |
| NOTSOFAR MTG_32313 sc_rockfall_2 | EN | 6 | 13.0 | 13.1 | 11.6 | 114 | 12 | 19 | 1.8 | 6.9 | +0 | 2 | 0 | 73 |
| NOTSOFAR MTG_32313 sc_rockfall_2 | 3.5 | 6 | 14.0 | 14.3 | 12.5 | 128 | 10 | 19 | 2.0 | 5.7 | +0 | 3 | 0 | 72 |
| NOTSOFAR MTG_32321 sc_panacast_0 | EN | 6 | 24.0 | 24.4 | 22.1 | 244 | 12 | 38 | 3.9 | 15.2 | +0 | 4 | 0 | 76 |
| NOTSOFAR MTG_32321 sc_panacast_0 | 3.5 | 6 | 26.1 | 26.6 | 23.6 | 250 | 12 | 52 | 4.5 | 13.0 | +0 | 2 | 0 | 78 |
| NOTSOFAR MTG_32346 sc_plaza_0 | EN | 6 | 49.5 | 50.1 | 45.5 | 590 | 8 | 53 | 8.8 | 37.0 | +1 | 18 | 3 | 69 |
| NOTSOFAR MTG_32346 sc_plaza_0 | 3.5 | 6 | 52.0 | 52.8 | 47.1 | 579 | 11 | 84 | 12.9 | 35.8 | +1 | 16 | 1 | 72 |

### pub-phone: phone-quality calls, accented English (Earnings-22)

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Earnings-22 4481221 (India) | EN | 21 | 86.9 | 87.8 | 11.0 | 72 | 40 | 153 | 40.9 | 49.3 | -2 | 1 | 1 | 76 |
| Earnings-22 4481221 (India) | 3.5 | 21 | 86.8 | 87.4 | 11.7 | 91 | 33 | 157 | 40.8 | 47.9 | -2 | 1 | 0 | 80 |
| Earnings-22 4474955 (China) | EN | 15 | 15.4 | 15.9 | 11.8 | 86 | 23 | 119 | 2.1 | 11.8 | +0 | 2 | 0 | 80 |
| Earnings-22 4474955 (China) | 3.5 | 15 | 17.1 | 17.8 | 13.8 | 111 | 21 | 136 | 1.9 | 10.8 | +0 | 3 | 0 | 82 |
| Earnings-22 4420696 (South Korea) | EN | 59 | 120.1 | 133.6 | 6.8 | 212 | 56 | 241 | 66.4 | 66.8 | -8 | 1 | 0 | 77 |
| Earnings-22 4420696 (South Korea) | 3.5 | 59 | 120.1 | 132.6 | 7.9 | 282 | 37 | 270 | 66.1 | 66.1 | -8 | 4 | 3 | 83 |
| Earnings-22 4482383 (Australia) | EN | 58 | 27.3 | 28.7 | 9.6 | 354 | 140 | 483 | 9.8 | 19.3 | -2 | 4 | 3 | 81 |
| Earnings-22 4482383 (Australia) | 3.5 | 58 | 31.0 | 32.4 | 13.6 | 652 | 133 | 608 | 9.9 | 19.2 | -2 | 11 | 2 | 83 |
| Earnings-22 4449269 (Brazil) | EN | 44 | 111.9 | 117.9 | 13.6 | 260 | 103 | 406 | 56.6 | 60.8 | -8 | 4 | 1 | 77 |
| Earnings-22 4449269 (Brazil) | 3.5 | 44 | 108.9 | 115.3 | 17.1 | 454 | 82 | 435 | 55.6 | 59.5 | -8 | 13 | 1 | 82 |
| Earnings-22 4469528 (Argentina) | EN | 28 | 25.3 | 25.4 | 21.3 | 130 | 183 | 499 | 2.2 | 11.6 | +1 | 0 | 0 | 78 |
| Earnings-22 4469528 (Argentina) | 3.5 | 28 | 26.0 | 26.1 | 22.0 | 218 | 148 | 474 | 2.3 | 10.4 | +1 | 4 | 1 | 85 |
| Earnings-22 4471586 (Russia) | EN | 21 | 13.4 | 14.0 | 12.9 | 121 | 45 | 194 | 0.5 | 13.3 | +0 | 4 | 1 | 80 |
| Earnings-22 4471586 (Russia) | 3.5 | 21 | 15.8 | 16.3 | 15.5 | 194 | 37 | 201 | 0.3 | 14.5 | +0 | 5 | 1 | 81 |
| Earnings-22 4483623 (Canada) | EN | 28 | 41.6 | 42.8 | 7.8 | 125 | 72 | 132 | 17.8 | 30.7 | -1 | 2 | 0 | 75 |
| Earnings-22 4483623 (Canada) | 3.5 | 28 | 42.6 | 43.7 | 9.0 | 236 | 26 | 120 | 18.2 | 28.5 | -1 | 2 | 0 | 80 |

### pub-lang: other languages, close and distant (AliMeeting zh, SUMM-RE fr, Indic DiarBench hi)

Nemotron 3.5 only. AliMeeting rows count characters (cpWER, tcpWER and WER are character error rates there; del, ins and sub are characters).

| Session | Model | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| AliMeeting R8001_M8004 headset mix | 3.5 | 26 | 41.6 | 44.0 | 35.1 | 2648 | 91 | 887 | 14.3 | 27.3 | +0 | 6 | 0 | 87 |
| AliMeeting R8001_M8004 array ch0 | 3.5 | 26 | 56.1 | 58.2 | 51.8 | 4079 | 59 | 1221 | 14.0 | 36.0 | +0 | 51 | 17 | 86 |
| AliMeeting R8003_M8001 headset mix | 3.5 | 34 | 32.3 | 34.1 | 23.9 | 1397 | 128 | 843 | 11.9 | 18.1 | +0 | 3 | 1 | 89 |
| AliMeeting R8003_M8001 array ch0 | 3.5 | 34 | 35.5 | 37.0 | 31.4 | 1968 | 178 | 966 | 8.2 | 20.7 | +0 | 33 | 3 | 88 |
| AliMeeting R8007_M8010 headset mix | 3.5 | 31 | 62.4 | 65.4 | 51.1 | 5593 | 119 | 1537 | 31.4 | 50.7 | +0 | 10 | 0 | 89 |
| AliMeeting R8007_M8010 array ch0 | 3.5 | 31 | 64.3 | 66.8 | 56.8 | 6299 | 68 | 1698 | 27.5 | 51.2 | +0 | 36 | 3 | 87 |
| AliMeeting R8007_M8011 headset mix | 3.5 | 31 | 39.4 | 42.6 | 27.3 | 2227 | 109 | 677 | 16.8 | 24.3 | +0 | 6 | 0 | 87 |
| AliMeeting R8007_M8011 array ch0 | 3.5 | 31 | 42.8 | 45.5 | 33.4 | 2790 | 78 | 830 | 15.2 | 25.5 | +0 | 22 | 2 | 88 |
| AliMeeting R8008_M8013 headset mix | 3.5 | 37 | 33.8 | 36.1 | 20.3 | 1182 | 69 | 509 | 14.5 | 22.8 | +0 | 21 | 3 | 87 |
| AliMeeting R8008_M8013 array ch0 | 3.5 | 37 | 47.1 | 48.2 | 41.8 | 2305 | 65 | 1262 | 9.9 | 28.1 | +0 | 87 | 23 | 87 |
| AliMeeting R8009_M8018 headset mix | 3.5 | 28 | 24.0 | 25.0 | 15.4 | 507 | 119 | 624 | 7.1 | 12.1 | +0 | 3 | 0 | 87 |
| AliMeeting R8009_M8018 array ch0 | 3.5 | 28 | 25.1 | 25.8 | 17.4 | 531 | 127 | 754 | 7.2 | 12.3 | +0 | 1 | 0 | 89 |
| AliMeeting R8009_M8019 headset mix | 3.5 | 33 | 25.5 | 25.9 | 20.5 | 898 | 117 | 692 | 6.1 | 12.9 | +0 | 2 | 0 | 86 |
| AliMeeting R8009_M8019 array ch0 | 3.5 | 33 | 29.8 | 30.4 | 26.0 | 1238 | 80 | 841 | 5.4 | 14.6 | +0 | 11 | 0 | 87 |
| AliMeeting R8009_M8020 headset mix | 3.5 | 32 | 14.2 | 14.7 | 11.2 | 432 | 88 | 420 | 3.1 | 6.8 | +0 | 0 | 0 | 88 |
| AliMeeting R8009_M8020 array ch0 | 3.5 | 32 | 16.9 | 17.5 | 13.9 | 586 | 63 | 513 | 3.2 | 7.6 | +0 | 5 | 0 | 88 |
| Indic DiarBench hindi_054 (far field) | 3.5 | 15 | 43.2 | 43.6 | 42.0 | 1432 | 30 | 443 | 8.8 | 29.6 | +0 | 11 | 0 | 82 |
| Indic DiarBench hindi_055 (far field) | 3.5 | 15 | 41.5 | 43.0 | 38.8 | 1006 | 72 | 585 | 12.8 | 21.9 | +0 | 6 | 0 | 80 |
| Indic DiarBench hindi_056 (far field) | 3.5 | 15 | 30.3 | 30.6 | 29.7 | 774 | 39 | 456 | 7.3 | 17.2 | +0 | 1 | 0 | 80 |
| Indic DiarBench hindi_057 (far field) | 3.5 | 15 | 18.9 | 19.0 | 18.6 | 349 | 24 | 306 | 1.4 | 5.1 | +0 | 1 | 0 | 81 |
| Indic DiarBench hindi_058 (far field) | 3.5 | 10 | 45.7 | 46.3 | 45.0 | 947 | 27 | 332 | 10.6 | 33.5 | +0 | 13 | 0 | 76 |
| Indic DiarBench hindi_059 (far field) | 3.5 | 10 | 54.0 | 54.3 | 50.1 | 1161 | 13 | 436 | 14.8 | 35.2 | +0 | 13 | 2 | 74 |
| Indic DiarBench hindi_060 (far field) | 3.5 | 10 | 27.4 | 27.7 | 26.4 | 303 | 28 | 339 | 2.7 | 7.0 | +0 | 1 | 0 | 78 |
| Indic DiarBench hindi_061 (far field) | 3.5 | 10 | 17.8 | 17.8 | 17.6 | 137 | 19 | 238 | 1.0 | 3.2 | +0 | 3 | 0 | 80 |
| Indic DiarBench hindi_062 (far field) | 3.5 | 5 | 40.0 | 40.3 | 38.9 | 340 | 33 | 232 | 7.7 | 16.9 | +0 | 0 | 0 | 71 |
| Indic DiarBench hindi_063 (far field) | 3.5 | 5 | 26.9 | 27.2 | 26.7 | 141 | 12 | 146 | 4.3 | 10.3 | +0 | 1 | 0 | 73 |
| Indic DiarBench hindi_075 (near field) | 3.5 | 15 | 37.4 | 37.7 | 36.1 | 723 | 10 | 256 | 5.5 | 21.2 | +0 | 27 | 4 | 78 |
| Indic DiarBench hindi_076 (near field) | 3.5 | 15 | 34.5 | 35.2 | 30.8 | 570 | 22 | 333 | 7.5 | 18.9 | +0 | 15 | 1 | 75 |
| Indic DiarBench hindi_077 (near field) | 3.5 | 15 | 20.6 | 20.7 | 19.8 | 267 | 26 | 287 | 1.5 | 6.1 | +0 | 5 | 0 | 79 |
| Indic DiarBench hindi_078 (near field) | 3.5 | 15 | 37.9 | 41.9 | 27.3 | 507 | 14 | 251 | 15.6 | 22.7 | +0 | 2 | 0 | 84 |
| Indic DiarBench hindi_079 (near field) | 3.5 | 10 | 50.9 | 51.2 | 49.7 | 371 | 10 | 434 | 6.5 | 15.8 | +0 | 8 | 0 | 79 |
| Indic DiarBench hindi_080 (near field) | 3.5 | 10 | 18.0 | 18.1 | 17.6 | 168 | 15 | 188 | 1.7 | 6.5 | +0 | 2 | 0 | 76 |
| Indic DiarBench hindi_081 (near field) | 3.5 | 10 | 44.7 | 45.5 | 43.8 | 501 | 21 | 388 | 6.5 | 22.9 | +0 | 10 | 0 | 78 |
| Indic DiarBench hindi_082 (near field) | 3.5 | 10 | 16.9 | 17.0 | 17.5 | 186 | 20 | 178 | 1.5 | 5.8 | +0 | 1 | 0 | 78 |
| Indic DiarBench hindi_083 (near field) | 3.5 | 5 | 14.2 | 14.2 | 13.8 | 54 | 2 | 82 | 0.6 | 4.7 | +0 | 0 | 0 | 73 |
| Indic DiarBench hindi_084 (near field) | 3.5 | 5 | 46.6 | 46.7 | 46.8 | 118 | 17 | 248 | 3.1 | 9.7 | +0 | 0 | 0 | 75 |
| SUMM-RE 018a_EARZ (Zoom) | 3.5 | 22 | 31.2 | 31.6 | 30.0 | 1071 | 125 | 340 | 4.1 | 13.0 | +0 | 30 | 5 | 79 |
| SUMM-RE 020b_EBDZ (Zoom) | 3.5 | 18 | 27.1 | 27.7 | 26.2 | 574 | 80 | 308 | 2.1 | 12.6 | +0 | 26 | 6 | 79 |
| SUMM-RE 027a_EBRH (studio) | 3.5 | 22 | 25.2 | 25.7 | 23.2 | 709 | 106 | 214 | 3.8 | 16.7 | +0 | 22 | 2 | – |
| SUMM-RE 032b_EADH (studio) | 3.5 | 20 | 54.2 | 54.7 | 50.3 | 2940 | 41 | 347 | 12.1 | 42.5 | +0 | 69 | 16 | 78 |
| SUMM-RE 033a_EBRH (studio) | 3.5 | 21 | 40.9 | 41.4 | 37.8 | 1485 | 99 | 420 | 7.7 | 18.8 | +0 | 22 | 3 | 78 |
| SUMM-RE 033c_EBPH (studio) | 3.5 | 21 | 46.0 | 46.5 | 42.6 | 1780 | 85 | 481 | 10.0 | 26.5 | +1 | 43 | 2 | 77 |
| SUMM-RE 034a_EBRH (studio) | 3.5 | 21 | 26.9 | 27.0 | 24.6 | 738 | 67 | 206 | 4.1 | 14.5 | +1 | 26 | 2 | 77 |
| SUMM-RE 035b_EADH (studio) | 3.5 | 20 | 28.9 | 29.7 | 26.5 | 828 | 67 | 281 | 5.3 | 14.5 | +0 | 17 | 3 | 78 |
| SUMM-RE 036c_EAPH (studio) | 3.5 | 27 | 38.4 | 39.0 | 36.0 | 1390 | 103 | 441 | 6.2 | 24.9 | +0 | 55 | 5 | 77 |
| SUMM-RE 069c_EEPL (lab) | 3.5 | 21 | 24.6 | 25.0 | 23.0 | 659 | 49 | 233 | 3.4 | 16.6 | +0 | 21 | 5 | 79 |

### Pooled

Sums over the sessions of each row (see Metrics). EN covers the English sessions only, so its totals over everything it ran are the English groups row.

| Set | Model | Sessions | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | DER c0.25 | Sum of abs count err | Gaps ≥ 2 s | Gaps ≥ 5 s | × real time |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| AMI EN2002 Mix-Headset (pub-meeting) | EN | 4 | 152 | 31.7 | 32.1 | 28.9 | 8010 | 239 | 1383 | 5.5 | 34.5 | 1 | 182 | 43 | – |
| AMI EN2002 Mix-Headset (pub-meeting) | 3.5 | 4 | 152 | 35.6 | 36.1 | 33.0 | 9174 | 207 | 1626 | 5.8 | 36.9 | 1 | 243 | 53 | – |
| ICSI (pub-meeting) | EN | 4 | 221 | 28.3 | 30.5 | 27.5 | 9027 | 356 | 1533 | 4.2 | 33.7 | 3 | 223 | 111 | 77 (2 of 4) |
| ICSI (pub-meeting) | 3.5 | 4 | 221 | 29.3 | 31.6 | 28.6 | 9332 | 250 | 1746 | 4.4 | 32.5 | 3 | 247 | 82 | 77 (3 of 4) |
| **pub-meeting** | EN | 8 | 373 | 29.9 | 31.2 | 28.2 | 17037 | 595 | 2916 | 4.8 | 34.1 | 4 | 405 | 154 | 77 (2 of 8) |
| **pub-meeting** | 3.5 | 8 | 373 | 32.2 | 33.7 | 30.6 | 18506 | 457 | 3372 | 5.0 | 34.9 | 4 | 490 | 135 | 77 (3 of 8) |
| AMI EN2002 Array1-01 (pub-far) | EN | 4 | 152 | 38.6 | 39.2 | 35.5 | 9805 | 194 | 1826 | 6.8 | 38.3 | 1 | 287 | 81 | 78 (2 of 4) |
| AMI EN2002 Array1-01 (pub-far) | 3.5 | 4 | 152 | 42.6 | 43.3 | 39.6 | 10832 | 168 | 2209 | 7.5 | 40.2 | 1 | 333 | 88 | 78 |
| NOTSOFAR-1 (pub-far) | EN | 32 | 208 | 45.4 | 46.7 | 39.6 | 18220 | 328 | 1624 | 10.3 | 27.2 | 7 | 480 | 74 | 74 (28 of 32) |
| NOTSOFAR-1 (pub-far) | 3.5 | 32 | 208 | 45.5 | 46.7 | 39.5 | 17571 | 297 | 2251 | 11.0 | 23.3 | 7 | 341 | 28 | 75 (31 of 32) |
| **pub-far** | EN | 36 | 360 | 42.7 | 43.7 | 38.0 | 28025 | 522 | 3450 | 8.8 | 33.5 | 8 | 767 | 155 | 75 (30 of 36) |
| **pub-far** | 3.5 | 36 | 360 | 44.4 | 45.3 | 39.6 | 28403 | 465 | 4460 | 9.6 | 32.9 | 8 | 674 | 116 | 76 (35 of 36) |
| **pub-phone** (Earnings-22) | EN | 8 | 274 | 61.2 | 65.3 | 11.0 | 1360 | 662 | 2227 | 28.7 | 38.0 | 22 | 18 | 6 | 78 |
| **pub-phone** (Earnings-22) | 3.5 | 8 | 274 | 62.2 | 66.1 | 13.4 | 2238 | 517 | 2401 | 28.6 | 37.2 | 22 | 43 | 8 | 82 |
| **English groups** (meeting, far, phone) | EN | 52 | 1006 | 41.6 | 43.3 | 29.0 | 46422 | 1779 | 8593 | 12.3 | 34.7 | 34 | 1190 | 315 | 77 (40 of 52) |
| **English groups** (meeting, far, phone) | 3.5 | 52 | 1006 | 43.3 | 45.1 | 31.1 | 49147 | 1439 | 10233 | 12.6 | 34.6 | 34 | 1207 | 259 | 78 (46 of 52) |
| AliMeeting headset mix (zh, per character) | 3.5 | 8 | 252 | 36.6 | 38.6 | 27.7 | 14884 | 840 | 6189 | 13.4 | 22.6 | 0 | 51 | 4 | 88 |
| AliMeeting array ch0 (zh, per character) | 3.5 | 8 | 252 | 42.0 | 43.6 | 36.2 | 19796 | 718 | 8085 | 11.4 | 25.3 | 0 | 246 | 48 | 87 |
| AliMeeting, both channels (zh, per character) | 3.5 | 16 | 504 | 39.3 | 41.1 | 32.0 | 34680 | 1558 | 14274 | 12.5 | 23.9 | 0 | 297 | 52 | 87 |
| SUMM-RE Zoom (fr) | 3.5 | 2 | 40 | 29.5 | 30.0 | 28.4 | 1645 | 205 | 648 | 3.3 | 12.9 | 0 | 56 | 11 | 79 |
| SUMM-RE studio and lab (fr) | 3.5 | 8 | 173 | 37.3 | 37.8 | 34.5 | 10529 | 617 | 2623 | 6.6 | 21.9 | 2 | 275 | 38 | 78 (7 of 8) |
| SUMM-RE (fr) | 3.5 | 10 | 213 | 35.9 | 36.4 | 33.4 | 12174 | 822 | 3271 | 6.0 | 20.2 | 2 | 331 | 49 | 78 (9 of 10) |
| Indic DiarBench near field (hi) | 3.5 | 10 | 110 | 31.7 | 32.5 | 29.4 | 3465 | 157 | 2645 | 5.5 | 14.4 | 0 | 70 | 5 | 78 |
| Indic DiarBench far field (hi) | 3.5 | 10 | 110 | 35.6 | 36.1 | 34.3 | 6590 | 297 | 3513 | 7.3 | 18.7 | 0 | 50 | 2 | 78 |
| Indic DiarBench (hi) | 3.5 | 20 | 221 | 34.0 | 34.6 | 32.3 | 10055 | 454 | 6158 | 6.5 | 16.8 | 0 | 120 | 7 | 78 |
| **pub-lang** (units mixed: zh characters, fr and hi words) | 3.5 | 46 | 938 | 37.6 | 38.9 | 32.3 | 56909 | 2834 | 23703 | 10.1 | 21.3 | 2 | 748 | 108 | 83 (45 of 46) |
| **all** (units mixed for 3.5) | 3.5 | 98 | 1944 | 40.1 | 41.6 | 31.8 | 106056 | 4273 | 33936 | 11.1 | 27.5 | 36 | 1955 | 367 | 81 (91 of 98) |

## What the numbers say

Measured unless marked as inference. One configuration per model, so these are reference points for later changes, not comparisons between settings.

- **Deletions dominate.** On the English groups, deletions are 82 % of the WER errors with EN (46,422 of 56,794) and 81 % with 3.5 (49,147 of 60,819). On the other languages they are 68 % (56,909 of 83,446). AMI ES2002a in the Zoom baseline shows the same pattern (339 of 433 with EN). Missed words are the main loss on every kind of audio here.
- **Open meetings are much harder than the gate set's AMI meeting.** AMI EN2002 (natural discussion, 24 to 30 % overlap) through the same kind of mix channel scores cpWER 31.7 with EN and 35.6 with 3.5. AMI ES2002a (scenario meeting) scores 18.0 and 20.7. ICSI scores 28.3 and 29.3.
- **EN is ahead of 3.5 on English, but leaves more long gaps.** Pooled over the 52 English sessions: cpWER 41.6 against 43.3, WER 29.0 against 31.1. The lead is largest on AMI (3.9 cpWER points on the mix, 4.0 on the array) and on Earnings-22 (WER 11.0 against 13.4); on NOTSOFAR the two are level (45.4 and 45.5). EN has 315 gaps of 5 s or more against 259 for 3.5. Most of them are on NOTSOFAR (74 against 28) and ICSI (111 against 82, 44 against 15 on Bmr019). So EN misses fewer words overall but loses them in longer runs there.
- **Distance on the same words.** AMI Array1-01 against Mix-Headset (same meetings, same reference): cpWER rises 6.9 points with EN (31.7 to 38.6) and 7.0 with 3.5 (35.6 to 42.6). Deletions rise by 1,795 and 1,658, and gaps of 2 s or more go from 182 to 287 and from 243 to 333. AliMeeting array channel 0 against the headset mix: cpWER (per character) 36.6 to 42.0, CER 27.7 to 36.2, deletions 14,884 to 19,796, gaps 51 to 246. Indic DiarBench far field against near field (31.7 against 35.6) and SUMM-RE in-room against Zoom (37.3 against 29.5) point the same way, but those are different recordings.
- **Phone calls: the words are right, the speakers are not.** Earnings-22 has the lowest WER of any group (11.0 with EN, 13.4 with 3.5) and the highest cpWER (61.2 and 62.2): WDER 28.7, count error summed over 8 calls 22. Three calls are fine (China, Russia and Argentina: WDER 0.3 to 2.3). The two calls with 12 and 13 reference speakers (South Korea 4420696, Brazil 4449269) come out with 4 and 5 transcript speakers, cpWER above 100 % and WDER 56 to 66. On 4420696 (EN), two transcript speakers hold 7,247 of the 7,308 words, and each of the three main reference speakers is split between them (the largest 1,329 to 528 aligned words). India 4481221 (6 reference speakers, 4 found) has WDER 41. Inference: one phone channel per call and many short analyst turns defeat the diarizer. That is an attribution problem to study, not to tune on these calls.
- **Speaker counts** are exact on 35 of 52 sessions with EN and 79 of 98 with 3.5, and within one on 47 and 93. The five beyond one are the same for both models: four Earnings-22 calls (12 to 4, 13 to 5, 6 to 4, 10 to 8) and ICSI Bed016 (10 to 8; the diarizer has 8 speaker slots, research/20 section 2.2).
- **Other languages (3.5):** Mandarin cpWER per character 39.3 (headset mix 36.6, array 42.0), French 35.9 (Zoom 29.5, in-room 37.3), Hindi 34.0 (near field 31.7, far field 35.6). AliMeeting R8007_M8010 is the outlier (62.4 and 64.3, WDER 31 and 28). SUMM-RE studio meeting 032b is the worst French session (54.2, 2,940 deletions).
- **tcpWER** stays within 2.5 points of cpWER on every pooled row except pub-phone (4.1 with EN, 3.9 with 3.5), so the references line up with the audio. Single sessions differ more: ICSI Bed004 by 5.5 and 5.7, NOTSOFAR MTG_32049 by 11.3 and 11.0 (68.8 against 80.1 with EN).
- **Speed** matches the Zoom recordings: 77 × real time pooled with EN and 81 with 3.5 on the timed runs, as on the owner's Zoom recordings (76 and 82). Peak VRAM delta of the transcribe step 1,277 to 2,243 MiB, median 1,373 (150 runs).

## Harness changes and problems found

Committed with this report (`tools/eval`):

- `metrics.py`: the speaker-agnostic WER with its deletions, insertions and substitutions (from the same alignment as WDER), and the gap count of research/19 section 0, are now in every metrics.json (`wer`, `w_del`, `w_ins`, `w_sub`, `gaps`, `gaps_5s`, `gap_ref_words`, `longest_gap_s`). Research/19 computed them with a scratch script. tcpWER error counts and DER's scored speech (`tcp_errors`, `tcp_length`, `der_c025_total_s`) are kept for pooling. `test_metrics.py` has a case for them.
- `metrics.py`: **meeteval 0.4.3 refuses its ORC-based measures when one side has more than 10 streams.** DI-cpWER puts the reference speakers on that side, so scoring failed outright on Earnings-22 4420696 (12 speakers) and 4449269 (13). DI-cpWER and ORC-WER are now NaN for such sessions and everything else is scored; cpWER and tcpWER allow 20 speakers.
- `run.py --no-diar-pass`: leaves out the standalone diarize pass that `--rttm` adds after the transcript is built. That pass never feeds the transcript, so the transcript, its tags and the transcribe-step time stay the same, and the GPU time per session nearly halves (research/19). Research/19 did this with a scratch copy of run.py. run.json records `diar_pass`.
- `levers.py baseline`: the per-group and pooled tables of this report from cached runs, with `--clean-timing` for the speed column and `--pool NAME=REGEX` for extra pooled rows.

Seen while running, not changed (they are outside tools/eval or belong to the coordinator):

- **run.py waits for an idle GPU after the queue has granted the lock.** `gpu.wait_idle()` waits up to 60 minutes while the caller holds gpu.lock and quiet.lock. From about 06:30 to 07:30 IST the owner's llama-server held about 8 GB of GPU memory. One of these runs then sat on both locks for 18 minutes before it was stopped. The next holder, another agent's run.py, held them from 06:51 until the memory was freed at about 07:30. The batch script used here now checks GPU memory before it queues and gives the locks back when the GPU is not idle. A shared fix would be to check before taking the lock.
- **`gpu-run.sh` and `gpu-run2.sh` do not stop on SIGTERM while waiting.** The `INT TERM` trap runs `cleanup` but does not exit, so the wait loop recreates its ticket and keeps its place. Only `kill -9` ends it, which leaves the ticket file until it goes stale (120 s). `trap 'cleanup; exit 143' INT TERM` would fix it.
- A python.exe CUDA process ran at up to 73 % SM around 04:20 IST while this agent held gpu.lock (EN, ICSI Bed016, which is therefore untimed).

## Reproducing

From `tools/eval` with the README setup, the seven sets fetched (research/20 section 7), a diarist-cli built from the beta.4 tree, and copies of the app's engine and models:

```sh
cargo build --release --locked -p diarist-cli && cp target/release/diarist-cli.exe tools/eval/.bin/   # from the repo root
export DIARIST_EVAL_ENGINE_DIR=<copy of the app's engine>/nemo-speech-0.2.0-windows-x86_64-cuda
export DIARIST_EVAL_MODELS_DIR=<models folder with the three manifest GGUFs>
export DIARIST_EVAL_CLI=<repo>/tools/eval/.bin/diarist-cli.exe
# transcribe, one session per process (each call wrapped in the agents' gpu-run2.sh, tier 3, with quiet.lock
# and nvidia-smi pmon -s u -d 1 around it; English sessions with both configs, pub-lang with shipped_n35 only)
uv run python -I run.py --config shipped_en  --sessions-file sessions.public.toml --sessions <id> --transcribe-only --no-diar-pass
uv run python -I run.py --config shipped_n35 --sessions-file sessions.public.toml --sessions <id> --transcribe-only --no-diar-pass
# score (CPU only)
uv run python -I run.py --config shipped_en  --sessions-file sessions.public.toml --groups pub-meeting,pub-far,pub-phone --metrics-only --no-report
uv run python -I run.py --config shipped_n35 --sessions-file sessions.public.toml --metrics-only --no-report
# tables (clean_timing.json: {"<config>/<session>": true|false} from the pmon logs and the quiet.lock rule above)
uv run python -I levers.py --sessions-file sessions.public.toml baseline --configs shipped_en,shipped_n35 \
  --clean-timing clean_timing.json \
  --pool "English groups=^(AMI|ICSI|NOTSOFAR|Earnings)" \
  --pool "AliMeeting headset mix=AliMeeting .* headset mix" --pool "AliMeeting array ch0=AliMeeting .* array ch0" \
  --pool "SUMM-RE Zoom=SUMM-RE .*\(Zoom\)" --pool "SUMM-RE in-room (studio, lab)=SUMM-RE .*\((studio|lab)\)" \
  --pool "Indic DiarBench near field=near field" --pool "Indic DiarBench far field=far field"
```

The two whole-set commands of research/20 section 7 (`run.py --config shipped_n35 --sessions-file sessions.public.toml` and the same with `shipped_en`) should give the same transcripts (the runtime is deterministic, research/19 section 2; not re-checked here). They run every session in one process and add the standalone diarize pass unless `--no-diar-pass` is given. The per-session loop, the timing check and the pmon logs are scratch scripts of this session, not part of the repository.
