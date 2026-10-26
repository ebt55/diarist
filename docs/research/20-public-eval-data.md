# Public evaluation data: what fills the gaps in our eval set (26 Oct 2026)

Research and tooling. Opus 5.5 research agent, worktree `claude/eval-public-data`. The owner's eval data (docs/05) is about 28 hours of their own Zoom recordings plus one public AMI meeting. It is almost all English Zoom talks with Q&A, scored against Zoom's own machine transcripts, and only AMI ES2002a has a human word reference. The owner has no other annotated data and asked for public sets. This document surveys the candidates, picks slices, and records how they were built into `tools/eval`. Every licence statement is quoted from its source, with the date it was read (26 Oct 2026 unless stated). Sources are numbered [S1] to [S27] at the end. **Inference** marks our reading, not something a source says. **Measured** marks something checked on the development PC.

**Summary.**

- **Seven sets chosen: 98 sessions, 32.4 hours of audio (25.7 hours of distinct speech, since AMI and AliMeeting are each heard through two microphones), in 4 groups** (section 3). AMI EN2002a-d in two channels and ICSI (open discussions, 3 to 10 speakers, overlap); NOTSOFAR-1 single-channel devices and AMI's table microphone (distant audio); Earnings-22 (accented phone calls with names and numbers); AliMeeting (Mandarin, close and distant), SUMM-RE (French) and Indic DiarBench (Hindi). All references are human-made: five sets have word times (manual or forced alignment) and two have turn times; four carry the transcript's punctuation and casing for the punctuation work, and two more its punctuation (Mandarin and Hindi have no case).
- **All are CC BY 4.0 or CC BY-SA 4.0, with no account, no registration and no click-through.** One open point: Earnings-22's licence file names only its transcripts and text files, not the audio Rev ships in the same repository (section 2.3).
- **Owner's call** (licence, registration or gate; not fetched): MLC-SLM (11 conversational languages, Data Use Agreement), VoxConverse and MSDWild (research-only, third-party video, diarization only), MagicData-RAMC (non-commercial), Mixer 6 and MMCSG (registration), Basis Conversations 1500 (gated), Santa Barbara Corpus (no derivatives). Section 4.
- **Fit the rules but not taken now**: CHiME-6 (since moved to OpenSLR under CC BY-SA 4.0, but 11 GB for its two dev sessions), DiPCo (one 13.4 GB archive), AISHELL-4, Earnings-21, EdAcc, Primock57. Section 4 says why and when to revisit.
- **Disk**: 10.3 GB downloaded, 3.6 GB kept after `--prune` (**measured**). **Smoke runs**: every set's references parse, line up with the audio and score on a 3-minute clip through the installed runtime (section 6).

## 1. The gaps

| Gap | What our set has | What a public set must add |
|---|---|---|
| 1. Open discussions, many people, overlap | AMI ES2002a (4 speakers, 21 minutes); the Zoom sessions are talks with Q&A, overlap serialised by Zoom | Natural meetings of 4 to 10 people with real overlap and a word reference that keeps it |
| 2. Poor or distant audio | Zoom audio (each person on their own microphone); one in-room session whose reference is coarse | Room microphones at a distance, and phone-bandwidth calls |
| 3. Non-English conversation | FLEURS slices, which are read sentences, WER only | Conversational multi-speaker speech in languages of `manifests/languages.json` |
| 4. Human word references, punctuated | AMI ES2002a only; Zoom references are machine output | Human transcripts with word times, ideally with their own punctuation and casing (docs/research/18) |

The languages the multilingual model is offered for (`manifests/languages.json` [S27]): ready tier es, it, pt, hi, ko, en, de, fr, ru, tr, vi, nl, ja, ar, uk; broad tier pl, nb, fi, zh-CN, cs, bg, sk, sv, hr, ro, et, da, hu; adaptation tier el, lt, lv, mt, sl, he, th, nn. Mandarin (`zh-CN`) is in the broad tier.

## 2. Survey

### 2.1 Table

"Words" means a word-level human reference with times; "turns" means speaker turns with text and times; "PnC" means the reference keeps punctuation and casing. Sizes are of the slice we would take unless stated.

| Set | Content | Hours | Speakers | Overlap | Audio | Reference | Licence | Access | Slice download | Formats | Decision |
|---|---|---|---|---|---|---|---|---|---|---|---|
| AMI Meeting Corpus [S1-S3] | Design-team scenario and natural meetings, Edinburgh, Idiap, TNO | about 100 | 3 to 5 | 24 to 30 % of speech in EN2002 (measured from the RTTMs) | Headsets, lapels, two table arrays, Mix-Headset | Words, PnC, speakers | CC BY 4.0 | Direct HTTPS | 0.61 GB (EN2002a-d, two channels) | WAV 16 kHz, NXT XML, RTTM | **Chosen** |
| ICSI Meeting Corpus [S4, S5] | Weekly research-group meetings, Berkeley 2000-2002 | about 70 (75 meetings) | 3 to 10 | 5 to 42 % by segments (measured) | Headset mix (one WAV per meeting) or individual headsets; the table microphones are not on the mirror | Words (forced-aligned, about 90 % timed), PnC, speakers | CC BY 4.0 | Direct HTTPS (a form writes a wget script) | 0.47 GB (4 meetings) | WAV, NXT XML | **Chosen** |
| NOTSOFAR-1 [S6, S7] | Real office meetings in 30 rooms, CHiME-8 task 2 | 237 meetings of about 6 min; eval-small 80 meetings, 8.2 h (measured) | 3 to 8 | high (hashtags #DebateOverlaps, #ShortTurnsOverlap) | Commercial meeting devices: single-channel and 7-channel, plus close-talk | Words (aligned on close-talk), PnC text, speakers | CC BY 4.0 | Hugging Face, not gated | 0.41 GB (32 meetings, one device each) | WAV 16 kHz, JSON | **Chosen** |
| DiPCo [S8] | Dinner parties of Amazon volunteers in a home | 10 sessions of 15 to 45 min | 4 | high | 5 far-field 7-mic devices, close-talk | Human transcripts | CDLA-Permissive-1.0 | Zenodo, no account | 13.4 GB (single archive, no smaller unit) | WAV, JSON | Fits the rules; not taken (section 4) |
| CHiME-6 [S9] | Dinner parties (CHiME-5, re-synchronised) | dev 2 sessions, eval 2 sessions (train 16) | 4 | high | Kinect arrays, binaural close-talk | Utterance transcripts, speakers | CC BY-SA 4.0 (openslr.org/150) | OpenSLR, no registration | 11 GB (dev archive) | WAV, JSON | Fits the rules; not taken (section 4) |
| CHiME-8 DASR, MMCSG [S10, S11] | DASR adds Mixer 6 to CHiME-6, DiPCo, NOTSOFAR-1; MMCSG is two-person talks on smart glasses | MMCSG 26 h | 2 (MMCSG) | | | | Mixer 6 via LDC request after challenge registration; MMCSG "a registration is needed to obtain the data" | Registration | | | Owner's call |
| Earnings-21 [S12] | 44 US earnings calls, 2020 | 39 | up to 14 | serialised | Webcast/phone audio, MP3 | Tokens with speaker, PnC, entity tags, no times; RTTMs | Transcripts CC BY-SA 4.0 | GitHub | | MP3, NLP, RTTM | Fits; not taken (no word times) |
| Earnings-22 [S13] | 125 earnings calls of companies in 27 countries | 119 | 2 to 33 | serialised | Webcast/phone audio, MP3, 11 to 48 kHz | Tokens with speaker, PnC, entity tags; force-aligned times | Transcripts CC BY-SA 4.0; audio unstated (2.3) | GitHub, Git LFS | 66 MB (8 calls) | MP3, NLP | **Chosen** |
| AliMeeting [S14] | Real Mandarin meetings, M2MeT challenge | 118.75; Eval 4 (8 meetings) | 2 to 4 | high (M2MeT) | 8-channel table array and a headset per participant | Turns per speaker (TextGrid), punctuated text | CC BY-SA 4.0 | Alibaba OSS, no account | 3.67 GB (Eval archive, "3.42G") | WAV, TextGrid | **Chosen** |
| AISHELL-4 [S15] | Mandarin conference-room meetings | 120 (211 meetings) | 4 to 8 | "speech overlap, quick speaker turn" | 8-channel circular array only | Turns, speaker activity | CC BY-SA 4.0 | OpenSLR | 5.2 GB (test archive) | WAV, TextGrid | Fits; not taken (AliMeeting first) |
| SUMM-RE [S19, S20] | French meeting-style conversations on set topics, studio, lab or Zoom | about 95; ASRU subset 10 meetings, 3.6 h (measured) | 3 or 4 | 4 to 28 % in the subset (measured) | One microphone per participant | Words (manually corrected alignment), lower case, no punctuation | CC BY-SA 4.0 | Hugging Face, not gated | 4.4 GB (ASRU subset, 39 tracks) | WAV, CSV, TextGrid | **Chosen** |
| Indic DiarBench [S21] | Natural multi-speaker conversation in 22 Indian languages, code-mixed | 108; Hindi 10.7 | 2 and more | 16.6 % average for Hindi (card) | Near field (virtual meetings), far field, YouTube | Turns, human-corrected | CC BY 4.0 | Hugging Face, not gated | 0.63 GB of the 1.22 GB Hindi parquet (range reads) | Parquet (WAV bytes) | **Chosen** (Hindi, near and far field) |
| MLC-SLM [S22] | Two-person conversations in 11 languages (2025), 14 in 2026 | about 1,600 (2025) | 2 | natural | Phone and device recordings | Turns, speakers | References CC BY-SA 4.0; audio under a Data Use Agreement | Registration | | | Owner's call |
| VoxConverse [S16] | Political debates and news from YouTube | over 50 | up to about 20 | yes | Broadcast audio | Speaker turns only (RTTM) | "available to download for research purposes" under CC BY 4.0; "The copyright remains with the original owners of the video" | Direct zip | | WAV, RTTM | Owner's call |
| MSDWild [S17] | Daily conversation videos, many languages | not stated | 2 to many | yes | Video soundtracks | Speaker turns only (RTTM) | "ONLY for research purposes", licensing agreement PDF | Google Drive | 7.56 GB (all audio) | WAV, RTTM | Owner's call |
| MagicData-RAMC [S18] | Mandarin two-person conversations | 180 | 2 | natural | Phone and device | Turns | CC BY-NC-ND 4.0 | OpenSLR | 15 GB | WAV, TXT | Owner's call |
| Basis Conversations 1500 [S23] | Multi-party conversation in 22 languages | 1,500 | 2 to 4 | full duplex | | | "other" | Hugging Face, gated (manual) | | | Owner's call |
| EdAcc [S24] | Two-person English conversations over Zoom, many accents | 40 | 2 | some (<OVERLAP> tags) | Zoom | Utterances, upper case, no punctuation | CC BY-SA 4.0 | Hugging Face, not gated | | Parquet | Fits; not taken |
| Primock57 [S25] | Mock GP consultations, clinicians and actors | 57 consultations | 2 | little | One channel per speaker | Utterance transcripts | CC BY 4.0 | GitHub, Git LFS | | WAV, TextGrid | Fits; not taken |
| Santa Barbara Corpus [S26] | American English conversation and other genres | 60 recordings, about 249,000 words | 2 to 11 | yes | Field recordings | Intonation-unit transcripts | CC BY-ND 3.0 US | Direct (Box) | | WAV, TRN | Owner's call (no derivatives) |

### 2.2 Notes per chosen set

- **AMI** [S1-S3]. The Full-corpus-ASR test partition has 16 meetings; the four EN2002 meetings are the non-scenario ones (natural discussion, not role play), with 24 to 30 percent of the speech overlapped (measured from the pyannote only_words RTTMs; ES2004, IS1009 and TS3003 have 4 to 16 percent). The same reference serves two channels: Mix-Headset and Array1-01, one microphone of the table array, so the effect of distance is measured on the same words.
- **ICSI** [S4, S5]. Bed004, Bed016, Bmr019 and Bro018 were picked from the 75 meetings by participant count and overlap (measured from the NXT segments): 7, 10, 9 and 5 participants, 33, 23, 32 and 9 percent of speech overlapped, one meeting from each of the three main series (Bed, Bmr, Bro). Two of them exceed the Nemotron 3 diarizer's 8 speaker slots (docs/04 section 1, research/04). We believe they are among the six meetings of the ICSI summarisation test split, but did not find a source listing that split. The Edinburgh mirror offers only the headset mix and individual headsets, so ICSI fills gap 1, not gap 2. About 10 percent of the transcribed words have no forced-alignment time (many in the read-digits part at the end of each meeting); they are spread over their segment.
- **NOTSOFAR-1** [S6, S7]. The GitHub README tells users to make a Hugging Face token, but the API reports the dataset as not gated and every file resolves anonymously (**measured**). Eval-small has ground truth for 80 meetings; we take 32 (every meeting with 6 or 7 participants and every fourth of the others) with one single-channel device each, rotating through the five device kinds.
- **Earnings-22** [S13]. Chosen over Earnings-21 because it ships force-aligned references, so word-level attribution metrics work; it also has the accents the owner's meetings may share (one call each from India, China, South Korea, Australia, Brazil, Argentina, Russia, Canada). The forced alignment times anywhere from 3 to 99.8 percent of a call's tokens (measured over all 125); the slice uses calls at 96.9 percent or more and spreads the few untimed tokens between their timed neighbours.
- **AliMeeting** [S14]. Each Eval meeting is used twice, as the headsets' mix and as array channel 0, for the same reason as AMI's two channels.
- **SUMM-RE** [S19, S20]. The full corpus's dev and test splits are 102 GB of parquet; the ASRU subset on Hugging Face is the same kind of data as single files: 10 meetings, 39 per-speaker tracks, manually corrected word alignments. A meeting's tracks are summed into one mix.
- **Indic DiarBench** [S21]. Only Hindi is in our language list. The in-the-wild clips are cut from YouTube videos, so the slice leaves them out (**inference**: CC BY 4.0 from the publisher cannot clear third-party footage). Clips from one source recording are not independent (the card says so), so at most one clip per recording is taken: per condition, in sample-id order, clips of 5 to 30 minutes until 2 hours, which gives 10 near-field and 10 far-field clips of 5 to 15 minutes (the 30- and 60-minute recordings are a few seconds over the limit and the many short clips under it). The parquet stores the audio per row group, so only the footer, the small columns and the four row groups that hold these clips are read (0.63 of 1.22 GB), and each clip's audio is pinned by its own SHA-256.

### 2.3 Earnings-22 licence: the open point

The dataset README shows a "License: CC BY-SA 4.0" badge linking to LICENSE.md, and the repository README says the dataset "provides the full audios, transcripts, and accompanying metadata" [S13]. LICENSE.md itself reads: "The transcripts and associated text files that are used for alignment in this directory are licensed under a Creative Commons Attribution-ShareAlike 4.0 International license." It does not mention the audio, and the repository has no top-level licence (the GitHub API reports none). The calls are public webcasts whose rights sit with the companies (**inference**). We use the audio for internal evaluation only and never redistribute it. If the owner wants a strict reading, drop the set (`earnings22_fetch.py` is independent of the others) or keep only its transcripts.

## 3. The choice

| Set (fetch script) | Group | Sessions | Hours | Speakers | Fills gaps | Reference size | Reference | Download | Kept |
|---|---|---|---|---|---|---|---|---|---|
| AMI EN2002a-d, Mix-Headset (`ami_test_fetch.py`) | pub-meeting | 4 | 2.53 | 3 to 4 | 1, 4 | 31,852 words | manual words, PnC | 0.61 GB (both channels) | 0.58 GB (both) |
| AMI EN2002a-d, Array1-01 (same script) | pub-far | 4 | 2.53 | 3 to 4 | 2, 4 | the same words | the same | | |
| ICSI Bed004, Bed016, Bmr019, Bro018 (`icsi_fetch.py`) | pub-meeting | 4 | 3.68 | 5 to 10 | 1, 4 | 39,650 words (13 % interpolated) | forced-aligned words, PnC | 0.47 GB | 0.41 GB |
| NOTSOFAR-1 eval-small, 32 meetings (`notsofar_fetch.py`) | pub-far | 32 | 3.47 | 3 to 7 | 2, 1, 4 | 50,606 words | words aligned on close-talk, PnC | 0.41 GB | 0.40 GB |
| Earnings-22, 8 calls (`earnings22_fetch.py`) | pub-phone | 8 | 4.56 | 4 to 13 | 2, 4 | 38,774 words | forced-aligned words, PnC | 0.07 GB | 0.51 GB (WAV) |
| AliMeeting Eval, headset mix and array ch0 (`alimeeting_fetch.py`) | pub-lang | 16 | 8.40 (2 x 4.2) | 2 to 4 | 3, 2 | 78,982 characters, 6,457 turns | turns, punctuated | 3.67 GB | 0.93 GB |
| SUMM-RE ASRU subset (`summre_fetch.py`) | pub-lang | 10 | 3.55 | 3 to 4 | 3 | 48,332 words | manually corrected aligned words, no PnC | 4.44 GB | 0.41 GB |
| Indic DiarBench Hindi, near and far field (`indic_fetch.py`) | pub-lang | 20 | 3.68 | 2 to 4 | 3, 2 | 51,675 words, 2,934 turns | turns, punctuated | 0.63 GB (range reads) | 0.41 GB |
| **All** | | **98** | **32.41** | | | | | **10.3 GB** | **3.6 GB** |

By group: pub-meeting 8 sessions, 6.21 h; pub-far 36 sessions, 6.00 h; pub-phone 8 sessions, 4.56 h; pub-lang 46 sessions, 15.63 h. "Kept" is `.work/public/<set>/` after `--prune` (audio as 16 kHz WAV, references, lists), measured with `du`.

Why these slices:

- **Worth the GPU time.** Each set is 2.5 to 4.6 hours per channel, 31,000 to 52,000 reference words (79,000 characters for Mandarin). At 40,000 words and a 30 percent WER the binomial standard error is about 0.2 points, so a one-point change per set stands out (**inference**: errors cluster by speaker and passage, so the real noise is larger). A full run of all of it on one configuration is about 32 hours of audio, about 80 minutes of GPU at the shipped speed (section 7). Each group can be run alone (`--groups`).
- **Comparable pairs instead of more hours.** AMI's two channels and AliMeeting's two channels give the same words on close and distant audio, which separates distance from everything else. That costs one extra pass of 2.5 and 4.2 hours, cheaper than another set.
- **Standard subsets where they exist.** AMI EN2002 is part of the Full-corpus-ASR test partition, NOTSOFAR eval-small is the challenge's evaluation set and AliMeeting Eval is M2MeT's, so published numbers exist (with other normalisers and systems, so not directly comparable).
- **No tuning on them.** They are held out like the `talks` group: report them, do not tune on them (**inference**: tuning on a public test set makes later comparisons with published results meaningless).
- **Smaller downloads where there is a choice.** SUMM-RE's ASRU subset instead of its 102 GB dev and test parquet; AliMeeting Eval (3.7 GB) instead of Test (8.9 GB) or AISHELL-4's test (5.2 GB); NOTSOFAR per file.

## 4. Left out, and when to revisit

**Owner's call** (licence or access outside the rules; not fetched):

- **MLC-SLM** [S22]: the best fit for gap 3 that exists: natural two-person conversations in English, French, German, Italian, Portuguese, Spanish, Japanese, Korean, Russian, Thai and Vietnamese, all in our list (Thai in the adaptation tier). The evaluation references are public under CC BY-SA 4.0, but the audio comes from Nexdata after registration, and "By registering, you agree to the Data Use Agreement", which says the data "will only be used for the purpose of the workshop challenge" and that "redistribution or any other use is strictly prohibited". Worth asking Nexdata whether evaluation use outside the challenge is allowed.
- **VoxConverse** [S16]: "available to download for research purposes under a Creative Commons Attribution 4.0 International License. The copyright remains with the original owners of the video." Diarization only (no words). Useful for DER on many-speaker broadcast audio if the owner accepts the research-purpose wording.
- **MSDWild** [S17]: "The database is ONLY for research purposes", with a licensing agreement; diarization only; audio on Google Drive.
- **MagicData-RAMC** [S18]: CC BY-NC-ND 4.0.
- **Mixer 6** (CHiME-8 DASR) and **MMCSG** [S10, S11]: LDC request after challenge registration, and Meta registration ("should not be further distributed to not-registered individuals").
- **Basis Conversations 1500** [S23]: gated on Hugging Face, licence "other".
- **Santa Barbara Corpus** [S26]: CC BY-ND 3.0 US; converting its transcripts into our format is arguably a derivative.

**Fit the rules, not taken now:**

- **CHiME-6** [S9]: on OpenSLR (SLR150) under CC BY-SA 4.0 with no registration. Its dev set is one 11 GB archive for two dinner-party sessions recorded by Kinect arrays in kitchens and living rooms: the hardest public far-field audio and further from Diarist's use than office meetings. Revisit if home or kitchen recordings matter.
- **DiPCo** [S8]: CDLA-Permissive-1.0, but only one 13.4 GB archive; dinner parties again. NOTSOFAR-1 and AMI's array cover distant meeting audio for a tenth of the download.
- **AISHELL-4** [S15]: a second Mandarin meeting set with 4 to 8 speakers, array only. Add if AliMeeting's 2 to 4 speakers prove too few.
- **Earnings-21** [S12]: no word times in the references, but entity tags and human RTTMs; useful for an entity-error report, which the harness does not have yet.
- **EdAcc** [S24] and **Primock57** [S25]: two-person calls, which the owner's own Zoom data already covers; EdAcc's transcripts are upper case without punctuation.

## 5. How it is built (tools/eval)

- `public_data.py`: downloads with `C:\Windows\System32\curl.exe -L --ssl-revoke-best-effort -A diarist-research`, every file checked against a SHA-256 pinned in its script (Hugging Face and Git LFS files by their LFS oid, the others hashed at the first fetch on 26 Oct 2026); conversion to 16 kHz mono PCM with ffmpeg; reference writing; the per-set `sessions.toml`. When `DIARIST_EVAL_LOCKS` names a folder, no download larger than 50 MB starts while its `quiet.lock` exists, one running when it appears is stopped and resumed from its partial file, and smaller files go at 1 MB/s. `range_get()` does the same for HTTP range reads (Indic DiarBench's parquet).
- One fetch script per set writes `.work/public/<set>/` (gitignored): `audio/`, `refs/<session>.ref.seglst.jsonl` and `.ref.rttm`, `sessions.toml`, `meta.json` (sources, SHA-256 of every input, licence, reference counts). `--prune` deletes `download/`.
- Reference rows are the harness's SegLST with two optional fields: `timing` ("word", "interp" for a word without its own time, "segment" for a turn) and `words_pc`, the source's own punctuation and casing (AMI, ICSI, NOTSOFAR, Earnings-22 per word; AliMeeting and Indic DiarBench per turn). `words` stays plain so scoring is unchanged. Indic DiarBench rows also carry `words_mixed`, the code-mixed form with English words in Roman script. RTTMs come from the source where it has turns (AMI's pyannote setup, AliMeeting, Indic DiarBench) and otherwise from the word rows by the AMI only_words rule (adjacent words of a speaker merged, any pause splits).
- `sessions.public.toml` (committed, no private paths) includes every set's list. Sessions are kind `ref`, a second name for the existing `ami` path (prebuilt SegLST + RTTM), with `ref_source` naming the reference type in the report; no new code path was needed. Session files can `include` others (glob patterns allowed); an include that matches nothing is skipped with a note.
- run.py changes: a session's `language` overrides the config's (zh-CN, fr, hi), English-only ASR configs skip those sessions, `--groups` filters, and the report has one section per public group. Non-English text is scored with the multilingual normaliser that FLEURS already used, now in `common.py`; Mandarin per character (so cpWER there is a speaker-attributed CER); French and Mandarin hesitations are dropped on both sides as English fillers are.
- `smoke.py`: cut a clip of a session, run `nemo-speech transcribe` on it the way `nemo_direct.py` does (shipped lookahead 13 and diarizer preset v3-offline), score it.

## 6. Smoke runs

26 Oct 2026, 20:50 to 21:30, on the development PC (RTX 3080) through the agents' GPU queue, after no timed measurement held the quiet lock. `smoke.py` cut a 3-minute clip of one or two sessions per set and ran the installed runtime (`%LOCALAPPDATA%pp.diarist.desktop`, NeMo-Speech.cpp 0.2.0 CUDA, the app's q8_0 GGUFs) the way `nemo_direct.py` does, with the shipped file settings: `nemo-speech transcribe <wav> --model <asr> [--language <tag>] --stream --endpointing --diarize --diar-model Nemotron-3-Diarization.q8_0.gguf --device cuda:0 --json --output <out> --force --asr.streaming.rnnt_right_context 13 --asr.diar.preset v3-offline`. English sessions used Nemotron EN (no language tag, as the app does), the others Nemotron 3.5 with the session's tag. The clip's hypothesis is the runtime's own words and speaker tags (no app-side segmentation), so the numbers are not the app's and not a baseline: they show that each reference parses, lines up with its audio and scores. Percentages; Mandarin rows are per character. **Measured.**

| Session | Clip | ASR | Tag | Ref words | Hyp words | Ref / hyp speakers | DER c0.25 | WDER | cpWER | tcpWER | ORC-WER | cp del / ins / sub | Wall s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `ami-en2002a-mix` | 600 to 780 s | nemotron-en | – | 800 | 561 | 4 / 4 | 35.8 | 9.0 | 40.0 | 40.6 | 34.1 | 250 / 11 / 59 | 2.75 |
| `ami-en2002a-arr1` | 600 to 780 s | nemotron-en | – | 800 | 520 | 4 / 4 | 39.3 | 9.1 | 43.0 | 43.5 | 38.9 | 284 / 4 / 56 | 2.87 |
| `icsi-bmr019` | 900 to 1080 s | nemotron-en | – | 705 | 610 | 6 / 5 | 11.8 | 3.5 | 28.4 | 29.1 | 24.4 | 134 / 39 / 27 | 3.3 |
| `icsi-bed016` | 900 to 1080 s | nemotron-en | – | 568 | 320 | 5 / 5 | 54.5 | 9.5 | 51.6 | 52.6 | 47.5 | 256 / 8 / 29 | 5.54 |
| `nsf-mtg32000` | 60 to 240 s | nemotron-en | – | 669 | 462 | 4 / 4 | 26.1 | 8.9 | 39.0 | 41.0 | 33.9 | 212 / 5 / 44 | 3.06 |
| `e22-4481221` | 900 to 1080 s | nemotron-en | – | 367 | 369 | 2 / 2 | 14.4 | 0.3 | 9.5 | 9.8 | 9.0 | 7 / 9 / 19 | 3.27 |
| `ali-r8001_m8004-near` | 300 to 480 s | nemotron-3.5 | zh-CN | 1318 | 943 | 4 / 4 | 31.2 | 19.4 | 50.4 | 52.7 | 46.8 | 420 / 45 / 199 | 3.7 |
| `ali-r8001_m8004-far` | 300 to 480 s | nemotron-3.5 | zh-CN | 1318 | 661 | 4 / 4 | 45.6 | 10.5 | 62.7 | 64.0 | 62.3 | 675 / 18 / 134 | 3.9 |
| `summre-018a_earz` | 300 to 480 s | nemotron-3.5 | fr | 684 | 597 | 4 / 3 | 14.0 | 3.1 | 26.3 | 26.6 | 24.4 | 106 / 19 / 55 | 3.21 |
| `summre-032b_eadh` | 300 to 480 s | nemotron-3.5 | fr | 1002 | 593 | 4 / 4 | 41.0 | 13.3 | 53.2 | 53.7 | 48.9 | 415 / 6 / 112 | 2.85 |
| `idb-hindi-075` | 120 to 300 s | nemotron-3.5 | hi | 600 | 460 | 4 / 4 | 22.1 | 7.1 | 39.7 | 41.2 | 37.0 | 153 / 13 / 72 | 3.68 |
| `idb-hindi-054` | 120 to 300 s | nemotron-3.5 | hi | 869 | 644 | 3 / 3 | 26.5 | 10.7 | 44.1 | 45.7 | 42.5 | 250 / 25 / 108 | 2.97 |

What the clips show (one clip each, so indications only, **inference**):

- **Alignment is right everywhere.** tcpWER (5 s collar) stays within 2.3 points of cpWER on every clip, so no reference is shifted against its audio; that includes AliMeeting R8001_M8004, whose array recording is 22 s longer than its headsets.
- **Deletions dominate.** In 11 of 12 clips the errors are mostly missed words (cp del), as on the owner's AMI ES2002a. The meeting sets will measure the word-loss problem directly, with human references.
- **Distance costs a few points on the same words.** AMI EN2002a, same 3 minutes: cpWER 40.0 on the headset mix, 43.0 on the table microphone; AliMeeting R8001_M8004: 50.4 headsets, 62.7 array.
- **Speaker counts hold.** The diarizer found the reference speaker count in 10 of 12 clips (one short on ICSI Bmr019 and SUMM-RE 018a).
- **Hindi: Nemotron 3.5 writes English loanwords in Devanagari** (0 of 460 hypothesis words in Latin script on `idb-hindi-075`). Against the code-mixed form (`words_mixed`, English in Roman script) the same clip scores 47.6 percent WER instead of 38.2 (speaker-agnostic, normalised words), so the Devanagari form is the right default. Nuqta spellings (ज़्यादा against ज्यादा) still count as errors; a Hindi normaliser that folds them is a possible follow-up.
- **SUMM-RE in-room meetings are harder than its Zoom ones** (cpWER 53.2 on studio meeting 032b, 26.3 on Zoom meeting 018a): summing in-room headsets mixes each voice with the others' crosstalk. Report the two kinds apart (the label says studio, lab or Zoom).

## 7. Commands for the full runs

From `tools/eval`, once per machine (10.3 GB downloaded, 3.6 GB kept):

```sh
uv run python -I ami_test_fetch.py
uv run python -I icsi_fetch.py --prune
uv run python -I notsofar_fetch.py
uv run python -I earnings22_fetch.py --prune
uv run python -I alimeeting_fetch.py --prune
uv run python -I summre_fetch.py --prune
uv run --with pyarrow==21.0.0 python -I indic_fetch.py
```

Then per configuration (each writes `docs/eval/<date>-<config>-public.md`; the gate report of the same day is not touched):

```sh
# everything with the multilingual model, the app's own defaults
uv run python -I run.py --config shipped_n35 --sessions-file sessions.public.toml
# the English sets with the English default model (it skips pub-lang by itself)
uv run python -I run.py --config shipped_en --sessions-file sessions.public.toml
# one gap at a time
uv run python -I run.py --config shipped_n35 --sessions-file sessions.public.toml --groups pub-far
uv run python -I run.py --config shipped_n35 --sessions-file sessions.public.toml --groups pub-lang
# recompute the metrics only (no GPU)
uv run python -I run.py --config shipped_n35 --sessions-file sessions.public.toml --metrics-only
```

At the shipped configurations' speed in the beta.2 measurements (about 30 times real time including the standalone diarize pass, docs/eval/2026-10-16-shipped_n35.md), plus about 10 s of start-up per session (**inference** from the per-session wall times there): `shipped_n35` on everything (98 sessions, 32.4 h) about 80 minutes; `shipped_en` on the three English groups (52 sessions, 16.8 h) about 40 minutes; one group 10 to 35 minutes. Run them only on an idle machine (the GPU queue in the agents' locks folder applies).

## Sources

Accessed 26 Oct 2026; dates in brackets are publication or last change where the source gives one.

- [S1] AMI Meeting Corpus licence: https://groups.inf.ed.ac.uk/ami/corpus/license.shtml ("The AMI corpus and its annotations are released under the Creative Commons Attribution 4.0 license agreement (also called CC BY 4.0).")
- [S2] AMI download page and mirror: https://groups.inf.ed.ac.uk/ami/download/ (manual annotations `ami_public_manual_1.6.2.zip`; audio at `AMICorpusMirror/amicorpus/<meeting>/audio/<meeting>.<stream>.wav`)
- [S3] AMI partitions: https://groups.inf.ed.ac.uk/ami/corpus/datasets.shtml; pyannote/AMI-diarization-setup, Apache-2.0, commit `67c2d53` [24 Oct 2022]: https://github.com/pyannote/AMI-diarization-setup
- [S4] ICSI corpus and licence: https://groups.inf.ed.ac.uk/ami/icsi/, https://groups.inf.ed.ac.uk/ami/icsi/license.shtml ("All of the signals and transcription, and some of the annotations, have been released publicly under the Creative Commons Attribution 4.0 International Licence (CC BY 4.0).")
- [S5] ICSI download: https://groups.inf.ed.ac.uk/ami/icsi/download/ (Headset mix about 120 MB per meeting; "ICSI core annotations v1.0 22-July-2016 (19MB)")
- [S6] NOTSOFAR-1 challenge repository: https://github.com/microsoft/NOTSOFAR1-Challenge ("The data provided in this repository is licensed under the Creative Commons Attribution 4.0 International License (CC BY 4.0)."; 237 meetings, about 6 minutes, 30 rooms, 4 to 8 attendees, 35 speakers)
- [S7] microsoft/NOTSOFAR on Hugging Face: https://huggingface.co/datasets/microsoft/NOTSOFAR (API: gated false; revision `ba8fd0f` [21 Jun 2026]; LICENSE.txt CC BY 4.0)
- [S8] DiPCo on Zenodo: https://zenodo.org/records/8122551 (licence cdla-permissive-1.0; DipCo.tgz 13,415,522,146 bytes [25 Jul 2020])
- [S9] CHiME-6 on OpenSLR: https://www.openslr.org/150/ ("License: Attribution-ShareAlike 4.0 (CC BY-SA 4.0 International)"; CHiME6_dev.tar.gz 11G, CHiME6_eval.tar.gz 12G); https://www.chimechallenge.org/challenges/chime6/download
- [S10] CHiME-8 DASR data: https://www.chimechallenge.org/challenges/chime8/task1/data (Mixer 6: "Each team should file a request for Mixer 6 Speech data by compiling this PDF Form")
- [S11] CHiME-8 MMCSG data: https://www.chimechallenge.org/challenges/chime8/task3/data ("a registration is needed to obtain the data")
- [S12] Earnings-21: https://github.com/revdotcom/speech-datasets/tree/main/earnings21 (README, LICENSE.md, rttms/README.md)
- [S13] Earnings-22: https://github.com/revdotcom/speech-datasets/tree/main/earnings22 (README, LICENSE.md, metadata.csv, transcripts/force_aligned_nlp_references); repository README and release notes; commit `c05ab6f` [29 May 2026]
- [S14] AliMeeting, OpenSLR SLR119: https://www.openslr.org/119/ ("License: CC BY-SA 4.0"; Eval_Ali.tar.gz 3.42G)
- [S15] AISHELL-4, OpenSLR SLR111: https://www.openslr.org/111/ ("License: CC BY-SA 4.0"; test.tar.gz 5.2G)
- [S16] VoxConverse: https://github.com/joonson/voxconverse (README "License"), https://www.robots.ox.ac.uk/~vgg/data/voxconverse/
- [S17] MSDWild: https://github.com/X-LANCE/MSDWild
- [S18] MagicData-RAMC, OpenSLR SLR123: https://www.openslr.org/123/ ("License: Attribution-NonCommercial-NoDerivatives 4.0 International Public License (CC BY-NC-ND 4.0)")
- [S19] SUMM-RE: https://huggingface.co/datasets/linagora/SUMM-RE (card: "License: CC BY-SA 4.0"; API gated false; revision `47cb4ec` [15 Oct 2024])
- [S20] SUMM-RE ASRU: https://huggingface.co/datasets/datasets-CNRS/summ-re-asru (card: "license: cc-by-sa-4.0", from https://www.ortolang.fr/market/corpora/summ-re-asru; API gated false; revision `de4e37a` [29 Mar 2025])
- [S21] Indic DiarBench: https://huggingface.co/datasets/sarvamai/indic-diarbench (card: "This dataset is released under the CC BY 4.0 license."; API gated false; revision `92877ba` [11 Aug 2026]); paper https://arxiv.org/abs/2607.23808 [26 Jul 2026]
- [S22] MLC-SLM: evaluation references https://huggingface.co/datasets/bsmu/MLC-SLM-Eval (CC BY-SA 4.0, text only: "we only provided the participants with the audio files of the Eval sets"); challenge page https://www.nexdata.ai/competition/mlc-slm (Data Use Agreement)
- [S23] Basis Conversations 1500: https://huggingface.co/datasets/basis-ai/basis-conversations-1500 (API: gated manual, licence other)
- [S24] EdAcc: https://huggingface.co/datasets/edinburghcstr/edacc
- [S25] Primock57: https://github.com/babylonhealth/primock57 (LICENSE.md: Creative Commons Attribution 4.0 International)
- [S26] Santa Barbara Corpus of Spoken American English: https://www.linguistics.ucsb.edu/research/santa-barbara-corpus-spoken-american-english ("licensed under a Creative Commons Attribution-No Derivative Works 3.0 United States License")
- [S27] `manifests/languages.json` in this repository
