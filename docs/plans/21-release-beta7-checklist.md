# 0.1.0-beta.7 release checklist

Status: prepared 18 Nov 2026 on `claude/beta7-integration` (draft PR "Diarist 0.1.0-beta.7"). Not built, not published. The release is hand-built as in `docs/DEV.md` ("Desktop app (Tauri)": update endpoint, versioning, release PDB), like 0.1.0-beta.6 (docs/plans/19-release-beta6-checklist.md). It needs no models release and has no database change: `RELEASED_SCHEMA` stays 5, and the retention bookkeeping is one JSON value in the settings table (`retention.state`). Steps marked **owner** publish something, push to `main`, or need the signing key; nothing happens there without the owner's go.

## 1. What is in it

Three branches, merged `--no-ff` in this order:

| Branch | What it brings |
|---|---|
| `claude/beta7-housekeeping` | `tauri-plugin-updater` 2.14.1 (zip 9 instead of zip 4.6.1, two advisories); the first-run screen quotes only the missing bytes to upgraders; the New recording sheet's line on consent and headphones; `.github/workflows/ci.yml` (pull-request checks on `windows-latest`); the env-gated tests marked `#[ignore]` with their reason |
| `claude/beta7-list-speaker` | Issue 19, the list's row menu (right-click, the ContextMenu key or Shift+F10: open, pin or unpin, delete); issue 20, the sort (recorded or added, newest or oldest first, `library.sort`); issue 21, L and H jump to the same speaker's next or previous line, and a click on a speaker chip opens its menu; keys stay in an open menu |
| `claude/beta7-retention` | Issue 22: "Keep audio for" defaults to 0 (keep forever) for anyone who never set it; the first-deletion hold, the library heads-up 3 days ahead, held overdue audio; recordings' size in Settings and the one-time prompt past 10 GB |

Integration work on top: `api.ts` lost the stale `retentionPreview` and `applyRetention`; the pin texts say "never deleted automatically"; the docs describe the new retention (docs/01 "Recording storage", KNOWN-ISSUES, INSTALL, PRIVACY, README, docs/02, docs/04, DEV, SETUP-WITH-AN-AGENT); the flaky live test waits on its condition instead of a 1 s sleep.

Words identical by construction: `git diff --stat origin/main...HEAD -- crates/core/src/engine crates/core/src/pipeline crates/core/src/transcript crates/core/src/punct crates/core/src/audio crates/core/src/live` shows only `engine/process.rs` (one `#[ignore]` line) and `live/tests.rs`. The `Cargo.lock` change is the updater's subtree only (zip 4.6.1, `arbitrary` and `derive_arbitrary` gone, base64 0.23.1 for the updater).

## 2. Known at release

- **Runtime 0.3.0 is not in it.** The beta.6 CHANGELOG said 0.3.0 was next, for beta.7; this release changed no speech-pipeline code and stays on 0.2.0. Its two KNOWN-ISSUES rows now say "a later release". Before any release adopts it, check NeMo-Speech.cpp issues #67 and #81 and any tag after v0.3.0 (docs/eval/2026-10-31-runtime-030.md; R's adoption patch is in the `beta6-r` worktree's `tools/eval/.work`).
- **No GPU run and no eval for this release.** File transcripts are the same as 0.1.0-beta.6's because no code on the speech path changed, not because they were measured. None of the GPU suites (`acceptance_gpu`, `live_ws_gpu`, `live_carry_repro`, `echo_sample`, `echo_fleurs`, `aec_postpass`, Process again exact) ran on this tip. Since housekeeping they are `#[ignore]`d, so `cargo test --workspace` reports them as ignored, and the new CI has no GPU.
- **Carried over from plan 19 section 8, not in beta.7:** `EXTRACTION_FACTOR` (1.4) underestimates 0.3.0's Vulkan and CPU packages (matters only with 0.3.0); a pull re-hashes every file already present, and its progress total is the model's full 320 MB rather than the bytes it fetches; Process again without a kept live transcript (plan 18 section 10.2); chunk M2 (waits for the owner's labels); the Tauri bump for Dependabot alert #1 (`glib` 0.18.5, Linux only). The first-run size for upgraders is fixed in this release; whether `disk_needed_bytes` still counts the punctuation model's full 372 MB is not re-checked here.
- **An update from beta.6 with a day limit set** (7, 30 or 90 stored) keeps the limit. The first check after the update finds audio already past it, so it must show the hold dialog and delete nothing until the owner answers. That is the release's main behaviour change for upgraders; section 3.4 tests it on a scratch root and section 7 on the installed copy.
- **Going back to beta.6 is not neutral for audio.** Keep forever is a default, not a stored value, so a reinstalled beta.6 falls back to its own 30-day default and deletes older unpinned audio at launch without a warning. Section 7 says to store "Keep audio for" (or pin) first.
- **A meeting deleted from the list menu** can stay in the library's heads-up or held card until the notices refresh (on focus after a minute, or after the next choice in them). Seen by reading the code, not in use.
- **Docs PR #28** ("docs: correct public claims ...") is open and touches README, PRIVACY, INSTALL, KNOWN-ISSUES, docs/01, docs/04, SETUP-WITH-AN-AGENT and CHANGELOG, which this branch also edits. Whichever merges second needs its conflicts resolved.

## 3. Owner tests on a build of the final tip

A test build of the integration branch is in `C:\Users\ebin\claude-ground\diarist-releases\0.1.0-beta.7-test\` (`app\diarist.exe`, `data\` with only `engine\` and `models\`, `start-beta7-test.cmd`, and `README.txt` with the steps). It runs against that scratch data root and never touches the installed app's folder. Quit the installed Diarist first: one instance runs at a time. On 18 Nov 2026 Microsoft Defender's ransomware protection rule blocked the test build's smoke start (event 1121, the rule "Use advanced protection against ransomware"), as it blocked beta.6's, so the build has not been run; see the KNOWN-ISSUES row for the steps to allow it. The test build's SHA-256 is `cf332de66246827d82299e9c55193c3a7f6f7d015f641e7d61db401eb620f60e`.

### 3.1 The list menu and the sort (issues 19 and 20)

- [ ] Right-click a meeting: Open, Pin audio (or Unpin), Delete. Shift+F10 and the ContextMenu key open the same menu on the selected row. Delete asks first.
- [ ] The sort control above the list: Recorded or Added, newest or oldest first. The choice survives a restart.

### 3.2 Speaker jump (issue 21)

- [ ] In a meeting, L and H move to the same speaker's next and previous line (from the playhead once played).
- [ ] Clicking a speaker chip in the header opens its menu with Next line and Previous line.

### 3.3 Keys in open menus

- [ ] With a menu open (the list menu, a chip menu), R does not start a recording, P does not pin, Space and Delete do nothing outside the menu, and Escape closes the menu. Alt+F4 still closes the window.

### 3.4 Retention (issue 22)

- [ ] A fresh root: Settings, "Keep audio for" reads 0 (keep forever), and Settings shows the space recordings use.
- [ ] The README's scenario: three short imports aged with the python line, 30 days set on the scratch root only. First start: the hold dialog lists the overdue meeting and its size, with Delete as set, Keep everything, Review and pin (and Ask later); nothing is deleted before a choice. After Delete as set and the second aging step: the heads-up lists the meeting due within 3 days with Pin beside it, and the held card lists the overdue one ("Diarist kept it because it was not listed here in time") with Pin, Delete now, Dismiss.
- [ ] Pinning from the list menu takes the meeting out of the notices.

### 3.5 The record sheet

- [ ] The New recording sheet says that recording other people needs their consent where the law asks for it, and that headphones give the cleanest result.

### 3.6 An import and a short recording on the final build

- [ ] Import a short English file (Ctrl+O): punctuated text, the punctuation line, playback.
- [ ] Record about a minute with a video playing, then stop: live words and speakers, the final transcript and playback after Stop, no new errors in the app log.

## 4. Before building

- [ ] The draft PR "Diarist 0.1.0-beta.7" is marked ready and merged into `main` (**owner**), and the build runs from that commit with a clean tree.
- [ ] Version `0.1.0-beta.7` in the workspace `Cargo.toml`, `Cargo.lock`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/package.json`, README.md (download name and status line), docs/INSTALL.md, docs/DEV.md and docs/SETUP-WITH-AN-AGENT.md. `cargo run --release -p diarist-cli -- --version` prints `diarist-cli 0.1.0-beta.7`.
- [ ] CHANGELOG: the `[0.1.0-beta.7]` heading carries 2026-11-18 as a placeholder; change it to the day the release is published.
- [ ] `RELEASED_SCHEMA` is 5 and no beta.7 branch added a migration (checked at integration, 18 Nov 2026): beta.7 opens a beta.6 database as it is and makes no new `diarist.db.bak-<n>`. No database backup step is needed.
- [ ] Checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; in `apps/desktop`, `bun run check`, `bun run test`, `bun run build`. The pull-request CI runs the same on `windows-latest`.
- [ ] docs/KNOWN-ISSUES.md: the status line names 0.1.0-beta.7; the retention row is in; the updater row gets the result of section 7.
- [ ] Privacy scan of every changed file before each push (section 9).

## 5. Build with the updater key (**owner**, the reference machine)

From `apps/desktop`, in PowerShell:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\diarist.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
bun run tauri build
```

- [ ] Outputs: `target\release\bundle\nsis\Diarist_0.1.0-beta.7_x64-setup.exe` and its `.sig` (the updater signature), `target\release\diarist.exe`, `target\release\diarist.pdb`.
- [ ] SHA-256 and size of the installer: `Get-FileHash -Algorithm SHA256 <installer>` and `(Get-Item <installer>).Length`.
- [ ] Smoke test the built exe without installing it, so this machine's installed 0.1.0-beta.6 stays in place for section 7: close the installed app, then start `target\release\diarist.exe` with `DIARIST_DATA_DIR` set to an empty scratch folder. It shows the setup screen, Settings, About says "0.1.0-beta.7", and "Keep audio for" reads 0.

## 6. Symbols, then the draft release (**owner**)

- [ ] Zip `target\release\diarist.exe` and `target\release\diarist.pdb` together as `diarist-0.1.0-beta.7-symbols.zip`, check that it resolves (`cdb -z diarist.exe -y . -c ".lines -e; ln diarist+0x1000; q"`, docs/DEV.md "Release PDB"), and keep it locally next to the signing key's backup. It is not attached to the release.
- [ ] Release notes: the CHANGELOG section, the line "The installer is not code-signed yet; SmartScreen asks once.", the installer's SHA-256 and size, and a line that the database is unchanged from 0.1.0-beta.6 (no upgrade on first start, and 0.1.0-beta.6 can open it again).
- [ ] `gh release create v0.1.0-beta.7 --draft --prerelease --target main --title "Diarist v0.1.0-beta.7" --notes-file <notes.md> <installer> <installer>.sig`.
- [ ] The draft has the installer and the `.sig` with the local sizes.

## 7. Publish, then the updater PR, then the update from beta.6 (**owner**)

The installed 0.1.0-beta.6 reads `updater/latest.json` on `main`, and draft assets are not public, so the manifest changes only after the release is public. Check for updates is manual, so the time between the manifest commit and the check below is the only exposure.

- [ ] Before: this machine's installed copy (`%LOCALAPPDATA%\Diarist\diarist.exe`) is 0.1.0-beta.6, and its data folder has a byte copy elsewhere (a copy of the folder, not a database backup: the schema does not change). Note its "Keep audio for" value.
- [ ] Publish the draft (it stays a pre-release). Publishing pushes the tag; the release workflow's guard job finds the installer on the release and builds nothing. If it fails with "Could not list releases", re-run it.
- [ ] The asset answers publicly: `curl.exe -sIL https://github.com/ebt55/diarist/releases/download/v0.1.0-beta.7/Diarist_0.1.0-beta.7_x64-setup.exe` ends in `200` with the local size as `content-length`.
- [ ] Write `updater/latest.json` from the `.sig` (from the repository root; node writes UTF-8 without a byte-order mark, which the updater's JSON parser needs):

  ```sh
  node -e "const fs=require('fs');const v='0.1.0-beta.7';const sig=fs.readFileSync('target/release/bundle/nsis/Diarist_'+v+'_x64-setup.exe.sig','utf8').trim();fs.writeFileSync('updater/latest.json',JSON.stringify({version:v,notes:'Diarist '+v+': see CHANGELOG.md',pub_date:new Date().toISOString().replace(/\.\d{3}Z$/,'Z'),platforms:{'windows-x86_64':{signature:sig,url:'https://github.com/ebt55/diarist/releases/download/v'+v+'/Diarist_'+v+'_x64-setup.exe'}}},null,2)+'\n')"
  ```

- [ ] Open the updater PR ("updater: offer 0.1.0-beta.7") with only `updater/latest.json`, after the privacy scan, and merge it into `main`. The CI runs on it too.
- [ ] `curl.exe -s https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json` shows 0.1.0-beta.7 (that address caches for up to five minutes).
- [ ] At once, in the installed 0.1.0-beta.6: Settings, About, Check for updates offers 0.1.0-beta.7; Install and restart. Expected: download, signature check, the installer runs, the app restarts as 0.1.0-beta.7 with the same meetings, and Diagnostics shows no new failures. No migration runs and no new `.bak` appears. "Keep audio for" keeps the value noted before (0 if it was never changed). If a limit was set and audio is past it, the hold dialog appears and nothing is deleted until a choice is made.
- [ ] Going back to 0.1.0-beta.6, if it is ever needed: first set "Keep audio for" explicitly in beta.7 (choose a value other than the default, then 0 again, so the 0 is stored), or pin what must stay. Then install beta.6's installer again; it opens the same database (schema 5). Beta.6 knows nothing of `retention.state` or the warnings: with no stored value it falls back to its own default of 30 days and deletes older unpinned audio at launch without asking, and a stored limit it applies at once.
- [ ] If any of it fails: revert the manifest commit on `main` and push at once, so no other installed copy is offered the update, then keep the app log (`logs\diarist.log` in the data folder) for the cause. The published release stays: its installer still works when downloaded by hand.
- [ ] Record the outcome in the KNOWN-ISSUES updater row.

## 8. After

- [ ] The installer downloaded from the release page has the SHA-256 in the notes.
- [ ] The CHANGELOG date is the publishing day.
- [ ] The next release: runtime 0.3.0 first, with the checks of section 2.

## 9. Privacy scan before every push

The owner's Zoom recordings, transcripts, meeting titles and participant names never go into the repository.

- [ ] Before each push (the integration branch, `main`, `updater/latest.json`): `python -I C:\Users\ebin\claude-ground\diarist-private\tools\privscan\scan.py <repo> <range>` over every line added since the last pushed commit. It prints needle numbers, never the text. Needle 92 in docs/05 is known on `main`; any other hit stops the push.
- [ ] Never commit `tools/eval/sessions.toml`, `tools/eval/.work`, run folders or audio.
