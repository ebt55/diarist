# Plan 20: the human set, a human-checked reference from the owner's own Diarist meetings (beta.7)

Status: designed 31 Oct 2026 by Fable 5.1 for beta.7, from the app's database read only (schema 5, 19 meetings; counts and durations only, no transcript text was read into this file), the eval harness (`tools/eval`), the gap-fill note (`docs/eval/2026-10-30-gapfill.md`, chunk N1), plan 15 section 2, plan 16 sections 12.1 to 12.2 and 12.7 (iv), and plan 18 sections 3.3, 5.1 and 7.2. Design only: no code, no GPU run, no audio copied. Plan 19 is the beta.6 release checklist. The owner agreed to the shape on 31 Oct 2026: "make my job easier by you picking the thing and i will label it."

## 0. For the owner, in plain words

Almost every number we have about missed words on your own recordings is scored against Zoom's own machine transcript, and Zoom misses much of the same speech our models miss. Chunk N1 showed what that does: filling the EN model's gaps with Nemotron 3.5's words helped on every set a person had checked (AMI and the public sets, about one point of word error and half the long gaps) and slightly hurt on your Zoom sessions, most likely because the recovered words were scored as insertions. Missed words are your top complaint and four of five English errors, so the decisions about them need a reference a person checked, on your audio, with your voice on it. Nobody else can be that person: the recordings cannot leave this PC and only you know what was said.

This plan keeps your part small and keyboard-only. Agents pick about 48 short excerpts (20 to 40 seconds each, about 24 minutes of audio in all) from your Diarist meetings by a rule fixed in advance, so the choice cannot favour one model; they transcribe each excerpt with every model on this PC and line the readings up; they build a local page that plays an excerpt, shows one line of text with the disputed spots marked, and lets you settle each spot with one key: pick reading A, B or C, mark a word as not said, mark a span as "can't tell", or mark a "me" word as yours or as echo. Typing is for the rare word no model heard, by voice (Win+H) into one field. Four sittings of about 25 minutes, roughly 1 h 40 min in all, done when you like, one sitting at a time. Everything stays on this PC outside every repository; the committed notes carry only counts.

## 1. What is in the app today (read 31 Oct 2026, read only)

`C:\Users\ebin\AppData\Local\app.diarist.desktop\diarist.db`, schema 5. 19 meetings, 417 minutes. Each live meeting's folder holds `playback.flac` (16 kHz stereo, left the mic, right the system-audio loopback, raw per-channel sample indices; plan 15 section 2), the runtime's words per channel (`loopback.words.json`, `mic.words.json` or `mic-aec.words.json`) and `engine.log`; the WAVs are deleted after the post-pass (plan 16 section 12.1 fact 1). The `words` table carries no usable confidence (every stored confidence is 0 or 1), so hardness has to come from disagreement between readings, not from the model's own doubt.

| Meeting | Date | Length (min) | Far-end speech (min) | Stored "me" words (speech min) | Far-end speakers (post-pass) | Echo state of the stored run | Kept live run |
|---|---|---|---|---|---|---|---|
| 12 | 18 Oct | 111.6 | 39.2 | 14,627 (44.6), of which 12,434 are echo (plan 16 section 12.1) | 8 | speakers, recorded before any gate existed | no |
| 13 | 20 Oct | 8.4 | 2.8 | 386 (1.1) | 2 | clear, round-one build (coupling 0.13, dropped 0) | yes |
| 14 | 20 Oct | 8.3 | 3.0 | 340 (1.0) | 2 | coupled, return loss 14.1 dB, canceller, 29 dropped | yes |
| 15 | 21 Oct | 6.7 | 2.5 | 187 (0.5) | 2 | clear, headphones | yes |
| 16 | 21 Oct | 7.5 | 2.8 | 293 (0.9) | 2 | coupled, 14.9 dB, 73 dropped | yes |
| 17 | 22 Oct | 6.6 | 2.8 | 160 (0.5) | 3 | coupled, 18.0 dB, 3 dropped | yes |
| 18 | 22 Oct | 11.3 | 4.2 | 757 (2.3) | 2 | coupled, 8.9 dB, 12 dropped (the known case, plan 16 section 12.2.4) | yes |
| 20 | 23 Oct | 95.1 | 29.6 | 1,245 (4.0) | 8 | coupled, 4.2 dB, 1,381 dropped | yes |
| 21 | 23 Oct | 51.6 | 23.3 | 288 (0.8), of which about 251 are echo (plan 16 section 12.1) | 5 | coupled, 4.6 dB, silent tier, 1,775 dropped | yes |
| 22 | 27 Oct | 0.4 | 0 | 26 (0.1) | 0 | a 24 s test | yes |
| 4, 5, 6, 7, 10, 11 | 16 to 17 Oct | 94.3 in all | 94.3 | 0 (silent mic) | 1 to 6 | no coupling possible | no |
| 9 | 17 Oct | 1.8 | 1.8 | 0 | 1 | Hindi; silent mic | no |
| 1, 19 | 11, 18 Oct | 11.2, 1.8 | single track | file imports (an .mkv; a clip from another project) | 1, 6 | not the live path | no |

What that gives the set: nine two-track meetings with the owner's voice, 307 minutes, about 110 minutes of far-end speech and about 20 minutes of the owner's own speech (meeting 12's roughly 1,462 own words in 27 stretches, plan 15 section 2, plus about 11 minutes elsewhere); far-end speaker counts from 2 to 8; three echo regimes (no gate, canceller with a strong return loss, canceller with a weak one); two headphone controls (13, 15); and 94 minutes of far-end-only audio with no echo at all. Long stretches with no word on either channel: 36 of 5 s or more on meeting 12, 43 on 20, 3 on 21; whether they are silence or missed speech is one of the things the set answers.

## 2. Decisions at a glance

| Id | Decision | Where |
|---|---|---|
| D1 | 48 excerpts of 20 to 40 s (about 24 min of audio), both tracks labelled, picked by a pre-registered rule over seven classes with per-meeting caps; 4 are repeated blind for self-agreement; four sittings of 12 to 13 items | section 3 |
| D2 | The draft is a word lattice of four readings (the stored post-pass, fresh EN and 3.5 runs on the raw channels, the kept live run) plus one reading from a different model family on the excerpts only; agreement is prefilled, disagreement is a spot with shuffled unlabelled alternatives and no default | section 4 |
| D3 | Correctness follows the harness normaliser; fillers, casing, punctuation and number spelling do not count; partial words are written with a trailing hyphen and dropped; "can't tell" spans are excluded from scoring on both sides; "me" words are `own`, `echo` or `unclear` | section 5 |
| D4 | One offline HTML file, keyboard-first, no network, three WAVs per excerpt for channel solo, folder-handle autosave with a localStorage fallback, both themes, model names never shown | section 6 |
| D5 | Output is meeteval SegLST per excerpt and lane plus an `unscored` span list; a private `human` group enters the harness through a gitignored session file; it judges gap fill, runtime deletions, echo rules and model candidates with bars fixed before each run | section 7 |
| D6 | Everything lives in one private folder outside every repository; committed files carry counts, class names and "Own N" labels only; no meeting text anywhere | section 8 |
| D7 | Three Opus agents: pool runs, selection and drafts (A); the UI (B, in parallel); harness integration and the first scoring pass (C, after B's format is fixed); about 25 minutes of GPU in all, on an idle machine | section 9 |

## 3. D1: size and selection

### 3.1 Size

- **48 excerpts, 20 to 40 s each, target 30 s**, about 24 minutes of audio, about 2,800 to 3,400 reference words at meeting speech rates (both lanes). Hard classes are over-sampled on purpose, so the set carries several hundred model errors, enough to see the direction of a change on the owner's audio; it is not a replacement for the 36-session set's statistics and is never pooled with it.
- **4 repeats.** Four excerpts (one easy, three hard) appear a second time in the last sitting under new ids; the owner is not told which. Their agreement is the set's own precision figure (section 7.4).
- **Owner's time.** Estimate 1.0 min per easy excerpt and 2.0 to 2.5 per hard one (one listen at 1× plus one replay of each spot, 3 to 8 keypresses per spot, under one voice-typed word per excerpt): 8 easy plus 40 hard plus 4 repeats is about 100 minutes. Four sittings of 12 to 13 items, about 25 minutes each, on the owner's schedule. The page records seconds per excerpt; **the pace rule, fixed now:** if sitting 1 runs over 30 minutes, the agent removes the lowest-ranked excerpt of each hard class per 5 minutes over (section 3.3's ranks), never below 36 excerpts, and says so in the eval note. No excerpt is added after sitting 1.

### 3.2 Pool and exclusions

- **Pool for every class:** the two-track live meetings 12, 13, 14, 15, 16, 17, 18, 20 and 21 (307 min).
- **Pool for class E2 only:** the far-end-only meetings 4, 5, 6, 7, 10 and 11 (94 min; English, silent mic).
- **Excluded:** 1 and 19 (file imports, single track, not the recording path the set is for; 19 is a clip from another project), 9 (Hindi; the set is English), 22 (a 24 s test). On every pool meeting the first 30 s (the echo monitor couples at 18 to 36 s on the coupled meetings, so the stored words there are ungated) and the last 10 s. Any 30 s window whose two channels are both under the recording's noise floor plus 6 dB for its whole length (nothing to label).
- **The owner's veto.** Any excerpt the owner skips in the page (one key, no reason asked) is replaced by the next-ranked window of its class; the eval note counts replacements. This is the only human step in the selection.

### 3.3 The rule, fixed before any run

Windows of 30 s at a 10 s hop over each pool meeting, on meeting time (the clock anchors map the mic's raw indices to meeting time, as `crates/core/tests/echo_sample.rs` does; the mic clock runs 51.6 ppm against the loopback, plan 15 section 2). Per window and channel the selector computes, from the stored data and the pool runs of section 4.1 (no human looks at the text):

- **d**, disagreement: the Levenshtein word distance between the fresh EN and fresh 3.5 readings after the harness normaliser (`tools/eval/common.py:191-226`), divided by the longer reading's length; 0 where both are empty.
- **g**, suspected missed speech: seconds inside stretches of 2 s or more with no EN word (the gap definition of `tools/eval/metrics.py:200-217`) whose RMS over 100 ms frames is at least 12 dB over the channel's floor (the 5th percentile of frame levels) for at least half the stretch.
- **o**, double talk: seconds where a stored "me" word and a loopback word overlap in time.
- **e**, echo decisions: words in `transcript_runs.echo_dropped` inside the window plus kept "me" words whose span overlaps far-end speech (the "far-active" words of plan 16 section 12.2.3).
- **n**, names and numbers: tokens in the stored punctuated text that are capitalised and not sentence-initial, or hold a digit, plus tokens the two fresh readings spell differently where both are capitalised.
- **t**, turns: loopback speaker changes in the stored run.
- **a**, activity: seconds with any word, per channel.

Classes, quotas and ranks. Windows are taken greedily in rank order within each class, skipping a window that overlaps an already taken one by more than 10 s, and skipping a meeting once it holds its cap for that class. A window qualifies for the first class in this order whose test it meets, so the quotas do not double-count:

| Class | Quota | Test (per window) | Rank | Caps |
|---|---|---|---|---|
| H3 echo and double talk | 10 | o ≥ 3 s and e ≥ 5 | e, then o | 3 per meeting; at least 2 from 12, 2 from 20, 1 from 21, 1 from 18 |
| H2 suspected gaps | 8 | g ≥ 3 s on either channel | g | 2 per meeting |
| H1 disagreement | 12 | a ≥ 10 s on the channel and d ≥ 0.25 there | d | 3 per meeting; at least 3 windows ranked on the mic lane |
| H4 names and numbers | 6 | n ≥ 3 | n | 2 per meeting |
| H5 dense turns | 4 | t ≥ 4 and the meeting has 3 or more far-end speakers | t | 2 per meeting (12, 17, 20, 21 qualify) |
| E1 easy, two tracks | 4 | d ≤ 0.05 on both channels, g = 0, o < 0.5 s, a ≥ 15 s | random, seed 20261031 | 1 per meeting |
| E2 easy, far end only | 4 | as E1 on the loopback, from the E2 pool | random, seed 20261031 | 1 per meeting |

Spread: every two-track meeting of 6 minutes or more gets at least 2 excerpts in all; if a meeting ends with fewer, its best-ranked unused H1 window replaces the lowest-ranked H1 window of the meeting with the most. Within a meeting, a window whose dominant loopback speaker already dominates two taken windows is skipped while another candidate of the class remains. Boundaries: each taken window is moved to the nearest pause of at least 0.4 s on both channels within ±5 s at each end, so excerpts start and end between words; length stays within 20 to 40 s. Ties in any rank go to the earlier window.

The selector (`tools/eval/human/select.py`) writes a private manifest (excerpt id, meeting, start and end on meeting time, class, the seven signals, the seed, the hashes of the pool runs it read). The committed eval note carries the class counts per meeting and nothing else. The rule is not changed after the manifest exists; a flaw found later is a new rule with a new manifest, recorded as such.

### 3.4 Both tracks

Both lanes are labelled. The far end holds most words and the owner's first complaint (EN errors on the loopback, 892 confirmed of 22,739; plan 16 section 12.1 rank 1); the mic lane is where echo decisions and the owner's own double-talk words live (ranks 2 and 3), and it is the lane the Zoom references never had. In E2 excerpts only the far-end lane exists. In H3 excerpts the mic lane is mostly `own` / `echo` keys, not word fixes (section 5.2).

## 4. D2: the draft the owner corrects

### 4.1 Sources

| Source | What | Lane | Cost |
|---|---|---|---|
| S0 | the stored post-pass run (`segments`, `words`, `words.raw_text` for the pre-punctuation text; echo gate applied; speakers) | both | none |
| S1 | fresh EN run on the raw channel WAVs cut from `playback.flac`: loopback diarized at the file setting (lookahead 13, `v3-offline`), mic plain; the app's path through `diarist-cli transcribe` in a scratch data root, or `tools/eval/nemo_direct.py` with the same flags | both | GPU, section 9 |
| S2 | the same with Nemotron 3.5 | both | GPU |
| S3 | the kept live run (`live_utterances`, latest revision per utterance; lookahead 1 on meetings 13 to 21, the beta.4 live setting) | both, where kept | none |
| S4 | one reading from a different model family on the 48 excerpts only: Whisper distil-large-v3, which the private word-error analysis of 25 to 26 Oct already ran on these meetings (plan 16 section 12.1). The two Nemotron models miss the same stretches (gapfill note section 4: 3.5's own gaps cover half or more of 66 percent of EN's), so a third family is what proposes words where both have none; the owner only confirms by ear | both | GPU, a few minutes |
| S5, later | Nemotron 3.5 Transcribe 30B, CPU only, if the owner's spike T (plan 18 section 7) leaves its files on this PC: about 25 to 50 minutes of CPU for 24 minutes of audio (research/21 section 2.6). Added as a sixth reading on a later pass, never a replacement for the owner's labels | both | CPU, optional |

Why S1 and S2 are run fresh rather than taken from S0: S0's mic lane is after the echo gate, so a fresh raw run is what shows the gate's decisions as alternatives (a word present in S1 and absent in S0 is a gate removal; the owner's `own` or `echo` key judges it). The runs are also what the first scoring pass scores, so the drafts and the hypotheses come from the same words.

### 4.2 The lattice and the spots

Per excerpt and lane, the readings are aligned into one word lattice: words of different sources whose spans overlap by at least 40 percent of the shorter span, or that are adjacent to an aligned pair and equal after normalisation, share a slot; the rest form insertion slots. Slots are grouped into segments at pauses of 0.5 s or more in S0 or at S0 speaker changes. In each segment:

- **Agreed spans** (every present source equal after normalisation, S3 not counted when it alone disagrees) are prefilled as text, accepted unless the owner touches them.
- **Spots** are maximal runs of slots where sources differ, including the case where some sources have words and others none. A spot shows its distinct alternatives (after normalisation; the punctuated S0 form is shown where S0 is one of them) as A, B, C in an order shuffled per spot with the manifest's seed, plus "none of these" and "nothing was said here". **No alternative is pre-selected**, so the owner's choice, not the shipped model, decides the reference where models disagree. The page logs the order shown.
- **Missing-word candidates** (one source has words, the others none) are spots with two alternatives: the words, or nothing.
- **Mic lane in coupled meetings:** every slot also carries `own` / `echo` / `unclear`, prefilled from S0 (kept words `own`, `echo_dropped` words `echo`) and shown as a shape, not a colour alone; the owner flips with one key, or with Shift over a run.

Expected density, from N1 and plan 16 section 12.1: about 4 to 8 spots per hard excerpt and 0 to 2 per easy one; most settled by one key.

### 4.3 The rare typed fix

A word no source offers: the owner presses `I` (insert here) or `X` (replace this word), one field opens under the spot, and they dictate it with **Win+H** (Windows voice typing) or type it, then Enter. Win+H sends the dictated word or two to Microsoft's online recogniser (the meeting audio never does; it plays locally). If the owner prefers nothing to leave the PC, Windows 11's on-device **Voice access** dictates into the same field offline, or they type. Expected under one such fix per excerpt; the page counts them.

## 5. D3: what counts as correct

### 5.1 Words

Scoring uses the harness normaliser as it stands (`common.normalize_token`: whisper's `EnglishTextNormalizer` per token, fillers dropped, `ok` to `okay`), so the labelling instructions follow from it:

- Words as spoken, including repetitions and false starts ("I I think" keeps both). Casing, punctuation and whether a number is spelt or written in digits do not matter (the normaliser maps them).
- Fillers (`um`, `uh`, `hmm`, `mhm` and the rest of `common._FILLERS`) may be left in or out; they are dropped on both sides. Backchannel words (`yeah`, `right`, `okay`) are words and stay.
- **Partial words** are written with a trailing hyphen (`fr-`). The normaliser gains one rule (chunk C): a token ending in `-` is dropped on both sides, so a model's guess at the fragment is neither rewarded nor punished. This is a 3-line change in `common.py` with a test in `test_metrics.py`, and it changes no existing score (no reference today carries such tokens; the Zoom and public references are checked for `-$` tokens before the change lands).
- **Names:** spelt as the owner knows them. A name whose spelling the owner does not know is marked "can't tell" rather than guessed, so a spelling difference is not scored as a substitution either way.
- **Non-English words:** as heard; if the owner cannot, "can't tell".
- **Can't tell:** any span the owner cannot settle after two replays (garble, two people at once, too quiet). One key over the spot or over a drag selection. Excluded from scoring on both sides (section 7.1). Expected to be a few percent of the audio; the eval note reports the seconds.

### 5.2 Speakers

- **Mic lane:** every word is `own`, `echo` or `unclear`. `own` words enter the reference as speaker `me`. `echo` words leave the mic lane's reference (their text belongs to the far-end lane, where it is labelled). `unclear` words become a "can't tell" span.
- **Far-end lane:** speakers are letters per meeting (A, B, C …), consistent across that meeting's excerpts: the page shows the meeting's speaker chips, prefilled from S0's labels mapped once per meeting, and the owner changes a segment's speaker with `F` and a digit, or splits a segment at the cursor with `/` when two people speak inside it. Stored display names are never shown or written (most far-end speakers in these meetings are unnamed in any case). If the owner's own voice appears on the loopback (sidetone, "Listen to this device"), it is labelled `me` there.
- Two people at once on the far end: label the audible one; if both are audible and separable, split; if not, "can't tell".

### 5.3 Time

The owner never edits times. A reference word takes the span of the aligned source word the owner picked (S0's where the span was agreed); a voice-typed word takes a span interpolated between its neighbours inside the segment. The scorer is tolerant by design: cpWER does not use word times, tcpWER uses a 5 s collar (`metrics.TCP_COLLAR_S`), gaps use word midpoints with a 0.5 s margin (`GAP_MARGIN_S`), and Nemotron word times run about 0.3 s late (metrics.py docstring). Segment times are the reference segments' bounds; the RTTM derives from them as `run.py` does for hand-corrected references (`run.py:390`).

## 6. D4: the labelling page

### 6.1 Shape

One file, `tools/eval/human/label.html` (committed; it holds code, no data), opened from the private set folder, where it finds `set.json` (the lattice, spots, speaker chips, sitting order) and the excerpt WAVs next to it. Vanilla HTML, CSS and JavaScript, no framework, no font or script from the network, no network at all; it works from `file://` in Edge or Chrome. Both themes follow `prefers-color-scheme`, with a toggle; everything reachable by keyboard; every key has a visible button for mouse use; focus always visible; text at 18 px or more; spots marked by an underline and a glyph, never colour alone; `own` / `echo` / `unclear` as three shapes.

### 6.2 Audio

Three WAVs per excerpt (16 kHz, 16-bit): the stereo mix as recorded (left mic, right loopback), the mic alone and the loopback alone, so the page solos a channel by switching the `<audio>` source at the same position (a Web Audio graph on `file://` media is silenced by cross-origin rules, so it is not used). Keys: `Space` plays or pauses the current segment and stops at its end; `R` replays the current spot with 1 s of context; `L` loopback only, `M` mic only, `B` both; `S` toggles 0.75× speed; the text cursor follows playback by word times. For a mic-only solo in a coupled meeting the owner hears the echo as it reached the mic, which is the question the `own` / `echo` key answers.

### 6.3 Keys

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | next / previous spot (across segments and lanes) |
| `↓` / `↑` | next / previous segment |
| `1` `2` `3` | pick alternative A, B, C for the spot; `0` none of these (then `I` or `X` to dictate); `N` nothing was said here |
| `D` | the word under the cursor was not said |
| `I` / `X` | insert a word here / replace this word: one field, Win+H or typing, `Enter` |
| `U` | mark the spot (or the current drag selection) "can't tell" |
| `O` / `E` / `?` | mic lane: this word is own / echo / unclear; with `Shift`, the whole run up to the next pause |
| `F` then a digit | far-end speaker of the segment; `/` splits the segment at the cursor |
| `K` | skip this excerpt (no reason asked); the agent replaces it |
| `Enter` on the excerpt's last spot, or `PageDown` | mark the excerpt done, go to the next |
| `Ctrl+S` | save now (autosave runs after every action anyway) |
| `H` | the key map |

### 6.4 Saving and export

On opening, one click grants a handle to the set folder (File System Access API); the page then writes `progress.json` after every action and `labels.sitting-N.json` when the owner presses "End sitting" (one click), both without further prompts. If the handle is refused or the browser lacks the API, the page keeps progress in `localStorage` and "End sitting" downloads the labels file (one Save dialog). Each saved action carries the excerpt id, lane, slot ids, the key, the alternatives' shown order, and a timestamp, so the agent can rebuild the reference and the time per excerpt. The agent's `tools/eval/human/export.py` turns the labels into SegLST and the unscored list (section 7.1).

### 6.5 Blindness and bias

- Model names are never shown; alternatives are letters in a shuffled order; the first screen of each sitting says only "pick what was said".
- The one thing not blind: agreed spans are prefilled, and all agreed sources are Nemotron-family or the kept live run except S4. The eval note states this and the share of reference words that came from agreed spans, so a reader knows how much of the reference the owner only confirmed by listening rather than chose.
- Sittings mix classes and meetings in a fixed shuffled order (the seed), so fatigue does not land on one class; the four repeats sit in the last sitting under fresh ids.

### 6.6 Testing the page without private data

Chunk B builds `set.json` for a public excerpt (AMI ES2002a, the harness's `data\samples\ES2002a.Mix-Headset.wav` and its word reference, with two altered copies of the reference as fake readings) and checks: every key, both themes, screen-reader labels on the buttons, autosave and export round trip, a 40 s excerpt with 12 spots in under 2.5 minutes by the agent's own run, no network request in the browser's log, and the page opening from `file://` with the folder handle and without it.

## 7. D5: the output and how it is used

### 7.1 Format

Per excerpt, in the private set folder under `refs/<excerpt id>/`:

- `far.ref.seglst.jsonl` and `me.ref.seglst.jsonl`: meeteval SegLST, one segment per line, `{"session_id", "speaker", "start_time", "end_time", "words"}`, times relative to the excerpt WAV, speakers `A`, `B`, … on the far lane and `me` on the mic lane; `far.ref.rttm` and `me.ref.rttm` derived.
- `unscored.json`: `[[start, end], …]` per lane, the "can't tell" spans.
- `labels.json`: the owner's raw actions (audit; never leaves the folder).
- `meta.json`: meeting id, offset on meeting time, class, length, the sources' file hashes, the seed, the owner's seconds on it, repeat-of (for the four repeats).

### 7.2 The harness

- A gitignored `tools/eval/sessions.human.toml` (the `include` mechanism of `sessions.example.toml:13`) lists two sessions per excerpt: `own-<n>-far` (audio: the loopback WAV, diarized) and `own-<n>-me` (audio: the mic WAV, `--no-diarize`, the app's mic setting), `kind = "ref"`, `ref_source = "owner-human"`, `group = "human"`, `label = "Own <n> far" / "Own <n> me"`, `uem = [0, length]`, and a new key **`unscored`**: the span list. `metrics.meeting_metrics` gains the key: reference and hypothesis words whose midpoints fall in an unscored span are dropped before every measure, and `word_gaps` treats those spans as covered, so no gap is counted inside them. The run cache key is unchanged; `.gitignore` gains the file.
- The report gains a `human` group (per-lane rows, pooled far and pooled me, by class), and `levers.py baseline` pools it like the others. Numbers only, labels "Own N".
- **Echo decisions are judged on the stored words, not by re-recording:** `echo_sample.rs` already replays every gate variant over a meeting's stored words and prints kept and dropped per path (its header). Chunk C adds a labels input: for each excerpt of a coupled meeting it counts, over the words inside the excerpt, kept-`own`, kept-`echo`, dropped-`own`, dropped-`echo`. "Owner's words kept" and "echo kept" become labelled counts, which is what plan 18 section 5.1 wanted for meeting 20 and could only get for ten words.

### 7.3 What it judges, and the bars

The set is small, so it decides direction and confirms, with bars fixed before each run, and it is never tuned on: a candidate is chosen on the 36-session set or the public sets as today, then read once on the human set. Decisions queued for it:

1. **Gap fill, reopened** (gapfill note section 7, last bullet). A new rule written before the run, on the human set's far and me lanes, EN primary: the fill ships only if pooled WER falls, deletions fall by at least 10 percent, insertions rise by no more than half the deletions saved, and no class is worse by more than 1 WER point. Rule 2 of plan 18 section 3.3 (the 36 sessions) becomes a report, not a bar, because the Zoom references are a recogniser's.
2. **Runtime deletions** (plan 18 section 4.4, R2's shape): pooled deletions per model on the human set no higher than the incumbent's by more than the N2 phase band (2.70 percent EN, 3.30 percent 3.5), read beside the broad set.
3. **Echo rules** (plan 16 section 12.7 (iv), plan 18 section 5): on the H3 excerpts a variant passes only if kept-`echo` does not rise and dropped-`own` does not rise, by the counts of 7.2; the "far-active cut words" proxy is reported beside them and retired as a bar once the set exists.
4. **Model candidates** (Parakeet Unified EN, Transcribe 30B's spike T in plan 18 section 7.2, NVIDIA's republished GGUFs): the "owner's Zoom session" bar of 7.2 moves to the human set's far lane.
5. **The owner's complaint classes** of plan 16 section 12.1 (far-end errors, echo in "me", the owner's own words) re-measured per release on the same excerpts, in the release note's human-set rows.

Noise floor, stated so no bar is set below it: with about 3,000 reference words, one error is about 0.03 WER points; a difference under 0.5 points pooled, or under 10 words in a class, is not read as a result. The repeats' agreement (7.4) sets the floor for the mic lane's `own` / `echo` counts.

### 7.4 The first scoring pass (what the eval note holds)

`docs/eval/<date>-human-set.md`: the class counts per meeting; reference size per lane (words, segments, unscored seconds, agreed-span share, voice-typed words, skipped and replaced excerpts); the owner's minutes per sitting and per class; the repeats' agreement (words changed between the two labellings, `own` / `echo` flips); EN and 3.5 at the shipped settings per lane and class (cpWER, tcpWER, WER with del / ins / sub, WDER on the far lane, gaps of 2 and 5 s); S4's figures beside them for the record; the shipped gate's four counts on H3; and, read once for the record, what the N1 fill at G = 2.0 s does on this set, as a first look before the reopened rule of 7.3 item 1 is written. No meeting text, no names.

## 8. D6: privacy

- **One folder, outside every repository and outside the app's data folder:** `C:\Users\ebin\diarist-private\human-set\` (the owner may name another). It holds the read-only database snapshot (made with SQLite's backup API from a `mode=ro` connection, never by copying the live files), the channel WAVs, the pool runs, the manifest, `set.json`, the excerpt WAVs, the labels and the references. Agents read the app's folder only; they never write into it and never run the app's post-pass on it.
- **The repository gets code only:** `select.py`, `label.html`, `export.py`, the harness changes, and eval notes with counts. `sessions.human.toml` is gitignored like `sessions.toml`. The privacy scan of plan 17 section 11 runs before every push; it gains the set folder's name as a forbidden string.
- **No meeting text in any committed file, note, commit message, chat message or agent report.** Agents report counts; where they must refer to a spot they use excerpt id, lane and slot ids. Scratch copies and the TSV dumps of `echo_sample.rs` are deleted after each step.
- **The owner's voice-typed words** go through Win+H's online recogniser unless the owner chooses Voice access or typing (section 4.3); the recording audio never leaves the PC.
- **Web requests:** none in the page; the agents' only downloads are model files already on this PC (S4 is installed). No owner name or email in any header, URL or payload, as every brief says.

## 9. D7: work breakdown

### 9.1 Chunks

| Chunk | Agent | Work | GPU | Agent time |
|---|---|---|---|---|
| A1 prep | A | the private folder; the database snapshot; per-meeting channel WAVs from `playback.flac` on meeting time via the clock anchors (ffmpeg split, 16 kHz mono); `select.py` with the signals of 3.3 and unit tests on synthetic words | none | half a day |
| A2 pool runs | A | S1 and S2 over the pool: about 400 min of loopback and 307 min of mic, both models, file settings, through the shared GPU queue, one job at a time, the GPU idle first (`gpu.py`; the heavy-runs rule) | about 18 min at 80 × real time | an idle window |
| A3 selection and drafts | A | the manifest; excerpt WAVs (three per excerpt); the lattice and spots; `set.json`; S4 on the 48 excerpts; a dry run of the page on one excerpt by the agent (no listening judgement, only that it loads) | S4 about 3 min | one day |
| B the page | B, in parallel with A | `label.html` to section 6, tested on the public excerpt of 6.6; `export.py`; the `set.json` schema fixed in a short `tools/eval/human/README.md` first so A3 and C build to it | none | one day |
| C harness | C, after B's schema | `kind = "ref"` with `unscored`, the `human` group, `sessions.human.toml` from the manifest, the hyphen rule in `common.py`, the labels input to `echo_sample.rs`; `test_metrics.py` cases for `unscored` and the hyphen rule | none | half a day |
| Owner | Ebin | four sittings of about 25 minutes, one at a time, any order of days; `K` to skip anything; "End sitting" at the end of each | none | about 1 h 40 min |
| A4 after sitting 1 | A | the pace rule of 3.1 if it trips; replacements for skipped excerpts; `export.py` on each sitting's labels | none | an hour per sitting |
| C2 first scoring pass | C | the two sessions per excerpt through `run.py` for `shipped_en` and `shipped_n35`; the gate counts on H3; the note of 7.4 | about 2 min (48 min of audio, two models) | half a day |

GPU in all: about 25 minutes, every run with the GPU idle first and nothing timed, so no quiet.lock is needed. CPU: the alignments and the page are light. Disk: about 2 GB in the private folder (the WAVs dominate), deleted down to the excerpts and references once the first pass is written, if the owner wants.

### 9.2 Order

```
now:            A1 (prep, select.py)        B (schema README, then the page)
idle GPU:       A2 (pool runs, about 18 min)
then:           A3 (manifest, excerpts, drafts, S4)       C (harness, to B's schema)
owner:          sitting 1  ->  A4 (pace rule, replacements)  ->  sittings 2 to 4, A4 after each
then:           C2 (first scoring pass, eval note)
beta.7:         the decisions of 7.3 read on the set, each with its rule fixed before its run
```

About one week of agent work to the first sitting; the owner's four sittings on their own days; the first scoring pass the day after the last sitting. Nothing here touches beta.6: chunk M2's ten-word listening step (plan 18 section 5.1) stays as written and happens first; once the set exists, the H3 excerpts give the same answer at scale for later echo work.

### 9.3 Briefs, shared files and checks

- **A** owns `tools/eval/human/select.py`, `tools/eval/human/draft.py` (lattice and `set.json`), the private folder; must not touch `metrics.py`, `run.py`, `common.py`, `label.html`. Reads the app's folder read only; never writes there; deletes scratch after each step.
- **B** owns `tools/eval/human/label.html`, `export.py`, `README.md` (the schema, the key map, the instructions page the owner sees first, in plain words); must not touch anything under `tools/eval/*.py`. No private data in tests; the public excerpt only.
- **C** owns `tools/eval/common.py` (the hyphen rule, append-only), `metrics.py` (`unscored`), `run.py` (the key and the group), `test_metrics.py`, `.gitignore` (one line), `crates/core/tests/echo_sample.rs` (the labels input, append-only), `docs/05-eval-data.md` (a `human` group row), `tools/eval/README.md` (a section), `docs/eval/<date>-human-set.md`.
- Checks as plan 18 section 13: `cargo fmt`, `clippy -D warnings`, `cargo test --workspace` where Rust changes; `uv run python -I test_metrics.py`; the privacy scan before any push; small commits with `git commit -- <paths>`; one agent per chunk; no GPU or heavy CPU while the owner's jobs use the machine, and a check of `nvidia-smi` before each run.
- Every brief repeats: no meeting text in any output the agent produces, counts only; no owner name or email in any request.

## 10. Decisions only the owner can make

| Decision | Options | Recommendation | If no answer |
|---|---|---|---|
| Both lanes or the far end only | both; far end only (about 20 percent less time) | **Both.** The mic lane is the one no reference has ever covered and where the echo rules are judged | both |
| S4, a different model family in the draft | Whisper distil-large-v3 on the excerpts; none | **Yes.** It is already on this PC and it is what proposes words both Nemotron models missed; the page never says which reading is whose | yes |
| The far-end-only meetings 4 to 11 as easy controls (E2) | in; out | **In**, four excerpts; they are the only echo-free far-end audio of the owner's | in |
| Typed fixes | Win+H (online); Voice access (offline); typing | **Win+H**, with the note of 4.3; the word or two dictated is all that leaves the PC | Win+H |
| The private folder | `C:\Users\ebin\diarist-private\human-set\`; another path | the default; anything outside every repository and the app's folder | the default |
| Pace | the trim rule of 3.1 if sitting 1 runs over 30 minutes; or accept a longer set | **The trim rule** | the trim rule |
| Keeping the pool WAVs after the first pass | delete down to the excerpts and references (about 150 MB); keep (about 2 GB) | **Delete**; they are rebuilt from `playback.flac` in minutes if needed | delete |
