# Eval: spike L, word boosting for names on the re-converted EN file (30 Oct 2026)

Plan 18 spike L (`docs/plans/18-beta6.md` section 3.6), the brief of plan 16 section 12.4 applied to the one re-converted EN file that survives: does RNNT word boosting in runtime 0.2.0 fix names without damage? Measurement only; no app code changes and no name source is built here. Opus 5.5 measurement agent. The owner's meeting is private: only numbers and the docs/05 session label appear below, never a name or any text.

## The kill rule, as fixed before any run

Plan 16 section 12.4 (and section 11), unchanged by plan 18: "the name source is built only if, at a boost where the 20-absent-names control costs at most 0.2 WER points, at least half the target names are fixed and no boosted phrase is inserted where the reference lacks it more than once per hour."

The runs, from plan 18 section 3.6: research/12's AMI clip test (`tools/eval/nemo_direct.py` on the 5-minute clip; targets Craig, beagle, Planet of the Apes; boosts 0.5, 1 and 2; the 20-absent-names control), "plus one of the owner's meetings with the most name errors (private; numbers only) with the speakers' names the owner gave as the phrase list, through the same path at the file setting (lookahead 13, `v3-offline`)". If it passes, the name source is a beta.7 chunk (and so is hosting a boostable EN file); if it fails, the KNOWN-ISSUES names row quotes the numbers.

## How each clause is read (fixed before any run)

The rule leaves these open; they were fixed here, before the first boosted run, and are not changed after.

- **Setting.** The verdict is read at the file setting, `--asr.streaming.rnnt_right_context 13 --asr.diar.preset v3-offline` on top of nemo_direct.py's `--stream --endpointing --diarize` (the app's file pass; no `--language`, as the app runs the EN model), for the clip and the meeting alike, because a name source would act on the file pass. The clip is also run at research/12's setting (the runtime's default lookahead, nothing added), for continuity with research/12's table only.
- **File.** `nemotron-speech-streaming-en-0.6b.q8_0-pwf16.gguf`, 770,986,624 bytes, SHA-256 `3d37c1c7e1fe8aea08389c188598895e90f3f8f33b34442db1d53209fac2a1da` (research/19 section 1), a byte copy of the file in the echo-copyright worktree's `.work/keep-fixed-en`. It embeds the SentencePiece tokenizer; the runtime's stderr is checked for "boosting disabled" on every boosted run.
- **Clip.** AMI ES2002a Mix-Headset, 50 to 350 s, cut as research/12 did (`ffmpeg -ss 50 -t 300 -i ES2002a.Mix-Headset.wav -ac 1 -ar 16000`), reference words of `ami` whose start lies in the window (as `levers.py boost`). Phrases `--speech-context Craig --speech-context beagle --speech-context "Planet of the Apes"`. The control: the 20 surnames research/12 used, none of which is in the clip: Kowalczyk, Nakamura, Oyelaran, Fitzgerald, Lindqvist, Moreau, Castellanos, Okonkwo, Haddad, Petrov, Svensson, Ramirez, Chatterjee, Delacroix, Abernathy, Yamamoto, Novak, Mbeki, Gallagher, Takahashi (from the research/12 run's command line; research/12 names only four of them).
- **The meeting.** The owner's Zoom meetings are the 28 recordings of the three meeting folders (the gate set's six Zoom sessions and the broad set; the webinars of the `talks` group are not meetings). Their speakers' names are read from the Zoom participant labels of each recording's reference, which are the names the speakers carry once they are named after the participants: the label's name part (before " - " in Zoom's "Name - Org" form), split into words, each normalised as the harness normalises words, kept if it has 3 letters or more and no digit. The eval sessions carry no names typed by the owner in Diarist, so this is the closest reading of "the speakers' names the owner gave". A reference occurrence of a name is the word itself or its possessive; it is wrong when metrics.py's WER alignment does not pair it with the same word. The meeting is the one where the cached v0.2.0 EN run (research/19's `rs_en_shipped`, shipped file, file setting) gets the most occurrences wrong (`names.py pick`). The phrase list is that meeting's names, one `--speech-context` per name, spelt as the label spells it, at the same boosts 0.5, 1 and 2.
- **Targets and fixed.** Targets are the reference occurrences of the boosted phrases that the unboosted run of the same file and setting gets wrong; a target is fixed when the boosted run gets it right. "At least half the target names are fixed" is read per recording that has targets: fixed ≥ targets / 2 on the clip and on the meeting.
- **Inserted.** An insertion is an occurrence of a boosted phrase in the boosted run's words that the alignment does not pair with a reference occurrence of it. "No boosted phrase is inserted [...] more than once per hour" is read per phrase and per recording: that phrase's insertions divided by the recording's hours at most 1. On the 5-minute clip one insertion is 12 per hour, and on the meeting one is more than one per hour, so the clause allows none. The unboosted run's own count is reported beside it (on a Zoom reference a correct name can count as inserted where Zoom's own transcript lacks it).
- **Control cost.** Clip WER of the control at boost b minus clip WER of the unboosted run, in points (speaker-agnostic, `levers.py boost`'s clip WER).
- **Verdict.** The rule passes at a boost b in {0.5, 1, 2} when, at the file setting, the control at b costs at most 0.2 points and the fixed and inserted clauses hold at b on both the clip and the meeting. The name source passes if the rule passes at some b.
- **GPU.** One batch through the shared queue (`gpu-run2.sh`, tier 2), untimed.

## Setup

| Item | Value |
|---|---|
| Machine, engine, diarizer | as in docs/eval/2026-10-30-phase-control.md: RTX 3080, driver 610.88, a byte copy of the app's nemo-speech 0.2.0 (`nemo-speech.exe` `d9527ade…3272`) and of its `Nemotron-3-Diarization.q8_0` (`08456d9e…a3a1`); one batch through the shared queue, 30 Oct 2026 23:54 to 31 Oct 00:01 IST |
| ASR | the re-converted EN file above, SHA-256 checked after the copy; no run printed "boosting disabled" or any other boosting message |
| Command (nemo_direct.py) | `nemo-speech transcribe <wav> --model <q8_0-pwf16.gguf> --stream --endpointing --diarize --diar-model <diarizer> --device cuda:0 --json --output <out> --force --asr.streaming.rnnt_right_context 13 --asr.diar.preset v3-offline [--speech-context <phrase> ...] [--speech-context-boost <b>]`; research/12's setting drops the two file-setting flags |
| Clip | `ami_050-350.wav`, 300.0 s, SHA-256 `e0f33f0d7bad572cba3507edaed2154ea897dc0c8f9e432e216ad85d959a9b84`; 595 reference words |
| Meeting | Aug 1 (47.4 min, 8,117 reference words), picked below; 5 names |

## The meeting

`names.py pick` over the 28 meetings, against the cached v0.2.0 EN runs (shipped file, file setting). The five with the most wrong occurrences of their speakers' names (counts only):

| Session | Zoom labels | Names | Reference occurrences | Wrong in the cached EN run |
|---|---|---|---|---|
| Aug 1 | 3 | 5 | 21 | 13 |
| Aug 2 | 6 | 10 | 22 | 11 |
| Sep 1 | 3 | 5 | 19 | 6 |
| Sep 2 | 3 | 4 | 13 | 6 |
| Aug 4 | 3 | 5 | 7 | 6 |

Over all 28 meetings: 262 reference occurrences of the speakers' names, 80 of them wrong. Aug 1 is the pick; the unboosted run of the re-converted file gets the same 13 of its 21 occurrences wrong, so the meeting has 13 targets.

## Results

### AMI clip, the file setting (the verdict)

Reference occurrences: Craig 2, beagle 3, Planet of the Apes 1. The unboosted run gets one Craig and Planet of the Apes wrong, so the clip has 2 targets.

| Run | Boost | Clip WER % | Words changed vs unboosted | Craig right | beagle right | Planet of the Apes right | Targets fixed | Insertions |
|---|---|---|---|---|---|---|---|---|
| unboosted | – | 15.63 | 0 | 1 | 3 | 0 | – | 0 |
| three targets | 0.5 | 15.63 | 0 | 1 | 3 | 0 | 0 of 2 | 0 |
| three targets | 1 | 15.63 | 0 | 1 | 3 | 0 | 0 of 2 | 0 |
| three targets | 2 | 15.13 | 8 | 2 | 3 | 1 | 2 of 2 | 1 (beagle) |
| 20 absent names | 0.5 | 15.13 | 5 | | | | | 0 |
| 20 absent names | 1 | 14.29 | 11 | | | | | 0 |
| 20 absent names | 2 | 14.45 | 13 | | | | | 0 |

At boost 2 the two targets are fixed (in normalised words, "i am greg" becomes "i am craig" and "planet 8 it is" becomes "planet of the apes"), and the boost also turns a correct "beagles" into "beagle" and drops the three words after it ("about beagles i could do we might" becomes "about beagle we might"): one boosted phrase where the reference lacks it, 12 per hour of the clip. Boosts 0.5 and 1 change no word. The control inserts none of its 20 names at any boost and the clip's WER falls by 0.50 to 1.34 points, from 5 to 13 changes elsewhere in the clip.

### AMI clip, research/12's setting (continuity only)

| Run | Boost | Clip WER % | Words changed | Craig / beagle / Planet right | Targets fixed | Insertions |
|---|---|---|---|---|---|---|
| unboosted | – | 15.29 | 0 | 1 / 3 / 0 | – | 0 |
| three targets | 0.5 | 15.29 | 0 | 1 / 3 / 0 | 0 of 2 | 0 |
| three targets | 1 | 15.46 | 3 | 1 / 3 / 0 | 0 of 2 | 1 (beagle) |
| three targets | 2 | 15.29 | 4 | 2 / 3 / 0 | 1 of 2 | 1 (beagle) |
| 20 absent names | 0.5 / 1 / 2 | 15.29 / 15.46 / 15.46 | 0 / 1 / 6 | | | 0 |

Research/12's pinned EN file at this setting: 15.1 %, Craig 1, beagle 3, Planet of the Apes 0; the re-converted 3.5 file there changed 19 words at boost 2 and 111 at boost 3, and the 20 absent names cost 22.7 points at boost 3. The EN file boosts far more gently at the plan's levels.

### The owner's meeting, the file setting

| Run | Boost | WER % | Words changed vs unboosted | Targets fixed | Right names broken | Insertions |
|---|---|---|---|---|---|---|
| unboosted | – | 3.91 | 0 | – (13 targets) | – | 0 |
| speakers' names | 0.5 | 3.97 | 5 | 0 of 13 | 0 | 0 |
| speakers' names | 1 | 3.97 | 7 | 0 of 13 | 0 | 0 |
| speakers' names | 2 | 3.95 | 9 | 0 of 13 | 0 | 0 |

Per name (P1 to P5, reference occurrences / right in every run): 2 / 0, 10 / 6, 1 / 1, 7 / 0, 1 / 1. No boost changes any name occurrence. WER against Zoom's transcript, which is itself a recogniser's (docs/05), so a name Zoom spelt differently counts as wrong here.

## Verdict

| Boost | Control cost on the clip (≤ 0.2) | Fixed, clip (≥ 1 of 2) | Fixed, meeting (≥ 7 of 13) | Insertions per phrase per hour, clip / meeting (≤ 1) | Rule at this boost |
|---|---|---|---|---|---|
| 0.5 | −0.50, holds | 0, fails | 0, fails | 0 / 0, holds | fails |
| 1 | −1.34, holds | 0, fails | 0, fails | 0 / 0, holds | fails |
| 2 | −1.18, holds | 2, holds | 0, fails | 12 (beagle) / 0, fails | fails |

**The kill rule fails at every boost the plan lists: no name source is built.** Plan 18 section 3.6's consequence applies: the KNOWN-ISSUES names row quotes these numbers, and the lever waits for an upstream change to RNNT boosting or NVIDIA's republished GGUFs. On this file boosting is no longer the wrecking ball research/12 measured on the 3.5 file (no absent name is inserted at boosts up to 2), but it does not fix the owner's names at all, and the one level that fixes the clip's targets also breaks a correct word next to them.

## GPU use

One batch, lock held 6.0 min (23:54:44 to 00:00:44 IST); the 18 `nemo-speech` runs sum to 4.5 min. Untimed, no quiet.lock.

## Reproduce

```sh
cd tools/eval
ffmpeg -ss 50 -t 300 -i ES2002a.Mix-Headset.wav -ac 1 -ar 16000 .work/clips/ami_050-350.wav
uv run python -I names.py pick --runs-dir <runs with rs_en_shipped> --config rs_en_shipped --write-phrases .work/spikes/names/meeting.phrases.txt
uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr <q8_0-pwf16.gguf> --out .work/spikes/names/clip_b2 -- \
    --asr.streaming.rnnt_right_context 13 --asr.diar.preset v3-offline \
    --speech-context Craig --speech-context beagle --speech-context "Planet of the Apes" --speech-context-boost 2
uv run python -I names.py score --clip --phrases Craig beagle "Planet of the Apes" --base .work/spikes/names/clip_base --runs .work/spikes/names/clip_b2
uv run python -I names.py score --session aug1 --phrases-file .work/spikes/names/meeting.phrases.txt --base .work/spikes/names/mtg_base --runs .work/spikes/names/mtg_b2
```
