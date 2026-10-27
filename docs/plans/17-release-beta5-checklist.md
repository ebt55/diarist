# 0.1.0-beta.5 release checklist

Status: prepared 27 Oct 2026 by chunk H of docs/plans/16, nothing published. The release is hand-built as in `docs/DEV.md` ("Desktop app (Tauri)": update endpoint, versioning, release PDB), like 0.1.0-beta.2 (docs/plans/13-release-beta2-checklist.md). It has one step beta.2 did not: the punctuation model is a required download, and five of its seven files come from this project's `models-2026-10` release, so that release goes public first (section 2). Steps marked **owner** publish something, push to `main`, or need the signing key; nothing happens there without the owner's go.

## 1. Decisions

Every owner decision is settled. Each row says what reversing it would take; none needs a code change.

| Decision | Outcome | To reverse it |
|---|---|---|
| Candidate A's provenance: Apache-2.0 only in the card's YAML, an organisation a few weeks old, partly LLM-written training text | Ship A with the mitigations of docs/plans/16 section 12.9: LICENSES.md rows, a dated snapshot of the card and the file list on the models release, the web.archive.org copies (done, section 9), a Hugging Face discussion asking for a LICENSE file (pending, section 9), the swap plan (section 8) | Section 8 (b): `"required": false` in the `punctuation-horizon-small` entry of `manifests/models.json`, or remove the entry |
| `required: true`, about 0.3 GB (306 MB) fetched on first run | Kept: the first-run screen fetches it with the other models ("Download it automatically is fine") | `"required": false` in that entry; README's quick start and table (1.0 GB, about 1.2 GB default), docs/INSTALL.md's first-start sizes, docs/01's install row and the CHANGELOG entry then say 770 MB and about 870 MB again; installed copies still get the one-time "New: punctuation" offer |
| French and Italian commas kept although their FLEURS slices measured just under the 0.75 bar (0.74 and 0.69, plan 16 section 3.13.6) | Kept as shipped | Add `"fr": ["period", "question"]` and `"it": ["period", "question"]` to the entry's `punctuation.marks_by_language`; docs/04 section 8's languages paragraph and the KNOWN-ISSUES comma row then say commas are left out for French and Italian too |
| Chunk M (the post-canceller correlation layer) | Not in beta.5: no variant met its ship rule (docs/eval/2026-10-26-echo-owner-level.md section 8); the KNOWN-ISSUES echo row and the CHANGELOG say what remains | Beta.6, section 10 |

## 2. The `models-2026-10` pre-release first (**owner**)

The punctuation entry is required, and `manifests/models.json` points its five non-Hugging-Face files at `https://github.com/ebt55/diarist/releases/download/models-2026-10/<file>`. Until that release is public those downloads end in 404, so a fresh beta.5 install would show the punctuation model as failed. Publish it before the app release, and before the app release's manifest commit at the latest.

- [ ] The assets are in `C:\Users\ebin\claude-ground\diarist-releases\models-2026-10\`. `CONTENTS.md` there lists each one with its bytes, SHA-256 and source. Upload every file except `CONTENTS.md`:
  - `diarist-onnxruntime.dll` (17,253,176 bytes), `onnxruntime-LICENSE.txt`, `onnxruntime-ThirdPartyNotices.txt`, `LICENSE-Apache-2.0.txt`, `NOTICE-punctuation.txt`: the five the manifest fetches;
  - `punctuation-model-card-ee31fdf.md` and `punctuation-model-info-2026-10-27.json`: the dated snapshot of plan 16 section 12.9 (iii) 2, which the manifest does not reference.
- [ ] `sha256sum` of each file matches CONTENTS.md before the upload.
- [ ] Create it as a pre-release, separate from app releases, for example `gh release create models-2026-10 --prerelease --target main --title "Models 2026-10" --notes-file <notes.md> <the seven files>`, with notes taken from CONTENTS.md (what each file is, its SHA-256, and that the app downloads the five by itself).
- [ ] Each of the five manifest assets answers publicly: `curl.exe -sIL https://github.com/ebt55/diarist/releases/download/models-2026-10/<file>` ends in `200` with the bytes of CONTENTS.md as `content-length`.
- [ ] A fresh scratch data root fetches the model from the real hosts: `diarist-cli --data-dir <empty folder> models pull punctuation-horizon-small` (the entry's name since integration; CONTENTS.md still calls it "Punctuation (Horizon-Labs small)"), then `models scan` lists `onnx\model_quantized.onnx` as `punctuation` with `manifest_name` `punctuation-horizon-small`.

## 3. Before building

- [ ] Every chunk in docs/plans/16 is committed on `claude/beta5-integration` (draft PR ebt55/diarist#12); the PR is marked ready and merged into `main` (**owner**), and the build runs from that commit with a clean tree.
- [ ] Version `0.1.0-beta.5` in the workspace `Cargo.toml`, `Cargo.lock`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/package.json`, README.md (download name and status line), docs/INSTALL.md, docs/DEV.md and docs/SETUP-WITH-AN-AGENT.md. `cargo run --release -p diarist-cli -- --version` prints `diarist-cli 0.1.0-beta.5`.
- [ ] CHANGELOG: the `[0.1.0-beta.5]` heading carries the publishing date (written as 2026-10-27; change it if the release slips).
- [ ] `RELEASED_SCHEMA` is 5 (`crates/core/src/store/sqlite/mod.rs`), so builds after beta.5 keep `diarist.db.bak-5`.
- [ ] Checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; in `apps/desktop`, `bun run check`, `bun run test`, `bun run build`.
- [ ] Eval: `docs/eval/2026-10-27-beta5-release.md` is a fresh full-set run of the rebuilt `diarist-cli` (36 sessions, both models), with every bar judged, and `docs/eval/benchmarks.json` follows it. The speed bar, timed on 27 Oct 2026: EN +5.0 percent (met), Nemotron 3.5 -5.4 percent (missed by 0.4 points as measured; its three timed-block runs overlapped the Claude app's GPU use, and the same build did those sessions within -1.5 to +1.3 percent of beta.4 earlier that day). The owner decides whether to re-time those three runs with the Claude app idle (about 3 minutes of GPU under quiet.lock) before shipping.
- [ ] Runtime watch (plan 16 section 4.6): on 27 Oct 2026 NeMo-Speech.cpp has no tag after v0.2.0 (only the moving `nightly`), so `manifests/engine.json` and `TESTED_ENGINE_VERSIONS` stay. If a tag appears before the build, it waits for beta.6 unless research/08's fixtures, the full set and the live latency all hold (and check issues #67 and #81 first).
- [ ] docs/KNOWN-ISSUES.md: the status line names 0.1.0-beta.5; the updater row gets the result of section 7.
- [ ] Privacy scan of every changed file before each push (section 11).

## 4. Owner tests on a build of the final tip

Before the installer is published, on a release build of the commit that will ship.

### 4.1 Punctuation in the app (the owner's eight steps)

Verbatim from chunk E's hand-over:

> Use a build of this branch plus a punctuation model: D's entry, or a drop-in folder with `<file>.meet.json` and `diarist-onnxruntime.dll`. Use the keyboard throughout.
> 1. Ctrl+, then go to Models, Punctuation. The model shows as selected, with "CPU". "New transcripts: Add automatically" is chosen. If a red "Punctuation runtime" row shows, use its Copy link, install the redistributable, and restart Diarist.
> 2. Import a short English file (Ctrl+O). Open it. The line reads "Punctuation: <model> (N marks, M capitals)." and the text has full stops, commas and capitals.
> 3. Tab to Remove punctuation and press Enter. The job list shows "Removing punctuation: <title>", then a toast. The text is back to the speech model's own. Then Add punctuation brings it back. Speaker names and colours stay the same both times.
> 4. Record about a minute with a video playing, then stop. After the final transcript arrives, the punctuation line shows. If an echo line appears, Restore still works and the text keeps its punctuation.
> 5. Settings: press "Add punctuation to N older meetings" (your beta.4 meetings). The confirmation names N and says the earlier versions stay. Press Enter. The job list counts "N of M meetings" and a report appears at the end. Open an older meeting: it is punctuated.
> 6. Set New transcripts to Off and import again. The line reads "No punctuation: it was off in Settings…". Set it back to Add automatically.
> 7. Only once D's entry exists: in a beta.4 data root without a punctuation model, the notice shows once. Later dismisses it, and it stays gone after a restart.
> 8. Diagnostics: the "Punctuation runtime" row passes.

(D's entry exists now, so step 7 applies. The hand-over file stored the ellipsis of step 6 as a Windows-1252 byte; it is written here as "…".)

- [ ] Steps 1 to 8 pass.

### 4.2 A live recording from a long data folder

KNOWN-ISSUES' long-path row still has "a live recording from a deep data folder, and the app itself started from one" to try by hand.

- [ ] Close the installed app. Start the built `diarist.exe` with `DIARIST_DATA_DIR` set to a scratch folder whose path is 264 characters long (the chunk C repro). Install the engine and the models there, or copy them in, and press Refresh.
- [ ] Record about 20 s with a video playing, then stop. Live words and speakers show; after Stop the final transcript, its punctuation line and the playback work; the app log has no path errors.
- [ ] Record the result in that KNOWN-ISSUES row, and delete the scratch folder.

### 4.3 Process again on a real meeting

- [ ] Pick one of your meetings whose live transcript was kept (its details offer "Process again"). Before: `Get-FileHash -Algorithm SHA256` of its `audio\<uid>\playback.flac`.
- [ ] Process again. The meeting shows as finalizing, then the new transcript is current, with the punctuation line and, if the microphone heard the speakers, the echo line; the run list still has the earlier transcript and the live one.
- [ ] After: `playback.flac` has the same SHA-256, and no `mic.wav`, `loopback.wav`, `.part` file or `rerun-from-flac.json` is left beside it.

## 5. Build (**owner**, the reference machine)

From `apps/desktop`, in PowerShell:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\diarist.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
bun run tauri build
```

- [ ] Outputs: `target\release\bundle\nsis\Diarist_0.1.0-beta.5_x64-setup.exe` and its `.sig` (the updater signature), `target\release\diarist.exe`, `target\release\diarist.pdb`.
- [ ] SHA-256 and size of the installer: `Get-FileHash -Algorithm SHA256 <installer>` and `(Get-Item <installer>).Length`.
- [ ] Smoke test the built exe without installing it, so this machine's installed 0.1.0-beta.4 stays in place for section 7: close the installed app (one instance at a time), then start `target\release\diarist.exe` with `DIARIST_DATA_DIR` set to an empty scratch folder. It shows the setup screen with "Install recommended models (1.0 GB: diarizer, the EN model and Horizon-Labs punctuation small)", each punctuation file listed with its address and hash, and "0.1.0-beta.5" in Settings, About.

## 6. Archive the PDB, then draft the release (**owner**)

- [ ] Zip `target\release\diarist.exe` and `target\release\diarist.pdb` together as `diarist-0.1.0-beta.5-symbols.zip`, check that it resolves (`cdb -z diarist.exe -y . -c ".lines -e; ln diarist+0x1000; q"`, docs/DEV.md "Release PDB"), and keep it locally next to the signing key's backup. It is not attached to the release.
- [ ] Release notes: the CHANGELOG section, the line "The installer is not code-signed yet; SmartScreen asks once.", the installer's SHA-256 and size, and a line that the first start upgrades the database and keeps `diarist.db.bak-3`.
- [ ] `gh release create v0.1.0-beta.5 --draft --prerelease --target main --title "Diarist v0.1.0-beta.5" --notes-file <notes.md> <installer> <installer>.sig`.
- [ ] The draft has the installer and the `.sig` with the local sizes.

## 7. Publish, then the manifest: the update from beta.4 (**owner**)

The installed 0.1.0-beta.4 reads `updater/latest.json` on `main`, and draft assets are not public, so the manifest changes only after the release is public. Check for updates is manual, so the time between the manifest commit and the check below is the only exposure.

- [ ] Before: section 2 is done (the models release is public). This machine's installed copy (`%LOCALAPPDATA%\Diarist\diarist.exe`) is 0.1.0-beta.4, and its data folder has a byte copy elsewhere. Its database is at schema 3, and the folder holds `diarist.db.bak-2` (checked 27 Oct 2026).
- [ ] Publish the draft (it stays a pre-release). Publishing pushes the tag; the release workflow's guard job finds the installer on the release and builds nothing. If it fails with "Could not list releases", re-run it.
- [ ] The asset answers publicly: `curl.exe -sIL https://github.com/ebt55/diarist/releases/download/v0.1.0-beta.5/Diarist_0.1.0-beta.5_x64-setup.exe` ends in `200` with the local size as `content-length`.
- [ ] Write `updater/latest.json` from the `.sig` (from the repository root; node writes UTF-8 without a byte-order mark, which the updater's JSON parser needs):

  ```sh
  node -e "const fs=require('fs');const v='0.1.0-beta.5';const sig=fs.readFileSync('target/release/bundle/nsis/Diarist_'+v+'_x64-setup.exe.sig','utf8').trim();fs.writeFileSync('updater/latest.json',JSON.stringify({version:v,notes:'Diarist '+v+': see CHANGELOG.md',pub_date:new Date().toISOString().replace(/\.\d{3}Z$/,'Z'),platforms:{'windows-x86_64':{signature:sig,url:'https://github.com/ebt55/diarist/releases/download/v'+v+'/Diarist_'+v+'_x64-setup.exe'}}},null,2)+'\n')"
  ```

- [ ] Commit `updater/latest.json` on `main` and push (after the privacy scan).
- [ ] `curl.exe -s https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json` shows 0.1.0-beta.5 (that address caches for up to five minutes).
- [ ] At once, in the installed 0.1.0-beta.4: Settings, About, Check for updates offers 0.1.0-beta.5; Install and restart. Expected: download, signature check, the installer runs, the app restarts as 0.1.0-beta.5 with the same meetings, and Diagnostics shows no new failures. The first start runs migrations 0004 and 0005 in one open: `diarist.db.bak-3` (the schema-3 database) is next to `diarist.db`, and `diarist.db.bak-2` is gone. The "New: punctuation" notice shows once; Download fetches the seven files.
- [ ] Going back to 0.1.0-beta.4, if it is ever needed: beta.4 refuses the upgraded database, so close Diarist, move `diarist.db` together with its `diarist.db-wal` and `diarist.db-shm` aside (they belong to it), then copy `diarist.db.bak-3` to `diarist.db`. Transcripts made with beta.5 are only in the copy moved aside.
- [ ] If any of it fails: revert the manifest commit on `main` and push at once, so no other installed copy is offered the update, then keep the app log (`logs\diarist.log` in the data folder) for the cause. The published release stays: its installer still works when downloaded by hand.
- [ ] Record the outcome in the KNOWN-ISSUES updater row.

## 8. If the punctuation model's repository goes away or its licence is disputed

From docs/plans/16 section 12.9 (iii) 4. Neither case touches Diarist's own MIT licence or its code.

- **(a) The repository is deleted or gated.** First-run downloads of the punctuation model fail; installed copies are unaffected. Diarist hosts the two upstream files itself, unchanged, under Apache-2.0 section 4: upload `onnx/model_quantized.onnx` and `tokenizer.json` to a models release of `ebt55/diarist` (for example `models-2026-10`, as `model_quantized.onnx` and `tokenizer.json`), and change their two `url` fields in `manifests/models.json`. The URLs change and the hashes do not. docs/INSTALL.md's firewall row for models then drops `huggingface.co` for this model. A private copy is kept at `C:\Users\ebin\claude-ground\diarist-releases\punctuation-model-backup-ee31fdf\` (`onnx\model_quantized.onnx` and `tokenizer.json` with the licence and notice files); on 27 Oct 2026 every file's SHA-256 matched the manifest.
- **(b) A rights claim.** Set `"required": false` on the entry, or remove it: a manifest change only. Installed users keep their copy under the irrevocable grant they already received.

**Upgrade rule (plan 16 section 3.12 rule 8).** Before pinning any new revision of this model, or any new punctuation model, re-read the card's licence line and the repository's file list at that revision, record both with the date in the eval note or the release's CONTENTS.md, and take a dated snapshot as in section 2.

## 9. Owner actions on the provenance (neither blocks the release)

- [x] Saved on web.archive.org (27 Oct 2026, checked):
  - the model page, showing `apache-2.0`: <https://web.archive.org/web/20261027103142/https://huggingface.co/Horizon-Labs/punctuation-restoration-small>;
  - the tree at `ee31fdf` (folders `code`, `eval`, `onnx` and `training`, plus `README.md` and `.gitattributes`; no LICENSE file): <https://web.archive.org/web/20261027103231/https://huggingface.co/Horizon-Labs/punctuation-restoration-small/tree/ee31fdfc5304f876ea3b0dd72d30eda7e940b2b1>.
- [ ] Pending: post a discussion on the model's Hugging Face page asking the authors for a LICENSE file. Draft:

  > Thanks for the model. Diarist (github.com/ebt55/diarist, MIT) downloads `onnx/model_quantized.onnx` and `tokenizer.json` at ee31fdf as its punctuation add-on. Could you add a LICENSE file with the Apache License 2.0 text, and a NOTICE naming mmBERT-small (MIT) as the base model, so the card's `license: apache-2.0` is backed by a file in the repository? If you tag releases, a v1.1 tag would help too.

## 10. After, and for beta.6

- [ ] The installer downloaded from the release page has the SHA-256 in the notes.
- [ ] The site's numbers come from `docs/eval/benchmarks.json`.
- [ ] Beta.6: chunk M, the post-canceller correlation layer; lead: spare words near a known owner level (docs/eval/2026-10-26-echo-owner-level.md section 8).
- [ ] Beta.6: Process again without a kept live transcript (names from the current transcript, the raw microphone transcribed for the echo step; KNOWN-ISSUES' Process again row).
- [ ] Beta.6: a NeMo-Speech.cpp release after v0.2.0 (the fix of #67; check #81 before upgrading), behind the full eval set.
- [ ] LICENSES.md's crate counts are from 15 Oct 2026 and predate `ort` and `tokenizers`; regenerate them with the `cargo metadata` command there.

## 11. Privacy scan before every push

The owner's Zoom recordings, transcripts, meeting titles and participant names never go into the repository.

- [ ] Before each push (the integration branch, `main`, `updater/latest.json`): run the scan of every file changed since the last pushed commit against the private needle lists (`meta.txt` and `ngrams.txt` in the scratch privscan folder; matched case-insensitively, counts and line numbers only). The scan prints needle numbers, never the text; the known false positives on `main` are needles 119 and 129 inside common English words and needle 92 in docs/05. Any other hit stops the push.
- [ ] Never commit `tools/eval/sessions.toml`, `tools/eval/.work`, run folders or audio.
