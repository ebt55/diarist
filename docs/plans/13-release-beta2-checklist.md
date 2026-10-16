# 0.1.0-beta.2 release checklist

Status: prepared 16 Oct 2026, nothing published. The release is hand-built as in `docs/DEV.md` ("Desktop app (Tauri)": update endpoint, versioning, release PDB) and the plan's chunk I (`docs/plans/12-release-beta2.md`, section 4). It is the first release that installed copies update to, so the order in step 5 matters. Steps marked **owner** publish something, push to `main`, or need the signing key; nothing happens there without the owner's go.

## 1. Before building

- [ ] Every chunk in section 2 of the plan is committed; the branch is merged into `main` (**owner**), and the build runs from that commit with a clean tree.
- [ ] Version `0.1.0-beta.2` in the workspace `Cargo.toml`, `apps/desktop/src-tauri/tauri.conf.json` and `apps/desktop/package.json` (Cargo.lock follows), and in README.md and docs/INSTALL.md. `cargo run --release -p diarist-cli -- --version` prints `diarist-cli 0.1.0-beta.2`.
- [ ] CHANGELOG: the `[0.1.0-beta.2]` heading carries the publishing date.
- [ ] Checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; in `apps/desktop`, `bun run check`, `bun run test`, `bun run build`.
- [ ] Eval table for the shipped configuration: a fresh full-set run of the rebuilt `diarist-cli` (`tools/eval/README.md`; runs cached from an older binary do not count, and a runtime-default baseline needs `engine_args = ["--asr.streaming.rnnt_right_context", "1"]`), written to `docs/eval/<date>-<config>.md` and `docs/eval/benchmarks.json`; README and docs/04 numbers updated from it.
- [ ] docs/KNOWN-ISSUES.md: status line, the Vulkan and CPU row, the punctuation rows, and the updater row (step 5 fills in the result).

## 2. Build (**owner**, the reference machine)

From `apps/desktop`, in PowerShell:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\diarist.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
bun run tauri build
```

- [ ] Outputs: `target\release\bundle\nsis\Diarist_0.1.0-beta.2_x64-setup.exe` and its `.sig`, `target\release\diarist.exe`, `target\release\diarist.pdb`.
- [ ] SHA-256 and size of the installer: `Get-FileHash -Algorithm SHA256 <installer>` and `(Get-Item <installer>).Length`.
- [ ] Smoke test the built exe without installing it, so this machine's installed 0.1.0-beta.1 stays in place for step 5: close the installed app (one instance at a time), then start `target\release\diarist.exe` with `DIARIST_DATA_DIR` set to an empty scratch folder; it shows the setup screen and "0.1.0-beta.2" in Settings, About.

## 3. Archive the PDB (**owner**)

- [ ] Zip `target\release\diarist.exe` and `target\release\diarist.pdb` together as `diarist-0.1.0-beta.2-symbols.zip`. A PDB matches only this build.
- [ ] Keep it privately (next to the signing key's backup), or attach it to the release in step 4; the owner decides. A published PDB holds function names and the build machine's source paths, no code.
- [ ] Check it resolves: in the folder with both files, `cdb -z diarist.exe -y . -c ".lines -e; ln diarist+0x1000; q"` prints a function and a source line (docs/DEV.md, "Release PDB").

## 4. Draft pre-release (**owner**)

- [ ] Release notes: the CHANGELOG section, the line "The installer is not code-signed yet; SmartScreen asks once.", and the installer's SHA-256 and size.
- [ ] `gh release create v0.1.0-beta.2 --draft --prerelease --target main --title "Diarist v0.1.0-beta.2" --notes-file <notes.md> <installer> <installer>.sig` (plus the symbols zip if it is published).
- [ ] The draft has the installer and the `.sig` with the local sizes.

## 5. Publish, then the manifest: the first update (**owner**)

The installed 0.1.0-beta.1 reads `updater/latest.json` on `main`, and draft assets are not public, so the manifest changes only after the release is public. Check for updates is manual, so the time between the manifest commit and the check below is the only exposure.

- [ ] Before: this machine's installed copy (`%LOCALAPPDATA%\Diarist\diarist.exe`) is 0.1.0-beta.1, and its data folder has a byte copy elsewhere. 0.1.0-beta.2 runs the first database migration ever (0002; the app also keeps `diarist.db.bak-<old schema version>`).
- [ ] Publish the draft (it stays a pre-release). Publishing pushes the tag; the release workflow's guard job finds the installer on the release and builds nothing. If it fails with "Could not list releases", re-run it.
- [ ] The asset answers publicly: `curl.exe -sIL https://github.com/ebt55/diarist/releases/download/v0.1.0-beta.2/Diarist_0.1.0-beta.2_x64-setup.exe` ends in `200` with the local size as `content-length`.
- [ ] Write the manifest from the `.sig` (from the repository root; node writes UTF-8 without a byte-order mark, which the updater's JSON parser needs):

  ```sh
  node -e "const fs=require('fs');const v='0.1.0-beta.2';const sig=fs.readFileSync('target/release/bundle/nsis/Diarist_'+v+'_x64-setup.exe.sig','utf8').trim();fs.writeFileSync('updater/latest.json',JSON.stringify({version:v,notes:'Diarist '+v+': see CHANGELOG.md',pub_date:new Date().toISOString().replace(/\.\d{3}Z$/,'Z'),platforms:{'windows-x86_64':{signature:sig,url:'https://github.com/ebt55/diarist/releases/download/v'+v+'/Diarist_'+v+'_x64-setup.exe'}}},null,2)+'\n')"
  ```

- [ ] Commit `updater/latest.json` on `main` and push.
- [ ] `curl.exe -s https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json` shows 0.1.0-beta.2 (that address caches for up to five minutes).
- [ ] At once, in the installed 0.1.0-beta.1: Settings, About, Check for updates offers 0.1.0-beta.2; Install and restart. Expected: download, signature check, the installer runs, the app restarts as 0.1.0-beta.2 with the same meetings, Diagnostics without new failures, and `diarist.db.bak-1` (the copy of the schema-1 database that 0.1.0-beta.1 left) next to `diarist.db`.
- [ ] If any of it fails: revert the manifest commit on `main` and push at once, so no other installed copy is offered the update, then keep the app log (`logs\diarist.log` in the data folder) for the cause. The published release stays: its installer still works when downloaded by hand.
- [ ] Record the outcome in the KNOWN-ISSUES updater row.

## 6. After

- [ ] The installer downloaded from the release page has the SHA-256 in the notes.
- [ ] The site's numbers come from `docs/eval/benchmarks.json`.
- [ ] The release workflow's header comment (`.github/workflows/release.yml`) still describes the old hand-built order (manifest before publishing); align it in the next change to that file.
