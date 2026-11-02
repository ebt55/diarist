# 0.1.0-beta.6 release checklist

Status: prepared 2 Nov 2026 by chunk H of docs/plans/18, nothing published. The release is hand-built as in `docs/DEV.md` ("Desktop app (Tauri)": update endpoint, versioning, release PDB), like 0.1.0-beta.5 (docs/plans/17-release-beta5-checklist.md). Unlike beta.5 it needs no models release first: the punctuation entry's two new files (`LICENSE` and `NOTICE`) come from the model's own repository on Hugging Face, and its other files stay on `models-2026-10`, unchanged. It has no database change either. Steps marked **owner** publish something, push to `main`, or need the signing key; nothing happens there without the owner's go.

## 1. Decisions

Every decision is settled. Each row says what reversing it would take.

| Decision | Outcome | To reverse it |
|---|---|---|
| The recording indicator | Option A, "Dot" (the owner's pick): the taskbar overlay with the description "Recording", the title "● Recording · Diarist", redrawn tray frames, a solid in-app dot with a halo (plan 18 section 9; `apps/desktop/src-tauri/src/taskbar.rs`) | A code change (the merge of `claude/beta6-rec-indicator`) |
| Runtime 0.3.0 (chunk R) | Not adopted: EN missed R5 on `pub-meeting` (30.10 against 30.08), Nemotron 3.5 missed R3 on the broad set (84 gaps of 5 s or more against 83.6); every other bar passed (docs/eval/2026-10-31-runtime-030.md). `manifests/engine.json` and `TESTED_ENGINE_VERSIONS` stay on 0.2.0 | Beta.7's first chunk. R's adoption patch (not applied) is `tools/eval/.work/r-adoption-not-applied.patch` in the `beta6-r` worktree |
| The gap fill (chunk N1, N3) | Closed by plan 18 section 3.3's rule: pooled cpWER 10.85 → 10.87 against a required gain of 0.2, gaps of 5 s or more 23.1 percent fewer against 30 (docs/eval/2026-10-30-gapfill.md). N3 did not start | A new plan; the rule is not reopened by the public sets' gain |
| Spike L (word boosting for names) | Killed by its rule at every boost (0 of 13 names fixed on the owner's meeting; docs/eval/2026-10-30-names-spike.md). KNOWN-ISSUES has the names row | An upstream change to boosting, or NVIDIA's republished GGUFs |
| Chunk M2 (the owner's words over loud far-end speech) | Not in this release: it waits for the owner's labels. If they pass before the build, its branch `claude/beta6-echo-m2` is merged as a follow-up and this checklist gains its rows (the KNOWN-ISSUES echo row, the CHANGELOG, `echo_sample` and `aec_postpass`) | Merging it later, in beta.7 |
| The punctuation pin (chunk Q) | v1.1.1 (`0cea764`): the model and tokenizer are the bytes of `ee31fdf`, so no installed copy downloads them again; the authors' `LICENSE` and `NOTICE` replace the hosted `LICENSE-Apache-2.0.txt` in the file list (docs/eval/2026-10-30-punctuation-pin.md) | `manifests/models.json`, the entry `punctuation-horizon-small`, back to beta.5's (git history) |
| Download sizes in decimal units | The setup screen and Settings show MB and GB of 10^6 and 10^9 bytes, as the docs do; the docs' per-item figures were restated (engine 102 MB, diarizer 107 MB, EN model 700 MB, Parakeet 714 MB, punctuation 320 MB; "Install recommended models" 1.13 GB) | `sizeMb` in `apps/desktop/src/lib/downloads.svelte.ts` and the docs' figures |

## 2. Before building

- [ ] Every beta.6 branch is committed on `claude/beta6-integration` (draft PR "Diarist 0.1.0-beta.6"); the PR is marked ready and merged into `main` (**owner**), and the build runs from that commit with a clean tree.
- [ ] Version `0.1.0-beta.6` in the workspace `Cargo.toml`, `Cargo.lock`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/package.json`, README.md (download name and status line), docs/INSTALL.md, docs/DEV.md and docs/SETUP-WITH-AN-AGENT.md. `cargo run --release -p diarist-cli -- --version` prints `diarist-cli 0.1.0-beta.6`.
- [ ] CHANGELOG: the `[0.1.0-beta.6]` heading carries the publishing date (written as 2026-11-03; change it to the day the release is published).
- [ ] `RELEASED_SCHEMA` is 5 (`crates/core/src/store/sqlite/mod.rs`) and no beta.6 branch added a migration (checked at integration, 2 Nov 2026), so beta.6 opens a beta.5 database as it is and makes no new `diarist.db.bak-<n>`.
- [ ] Checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; in `apps/desktop`, `bun run check`, `bun run test`, `bun run build`; `tools/eval`'s `test_metrics.py` and `test_gapfill.py`.
- [ ] Eval: `docs/eval/2026-11-03-beta6-release.md` is a fresh full-set run of the rebuilt `diarist-cli` (36 sessions, both models, the app's defaults), with every bar of the beta.5 note's "Release bars" judged, and `docs/eval/benchmarks.json` follows it. Its speed section is the timed gate-set block under quiet.lock with `nvidia-smi pmon -s u -d 1` (validity per run as in the beta.5 note and the 31 Oct re-time).
- [ ] The GPU suites on the merged tip, each through the shared GPU queue: `acceptance_gpu`, `live_ws_gpu`, `live_carry_repro` (lookahead 6), `echo_sample`, `echo_fleurs`, `aec_postpass` and the Process again exact test. Results in the release note.
- [ ] Runtime watch: 0.3.0 is tagged and measured, not adopted (section 1). Before beta.7 adopts it, check NeMo-Speech.cpp issues #67 and #81 and any tag after v0.3.0.
- [ ] docs/KNOWN-ISSUES.md: the status line names 0.1.0-beta.6; the runtime 0.3.0 row and the names row are in; the updater row gets the result of section 6.
- [ ] Privacy scan of every changed file before each push (section 9).

## 3. Owner tests on a build of the final tip

Before the installer is published, on a release build of the commit that will ship. A test build of the integration branch for the recording checks is in `C:\Users\ebin\claude-ground\diarist-releases\0.1.0-beta.6-test\` (its `README.txt` has the steps; it runs against a scratch data root and never touches the installed app's folder). Quit the installed Diarist first: one instance runs at a time.

### 3.1 The recording indicator (chunk V's manual checks)

From chunk V's hand-over. Each check needs a short recording.

1. **Taskbar at 200 %, dark taskbar, auto-hide on.** Start a recording, then reveal the taskbar. The Diarist button carries a red dot in a white ring at its lower right, sharp and not blurry. The hover thumbnail and Alt+Tab show "● Recording · Diarist".
2. **Stop.** After the post-pass starts, the dot leaves the button and the title returns to "Diarist".
3. **Failed start.** Start with something missing (for example a model folder renamed, in a scratch data root only). The title stays "Diarist" and no dot appears.
4. **Minimise and restore** while recording. The dot stays on the button.
5. **Tray round trip.** Close the window while recording (it hides to the tray, and the button goes away). Reopen it from the tray icon or "Open Diarist". The dot comes back on the button.
6. **Tray icon.** While recording, the notification-area icon shows the red dot cut into its corner, sharp at 200 %. Hover shows "Diarist: recording since HH:MM". The menu still reads Stop recording.
7. **Narrator.** With Narrator on, focus the taskbar button (Win+T, arrows). It reads the window title, and Windows may add "Recording", the overlay's description.
8. **In-app.** The dot in the sidebar Stop button and in the live strip stays solid; a thin halo grows and fades around it every 2 s; the dot never fades to pink. With "Show animations" off in Windows (reduced motion), there is no halo.
9. **Settings only you can change:** a light taskbar (the dot reads on the ink icon); "Show badges on taskbar apps" off (the dot is expected to vanish, and the title still says it); "Show smaller taskbar buttons"; 100 % and 150 % scaling (the tray dot is sharp at 16 and 24 px).
10. **Explorer restart** while recording (Task Manager, Windows Explorer, Restart). The dot is back on the button once the taskbar returns.

- [ ] Checks 1 to 10 pass, or each failure is written down with the scale and taskbar settings it was seen at.

### 3.2 An import and a short recording on the final build

- [ ] Import a short English file (Ctrl+O). Open it: the text is punctuated, and the line reads "Punctuation: <model> (N marks, M capitals)."
- [ ] Record about a minute with a video playing, then stop. Live words and speakers show; after Stop the final transcript, its punctuation line and the playback work; the app log has no new errors.

## 4. Build (**owner**, the reference machine)

From `apps/desktop`, in PowerShell:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\diarist.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
bun run tauri build
```

- [ ] Outputs: `target\release\bundle\nsis\Diarist_0.1.0-beta.6_x64-setup.exe` and its `.sig` (the updater signature), `target\release\diarist.exe`, `target\release\diarist.pdb`.
- [ ] SHA-256 and size of the installer: `Get-FileHash -Algorithm SHA256 <installer>` and `(Get-Item <installer>).Length`.
- [ ] Smoke test the built exe without installing it, so this machine's installed 0.1.0-beta.5 stays in place for section 6: close the installed app, then start `target\release\diarist.exe` with `DIARIST_DATA_DIR` set to an empty scratch folder. It shows the setup screen with "Install engine (102 MB)" and "Install recommended models (1.13 GB: diarizer, the EN model and Horizon-Labs punctuation small)", the punctuation model as one line whose "Files and addresses" lists its eight files with their addresses and hashes (`LICENSE` and `NOTICE` from huggingface.co), and "0.1.0-beta.6" in Settings, About.

## 5. Archive the PDB, then draft the release (**owner**)

- [ ] Zip `target\release\diarist.exe` and `target\release\diarist.pdb` together as `diarist-0.1.0-beta.6-symbols.zip`, check that it resolves (`cdb -z diarist.exe -y . -c ".lines -e; ln diarist+0x1000; q"`, docs/DEV.md "Release PDB"), and keep it locally next to the signing key's backup. It is not attached to the release.
- [ ] Release notes: the CHANGELOG section, the line "The installer is not code-signed yet; SmartScreen asks once.", the installer's SHA-256 and size, and a line that the database is unchanged from 0.1.0-beta.5 (no upgrade on first start, and 0.1.0-beta.5 can open it again).
- [ ] `gh release create v0.1.0-beta.6 --draft --prerelease --target main --title "Diarist v0.1.0-beta.6" --notes-file <notes.md> <installer> <installer>.sig`.
- [ ] The draft has the installer and the `.sig` with the local sizes.

## 6. Publish, then the manifest: the update from beta.5 (**owner**)

The installed 0.1.0-beta.5 reads `updater/latest.json` on `main`, and draft assets are not public, so the manifest changes only after the release is public. Check for updates is manual, so the time between the manifest commit and the check below is the only exposure.

- [ ] Before: this machine's installed copy (`%LOCALAPPDATA%\Diarist\diarist.exe`) is 0.1.0-beta.5, and its data folder has a byte copy elsewhere. Its database is at schema 5, and the folder holds `diarist.db.bak-3` (from the beta.4 to beta.5 update).
- [ ] Publish the draft (it stays a pre-release). Publishing pushes the tag; the release workflow's guard job finds the installer on the release and builds nothing. If it fails with "Could not list releases", re-run it.
- [ ] The asset answers publicly: `curl.exe -sIL https://github.com/ebt55/diarist/releases/download/v0.1.0-beta.6/Diarist_0.1.0-beta.6_x64-setup.exe` ends in `200` with the local size as `content-length`.
- [ ] Write `updater/latest.json` from the `.sig` (from the repository root; node writes UTF-8 without a byte-order mark, which the updater's JSON parser needs):

  ```sh
  node -e "const fs=require('fs');const v='0.1.0-beta.6';const sig=fs.readFileSync('target/release/bundle/nsis/Diarist_'+v+'_x64-setup.exe.sig','utf8').trim();fs.writeFileSync('updater/latest.json',JSON.stringify({version:v,notes:'Diarist '+v+': see CHANGELOG.md',pub_date:new Date().toISOString().replace(/\.\d{3}Z$/,'Z'),platforms:{'windows-x86_64':{signature:sig,url:'https://github.com/ebt55/diarist/releases/download/v'+v+'/Diarist_'+v+'_x64-setup.exe'}}},null,2)+'\n')"
  ```

- [ ] Commit `updater/latest.json` on `main` and push (after the privacy scan).
- [ ] `curl.exe -s https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json` shows 0.1.0-beta.6 (that address caches for up to five minutes).
- [ ] At once, in the installed 0.1.0-beta.5: Settings, About, Check for updates offers 0.1.0-beta.6; Install and restart. Expected: download, signature check, the installer runs, the app restarts as 0.1.0-beta.6 with the same meetings, and Diagnostics shows no new failures. No migration runs: `diarist.db.bak-3` stays and no new `.bak` appears. Settings, Models shows the punctuation row as "2 of 8 files missing: LICENSE, NOTICE (13 kB)" while punctuation keeps working; "Download recommended" fetches those two files and nothing else (docs/KNOWN-ISSUES.md, the upgrader row).
- [ ] Going back to 0.1.0-beta.5, if it is ever needed: install its installer again; it opens the same database (schema 5).
- [ ] If any of it fails: revert the manifest commit on `main` and push at once, so no other installed copy is offered the update, then keep the app log (`logs\diarist.log` in the data folder) for the cause. The published release stays: its installer still works when downloaded by hand.
- [ ] Record the outcome in the KNOWN-ISSUES updater row.

## 7. The punctuation snapshot (**owner**, does not block the release)

Plan 18 section 8.1 item 5: chunk Q prepared the dated snapshot of the model at v1.1.1 in `C:\Users\ebin\claude-ground\diarist-releases\models-2026-10-v1.1.1-snapshot\` (`punctuation-model-card-0cea764.md`, `punctuation-model-info-2026-10-30.json` and a `CONTENTS.md` with their sizes, hashes and sources). Where it goes is the owner's choice: two extra assets on `models-2026-10` (its manifest-referenced files are never changed or removed, and beta.5's manifest still fetches `LICENSE-Apache-2.0.txt` from it), or the first release of a separate models repository (plan 18 section 14). If they go on `models-2026-10`, `CONTENTS.md` in `C:\Users\ebin\claude-ground\diarist-releases\models-2026-10\` gains the two rows.

## 8. After, and for beta.7

- [ ] The installer downloaded from the release page has the SHA-256 in the notes.
- [ ] The site's numbers come from `docs/eval/benchmarks.json`.
- [ ] Beta.7: runtime 0.3.0 first (docs/eval/2026-10-31-runtime-030.md; R's prepared adoption patch and the re-recorded WebSocket fixture are in the `beta6-r` worktree's `tools/eval/.work`). Check NeMo-Speech.cpp issues #67 and #81 first.
- [ ] Beta.7, with 0.3.0: `models::setup`'s `EXTRACTION_FACTOR` of 1.4 (the extracted runtime against its zip, from 0.2.0's CUDA package: 102 MB zipped, 141 MB extracted) underestimates 0.3.0's Vulkan and CPU packages, which grow from 22.4 to 77.1 MB (3.4 times) and from 8.7 to 33.1 MB (3.8 times) when extracted (CUDA 508.6 to 683.7 MB, 1.34 times). The setup screen's disk check then asks for too little for those two packages; a per-package figure or a larger factor fixes it.
- [ ] Beta.7: chunk M2, if the owner's labels pass and it was not merged into beta.6 (section 1).
- [ ] Beta.7: the first-run screen, opened from Settings by a 0.1.0-beta.5 upgrader, quotes the full recommended size, and `disk_needed_bytes` counts the punctuation model's full 372 MB, although a pull fetches only the 13 kB that are missing.
- [ ] Beta.7: a pull re-hashes every file already present in full, and its progress total is the model's full 320 MB rather than the bytes it fetches.
- [ ] Beta.7: Process again without a kept live transcript (plan 18 section 10.2).
- [ ] The Tauri bump of plan 18 section 10.1 item 2 (Dependabot alert #1, `glib` 0.18.5, a Linux-only GTK dependency) is not in beta.6.

## 9. Privacy scan before every push

The owner's Zoom recordings, transcripts, meeting titles and participant names never go into the repository.

- [ ] Before each push (the integration branch, `main`, `updater/latest.json`): run the scan of every file changed since the last pushed commit against the private needle lists (`meta.txt` and `ngrams.txt` in the scratch privscan folder; matched case-insensitively, counts and line numbers only). The scan prints needle numbers, never the text. The known false positives are needles 119 and 129 inside common English words, needle 92 in docs/05, and the owner's first name inside the word "webinars" in the eval notes that describe the talks. Any other hit stops the push.
- [ ] Never commit `tools/eval/sessions.toml`, `tools/eval/.work`, run folders or audio.
