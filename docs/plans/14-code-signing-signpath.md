# Code signing through SignPath Foundation

Status: prepared 16 Oct 2026 (Opus 5.5 agent), nothing applied for, registered or published. Research is current as of that date; sources are in section 9. What exists:

- `.github/workflows/sign-release.yml`: new, manual only (`workflow_dispatch`), never run. `release.yml` is unchanged and the hand-built path (`docs/plans/13-release-beta2-checklist.md`) keeps working.
- The site page `https://diarist.dev/code-signing/` ("Code signing policy"), with links from the footer and the hero's download line: one commit on the local branch `code-signing-policy` of the diarist.dev repository, not pushed. `bun run build` and `bun run check` pass.

Section 2 says when to apply; section 3 is the checklist, in order, top to bottom. Steps marked **owner** need Ebin personally: accounts, MFA, settings, the application, approvals and publishing. 0.1.0-beta.2 ships unsigned by the hand-built path whatever happens here.

## 1. Why SignPath, and what it gives

**The options.** Microsoft's Artifact Signing does not take individual developers outside the US and Canada. An OV certificate costs money every year and needs a hardware token or a cloud key. Certum's open-source certificate (about EUR 49 a year, issued to the individual after an identity check) works but has no official CI support. SignPath Foundation signs OSI-licensed open-source projects for free: the certificate is issued to SignPath Foundation, the private key stays in SignPath's HSM, and SignPath checks for every signing request that the files were built by the project's GitHub Actions workflow from its public repository. Nothing new to back up: SignPath holds the key. The updater key (`%USERPROFILE%\.tauri\diarist.key`) stays as it is and still needs its backup.

**What users see.** Windows names **SignPath Foundation** as the publisher, in the SmartScreen dialog, in the file's Properties (Digital Signatures) and in `Get-AuthenticodeSignature`. Not "Diarist", not Ebin. That is the cost of a free certificate: SignPath's terms make the Foundation the publisher.

**SmartScreen.** Signing does not remove the warning on day one. SmartScreen weighs the reputation of the signing certificate and of each file's hash; a newly signed file can still be "unrecognized" until enough clean downloads accumulate, and Microsoft gives no threshold (its guidance speaks of weeks and hundreds of installs). What changes at once: the dialog shows a verified publisher instead of "Unknown publisher", and reputation can carry from one release to the next because every release is signed with the same identity. How much reputation a SignPath Foundation certificate brings for a new project is not published (open question 7).

**Smart App Control.** On Windows 11 with Smart App Control on, every executable is checked, not only downloads: an unsigned file without a positive cloud verdict is blocked, and a validly signed one is allowed when the cloud has no verdict. So signing `diarist.exe` is what lets the app itself run on those PCs, and signing the installer lets it install. It does not help the NVIDIA runtime: `nemo-speech.exe` and its CUDA DLLs stay unsigned, and SignPath's terms forbid signing other projects' binaries, so the KNOWN-ISSUES row about Smart App Control and the runtime stays open until NVIDIA signs its packages.

**What gets signed, and what does not.**

| File | Signed | Why |
|---|---|---|
| `Diarist_<version>_x64-setup.exe` | Yes, SignPath request 2 | What users download; SmartScreen and Smart App Control check it |
| `diarist.exe` (inside the installer) | Yes, SignPath request 1, before bundling | Smart App Control checks every executable; firewall and antivirus prompts show the publisher |
| `uninstall.exe` | No | The installer writes it during installation; Tauri can sign it only through a local signing command at bundle time (`!uninstfinalize`), which SignPath's approval flow cannot serve |
| NSIS plugin DLLs the installer unpacks while it runs | No | Upstream NSIS and Tauri binaries; SignPath allows them unsigned inside a signed installer but not signed by this project |
| NeMo-Speech.cpp runtime, models | No | Other projects' files, downloaded at first run on request, checked against their SHA-256 |

SignPath cannot open an NSIS installer to sign the exe inside it (NSIS is not one of its container formats; MSI, CAB, ZIP and others are), so the exe is signed first and the installer is built around the signed exe. Two signing requests per release means **two approvals per release** in SignPath's web page.

**The updater does not need the inner exe signed.** It checks the minisign `.sig` over the downloaded installer against `plugins.updater.pubkey` and runs the installer; Authenticode plays no part in that check. What the updater does need is a `.sig` computed over the **final, signed** installer: Authenticode signing rewrites the file (certificate table, header checksum), so a `.sig` made before it fails in every installed copy. Section 4 sets the order.

## 2. Before applying: the reputation question

SignPath's terms say that executable programs need "a certain verifiable reputation" before they sign them in their own name, and the application form has a Reputation field asking for media reports, blog articles, Wikipedia, download counts and GitHub insights. Recent outcomes for young projects:

- docent (28 May to 4 Jun 2026, from opening its tracking issue to the decline): declined for lack of external verification and public traction; invited to reapply later.
- dictus-desktop (applied 16 Aug 2026): first reply on 16 Sep asking for public references; declined on 7 Oct 2026 despite 1,356 downloads over 7 releases and 24 stars; SignPath welcomed a new application once the project is more widely known, and offered a paid subscription instead.
- n1mm-scope-bridge (Oct 2026): decided not to apply yet for the same reason.

Diarist went public on 15 Oct 2026. An application now will probably be answered "not yet". A decline costs the form and does not bar a later application, and SignPath's reply says what evidence they want. **Recommendation:** do sections 3.1 and 3.2 now (cheap, and the policy page is a precondition anyway), apply as soon as the page is live, and if declined, reapply after 0.1.0-beta.3 with whatever outside references exist by then. Never invent reputation: the field gets only real, checkable links.

## 3. Owner checklist

### 3.1 GitHub (before the policy page goes live, because the page states these controls)

- [ ] **owner**: GitHub account, Settings, Password and authentication: two-factor authentication on (an authenticator app or a passkey), recovery codes stored offline.
- [ ] **owner**: Repository Settings, Rules, Rulesets, New branch ruleset: name `main`, enforcement Active, target "Include default branch", rules "Restrict deletions" and "Block force pushes", no bypass list. Leave "Require a pull request before merging" off unless SignPath asks for it (open question 3): for a one-person project it turns every commit into a pull request.
- [ ] **owner**: New tag ruleset: name `release tags`, target pattern `v*`, rules "Restrict updates" and "Restrict deletions". Leave creation allowed: publishing a draft release creates its tag.
- [ ] **owner**: Settings, Code security (Advanced Security on newer layouts), "Private vulnerability reporting": Enable. The policy page links to it as the private way to report a bad file.
- [ ] **owner**: Settings, Actions, General: "Allow GitHub Actions to create and approve pull requests" on (release.yml needs it too). Workflow permissions can stay read-only; both workflows declare their own.
- [ ] **owner**: Settings, Secrets and variables, Actions: `TAURI_SIGNING_PRIVATE_KEY` = the contents of `%USERPROFILE%\.tauri\diarist.key` (the key itself, not a path: `tauri signer sign` needs the literal key) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = empty. Skip if already set for release.yml. Any workflow on the repository can read repository secrets; only the owner can add workflows.

### 3.2 Publish the policy page and these files

- [ ] **owner**: In `C:\Users\ebin\claude-ground\diarist.dev`, review branch `code-signing-policy` (one commit: `src/code-signing.html`, the page block in `build/build.ts`, the footer link, the hero link, a note in DESIGN.md and README.md). `bun run build` then `bun run check`. Merge into `main` and push; Workers Builds deploys. `https://diarist.dev/code-signing/` then answers, and every page's footer and the home page's download line carry "Code signing policy".
- [ ] **owner**: Merge this plan and `.github/workflows/sign-release.yml` into `main` of ebt55/diarist (the page links the workflow file on `main`; the workflow can only be started once it is on `main`).
- [ ] Follow-up for whoever edits README.md next: a short "Code signing" paragraph linking `https://diarist.dev/code-signing/`. The repository's front page is the second place SignPath's reviewers look for the term.

### 3.3 Apply (**owner**)

- [ ] Use the Google account (2-Step Verification on) as the SignPath identity. SignPath's sign-in choices are Google, Microsoft, a SignPath username and password, or enterprise SSO; the terms require MFA on the SignPath account, and with Google sign-in the Google account's 2-Step Verification is what protects it.
- [ ] Open `https://signpath.org/apply` and fill in the form. The page embeds a HubSpot form (since April 2026); its fields are expected to match SignPath's earlier spreadsheet form (OSSRequestForm-v4, still in their website repository), which these answers follow. Use the live form's wording where it differs.

| Field | Answer |
|---|---|
| Name | `Diarist` |
| Handle | `diarist.dev` (the form allows the home page's domain when the home page is the root of a custom domain; the handle becomes part of the short link `sig.fo/<handle>` in the certificate) |
| Type | Program |
| License | MIT License, `https://opensource.org/licenses/MIT` |
| Repository URL | `https://github.com/ebt55/diarist` (it must be the URL the CI builds from; it is checked on every signing request) |
| Homepage URL | `https://diarist.dev` |
| Tagline | `Local meeting transcription with speaker labels` (the installer's `shortDescription`) |
| Description | One paragraph that stays valid across versions: "Diarist is a Windows desktop application that records meetings or imports recordings, transcribes them on the user's own GPU or CPU with open speech recognition and speaker diarization models, and stores the transcripts, with a speaker label on every line, in a searchable database on the user's computer. It has no account, no cloud service and no telemetry. The speech runtime and the models are downloaded from their publishers at first run, at the user's request, and are not part of the installer. Diarist is maintained by one person." |
| Download URL | `https://diarist.dev` (the home page is the download page; it carries the Code signing policy link next to the Download button) |
| Privacy Policy URL | `https://diarist.dev/docs/privacy/` |
| Reputation | Only real, checkable items, current on the day: the repository's stars, forks and release download counts (Insights), the first public release (0.1.0-beta.1, 15 Oct 2026) and later ones, the upstream report NVIDIA/NeMo-Speech.cpp#67, and any article, forum thread or listing that mentions Diarist. Say plainly that the project is new. |
| Wikipedia URL | leave empty |
| Build System | GitHub Actions |
| User Full Name, User Email | Ebin's name and the address SignPath should write to |
| Terms of use | type `I hereby accept the terms of use` after reading `https://signpath.org/terms` |

- [ ] If the form has a free-text note, or in the first reply: the runtime's CUDA package, which the app downloads from NVIDIA's GitHub release at the user's request, contains NVIDIA's CUDA libraries under the CUDA EULA; none of it is in the installer or would be signed (open question 5). Better stated up front than discovered.
- [ ] Wait for the email. Answer evidence requests with links only. If declined, note what they asked for and reapply later (section 2).

### 3.4 After acceptance: SignPath setup (**owner**, at app.signpath.io)

SignPath may create some of this during onboarding; check what exists before adding.

- [ ] Accept the organization invitation from the acceptance email. Organization settings shows the Organization ID (a GUID): GitHub secret `SIGNPATH_ORGANIZATION_ID`.
- [ ] Project: slug `diarist` (GitHub secret `SIGNPATH_PROJECT_SLUG`), repository URL `https://github.com/ebt55/diarist`.
- [ ] Trusted build systems: add the predefined **GitHub.com** trusted build system to the organization and link it to the project. The SignPath GitHub App is only needed for audit-log policies (GitHub Enterprise); do not install it unless SignPath asks.
- [ ] Artifact configurations, Add, Custom, exactly these two slugs (the workflow names them):

  `diarist-exe`:

  ```xml
  <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
    <parameters>
      <parameter name="version" required="true" />
    </parameters>
    <zip-file>
      <pe-file path="diarist.exe" product-name="Diarist" product-version="${version}">
        <authenticode-sign />
      </pe-file>
    </zip-file>
  </artifact-configuration>
  ```

  `diarist-installer`:

  ```xml
  <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
    <parameters>
      <parameter name="version" required="true" />
    </parameters>
    <zip-file>
      <pe-file path="Diarist_${version}_x64-setup.exe" product-name="Diarist" product-version="${version}">
        <authenticode-sign />
      </pe-file>
    </zip-file>
  </artifact-configuration>
  ```

  The `<zip-file>` root is there because `actions/upload-artifact` zips what it uploads. `product-name` and `product-version` are the metadata restrictions SignPath's terms require (product name = the project's name, product version the same in every file of a build); Tauri writes ProductName `Diarist` and ProductVersion `<version>` into both files (the installed 0.1.0-beta.1 exe shows exactly that).
- [ ] CI user: Users and groups, add a CI user (for example `diarist-github`). Generate its API token (shown once): GitHub secret `SIGNPATH_API_TOKEN`. SignPath recommends one CI user per project; with origin verification on, interactive users cannot submit.
- [ ] Signing policies (the slugs the workflow offers): `test-signing` with the test certificate, `release-signing` with the SignPath Foundation certificate (it may show "CSR pending" until SignPath imports the certificate). For both: Verify origin on; allowed branch names `main`; submitters: only the CI user; "Use approval process" on, approvers: Ebin, approvals required: 1. Accept whatever build and SCM policies SignPath sets for Foundation projects (runners must be GitHub-hosted; re-runs may be refused).
- [ ] Notifications: make sure approval requests reach the inbox you read; each one waits at most two hours (the workflow's limit) before the run fails.

### 3.5 First run: test certificate (**owner**)

- [ ] Actions, sign-release, Run workflow. "Use workflow from": `main`. tag: `v` plus the version in `tauri.conf.json` on `main` (a test run may use a version that is already released). signing-policy: `test-signing`.
- [ ] Approve the two requests in SignPath as the emails arrive: first `diarist.exe`, after the checks and the release build (estimated 25 to 40 minutes on a cold runner; not measured), then the installer a few minutes after that approval.
- [ ] When the run is green, download the artifact `diarist-<version>-test-signed`. Both files show a signature (by a test certificate, untrusted, which is expected) in Properties, Digital Signatures. Start the signed `diarist.exe` directly with `DIARIST_DATA_DIR` set to an empty scratch folder (as plan 13 step 2); install the test-signed installer only on a spare PC or VM, since installing it here would replace the installed copy that the next update test needs.
- [ ] Tell SignPath that test signing works and give them the run's link, so they can import the release certificate (one Foundation project waited after this step; follow up if nothing happens within two weeks).

### 3.6 Each signed release (**owner**)

Replaces steps 2 to 4 of the hand-built checklist (plan 13); its steps 1, 5 and 6 stay.

- [ ] Version bumped, CHANGELOG dated, checks green, everything merged into `main`. **Do not push the tag**: release.yml would build an unsigned installer for it, and sign-release would then refuse.
- [ ] Actions, sign-release, Run workflow from `main`, tag `v<version>`, signing-policy `release-signing`.
- [ ] Approve request 1 (`diarist.exe`), then request 2 (the installer), each within two hours of its email.
- [ ] The run leaves: a draft pre-release with `Diarist_<version>_x64-setup.exe` and its `.sig` (notes with the SHA-256, the size and the code signing policy link); the artifact `diarist-<version>-symbols` (signed exe plus PDB: download it once and archive it, as plan 13 step 3 describes; its source paths are the runner's, no user folder); a pull request "updater: offer v<version>".
- [ ] Download the installer from the draft. `Get-AuthenticodeSignature` says Valid, signer SignPath Foundation; `Get-FileHash -Algorithm SHA256` matches the notes. Smoke test the signed `diarist.exe` from the symbols artifact with a scratch data root (plan 13 step 2). Do not install the new installer on this PC yet: the update test below needs the previous version installed.
- [ ] Publish the draft (it stays a pre-release). Publishing creates the tag on the commit that was built; release.yml's guard finds the installer and builds nothing.
- [ ] The asset answers publicly (`curl.exe -sIL https://github.com/ebt55/diarist/releases/download/v<version>/Diarist_<version>_x64-setup.exe` ends in 200).
- [ ] Merge the updater pull request, then the update test of plan 13 step 5 (Check for updates in the installed previous version; revert the manifest commit at once if anything fails).
- [ ] Update the policy page's status line on the site after the first signed release.

A release can still be built by hand (plan 13); it is simply unsigned. release.yml stays as the tag-push path for unsigned CI builds.

## 4. CI design: `.github/workflows/sign-release.yml`

**Trigger.** `workflow_dispatch` only, from `main`, with two inputs: `tag` (the release tag to create, matched against `tauri.conf.json`) and `signing-policy` (`test-signing` or `release-signing`, default test). It cannot start by accident, and it refuses any other ref, so what SignPath verifies (repository, branch `main`, commit) is exactly what was built.

**Why from `main` and not from a tag.** A tag push starts release.yml, which would build an unsigned installer for the same tag. So sign-release never pushes the tag: the draft release targets the built commit (`gh release create --draft --target <sha>`), and publishing the draft creates the tag there. release.yml's guard then sees the installer on the release and builds nothing, the same as for a hand-built release. A side benefit: SignPath's "allowed branch names" can stay `main`, and whether SignPath matches tags there (undocumented) does not matter.

**Jobs.**

1. `guard` (Ubuntu): run from `main`; the tag looks like `v1.2.3` or `v1.2.3-beta.4`; for release-signing, the release has no installer yet (a failed release listing fails the job instead of being read as "no release", as in release.yml), and the tag does not exist or already points at the commit being built.
2. `windows` (`windows-latest`, GitHub-hosted, no build cache, since origin verification expects the signed files to come from this source and run alone):
   1. Toolchains and `bun install --frozen-lockfile` as in release.yml; tag matches `tauri.conf.json`; `cargo fmt`, `clippy -D warnings`, `cargo test`.
   2. `bun run tauri build --ci --no-bundle`: `target/release/diarist.exe`, unsigned; no installer and no updater artifacts yet.
   3. Upload `diarist.exe` (`actions/upload-artifact` v7.0.1, pinned by commit), submit SignPath request 1 (`signpath/github-action-submit-signing-request` v3.0, pinned by commit; artifact configuration `diarist-exe`, parameter `version`), wait up to two hours for the approval and the signed file.
   4. Check the signature (release-signing: status Valid, signer `CN=SignPath Foundation`, timestamp present; test-signing: an intact signature), then put the signed exe in place of the unsigned one.
   5. `bun run tauri bundle --ci --bundles nsis --no-sign --no-binary-patching --config '{"bundle":{"createUpdaterArtifacts":false}}'`. `--no-binary-patching` (Tauri CLI 2.12.0 and later; the project has 2.12.3) stops the bundler from writing the bundle type into the exe, which would break its signature; the patch matters only when one platform ships several installer types, and Diarist ships NSIS only, so the updater looks up `windows-x86_64` in latest.json as before. Updater artifacts are off here because a `.sig` over this unsigned installer would be wrong.
   6. Extract `diarist.exe` from the installer with 7-Zip and compare its SHA-256 with the signed exe, so an installer around an unsigned or patched exe never reaches SignPath.
   7. Upload the installer, submit SignPath request 2 (`diarist-installer`), wait, check the signature.
   8. test-signing stops here and keeps both signed files as the artifact `diarist-<version>-test-signed` (7 days).
   9. release-signing: keep the signed exe and the PDB as `diarist-<version>-symbols` (90 days); `bun run tauri signer sign --app-version <version>` over the **signed** installer (writes `<installer>.sig`, with the version in the signature's trusted comment, as `tauri build` does); verify that `.sig` in Node against `plugins.updater.pubkey` (BLAKE2b-512 digest, Ed25519, trusted comment, version), which is the check every installed copy makes, and which also proves the GitHub secret is the right key; create the draft pre-release with the installer and the `.sig`; write `updater/latest.json` and open the pull request, both as in release.yml.

**The order, once more.** Build exe, sign exe (approval 1), bundle around it, sign installer (approval 2), compute the updater `.sig` over the signed installer, publish. Any `.sig` computed earlier is over different bytes.

**Secrets** (listed at the top of the file): `SIGNPATH_API_TOKEN`, `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Permissions: `contents: write` (draft release, updater branch), `pull-requests: write`, `actions: read` (SignPath's connector reads the run and downloads the artifacts with the job token).

**Not used: Tauri's own Windows signing.** `bundle.windows.certificateThumbprint` needs a certificate in the runner's store, and `bundle.windows.signCommand` runs a command that must sign each file on the spot; Tauri would call it for the exe, the NSIS plugin DLLs, the uninstaller and the installer. With SignPath each call would be a separate signing request with its own approval, and origin verification needs the files to pass through the GitHub action as workflow artifacts. `tauri.conf.json` stays as it is.

**Limits.** Neither workflow has run yet. The 7-Zip check, the SignPath metadata check of Tauri's version resource and the two-hour approval window are untested until the first test-signing run. The Tauri CLI downloads the NSIS toolset (with Tauri's NSIS plugin) from GitHub during the bundle step, as it does on the reference machine; those files end up in the installer unsigned. A failed run may not be re-runnable under SignPath's re-run policy: start a new run.

**Reference integrations.** WinScript (Tauri, SignPath Foundation) uses the same two-pass order (sign the exe, `tauri bundle`, sign the installer, then `tauri signer sign` and latest.json), with a read-only file attribute as the workaround that `--no-binary-patching` now replaces. Seelen UI (Tauri, SignPath Foundation) signs binaries, bundles with `--no-binary-patching`, signs the bundles, then regenerates the updater signatures. Mini Diarium (Tauri) documents the test-certificate phase and the wait for the release certificate.

## 5. User-facing docs once signed builds ship

Not changed by this plan (other agents own these files); the wording to use when the first signed release ships, and two corrections that apply now.

**SmartScreen explainer, now (unsigned).** For docs/INSTALL.md step 2 and the README: "Windows SmartScreen may show "Windows protected your PC". It means Windows has not seen this file often enough to know its reputation, not that it found malware; the installer is not code-signed yet. Download it only from the releases page or diarist.dev, compare its SHA-256 with the release notes (`Get-FileHash -Algorithm SHA256 .\Diarist_<version>_x64-setup.exe` in PowerShell), then choose More info, Run anyway."

**SmartScreen explainer, signed.** "The installer and the app are signed; Windows shows the publisher as SignPath Foundation, which signs open-source projects for free after checking that each file was built from the public source ([code signing policy](https://diarist.dev/code-signing/)). A new release can still meet "Windows protected your PC" until enough people have downloaded it: More info should show SignPath Foundation as the publisher; then Run anyway. If it shows "Unknown publisher", do not run the file. The SHA-256 is in the release notes."

**Where.**

- docs/INSTALL.md, section 1 step 2: the signed explainer; the version line.
- README.md, the paragraph after Quick start (lines 61 to 63 today): the signed explainer in two sentences, plus a link to the policy page.
- docs/KNOWN-ISSUES.md, "Windows and the installer": the row "The installer is not code-signed" becomes "New releases can meet a SmartScreen prompt until they gain reputation; the publisher shown is SignPath Foundation", with what Diarist does meanwhile (SHA-256 in the notes) and waiting on "download volume". The Smart App Control row about the NVIDIA runtime keeps waiting on "NVIDIA signing the runtime" only: its current "or Diarist being signed" is wrong, because signing Diarist does not change the verdict on `nemo-speech.exe` and SignPath forbids signing it. Add a row if the Smart App Control test (open question 6) finds the unsigned `uninstall.exe` or NSIS plugins blocked.
- CHANGELOG.md: "Windows: the installer and diarist.exe are signed (free code signing provided by SignPath.io, certificate by SignPath Foundation); the publisher shown is SignPath Foundation."
- docs/DEV.md, "Desktop app (Tauri)": the signed release path (this plan's section 3.6) next to the hand-built one.
- `.github/workflows/release.yml` keeps its unsigned release body ("The installer is not code-signed yet"); correct for what it builds. Its header comment's hand-built order still needs the alignment noted in plan 13 step 6.
- The site: the policy page's status line; the home page's Status section if it mentions signing.
- PRIVACY.md, now: the "Check for updates" row says `github.com` (the release's `latest.json`), but the endpoint is `https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json`, and the installer then comes from `github.com`. The policy page states it the second way.

## 6. Privacy statement for the policy page

As published on `/code-signing/`, consistent with PRIVACY.md (and with the correction above):

> This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.
>
> Diarist has no account, no analytics, no crash reporting and no telemetry. It connects to the internet only when you press a button that says so: Install engine, or a runtime (`github.com` and GitHub's download servers); Install or Download a model (`huggingface.co` and its download servers, through Windows' `curl.exe`); Add from URL (the address you typed); Check for newer runtime (`api.github.com`); Check for updates (GitHub: the update manifest on `raw.githubusercontent.com`, then the installer from `github.com` if you install it).
>
> Like any download, these requests show your IP address to the server; nothing about your meetings, your computer or your use of the app is sent. If WebView2 is missing, the installer downloads it from Microsoft. The services involved have their own privacy policies: GitHub, Hugging Face and Microsoft. The full statement is the privacy page.

The first sentence is the wording SignPath's terms offer; the rest names the third-party services whose policies affect users, which the terms also ask for.

## 7. Timeline

| When | What |
|---|---|
| 16 Oct 2026 | Prepared: this plan, the workflow, the site page on a local branch. |
| Next day or two (**owner**) | Section 3.1 GitHub settings; push the site branch and merge this plan and the workflow; apply (3.3). |
| 0.1.0-beta.2 | Hand-built and unsigned, by plan 13, whatever SignPath says. |
| 1 to 7 weeks after applying | SignPath's decision. Observed in 2026: a decline a week after the tracking issue was opened (docent); a first reply after a month and a decline after seven weeks (dictus-desktop). Expect "not yet" for a project this new (section 2). |
| 1 to 4 weeks after acceptance | Onboarding: organization, project, test-signing run (3.5), then SignPath imports the release certificate. |
| The release after beta.2, at the earliest | First signed release, only if acceptance and the release certificate arrive before it is cut. Otherwise beta.3 ships unsigned by the hand-built path, and signing starts with the first release after onboarding. |
| If declined | Reapply once there are outside references and download numbers over a few releases. The fallback is Certum's open-source certificate (publisher shown as the individual, identity check, yearly fee, no official CI support); it needs a different workflow and is not planned. |

## 8. Open questions

1. **Acceptance.** SignPath's reputation bar for programs is the most likely reason for a "not yet" (section 2). Nothing in the repository can change that.
2. **The form.** The live HubSpot form could not be read (it renders in the browser only); section 3.3 follows SignPath's earlier form, OSSRequestForm-v4.
3. **SCM policies for Foundation projects.** SignPath can require GitHub branch rulesets (`non_fast_forward`, `deletion`, `pull_request` and more) and forbid re-runs. Which ones it sets for Foundation projects is not documented. If it requires pull requests on `main`, every change becomes a pull request.
4. **Metadata check.** Whether SignPath reads Tauri's string ProductVersion (`0.1.0-beta.3`) for `product-version`, and the NSIS installer's version keys the same way. The first test-signing run shows it; if it fails, drop `product-version` and ask SignPath which attribute they expect.
5. **NVIDIA's CUDA libraries.** The runtime package the app downloads on request contains them under the CUDA EULA. They are not in the installer, but SignPath's "no proprietary code" condition could be read broadly. State it in the application (3.3).
6. **Smart App Control.** Whether, with it on, the signed installer runs with its unsigned NSIS plugin DLLs, and whether the unsigned `uninstall.exe` runs. Test on a PC with Smart App Control on after the first signed release.
7. **SmartScreen.** How much reputation the SignPath Foundation certificate gives a new project's first signed release. Seen only after it ships.
8. **Approval window.** Two hours per request (`wait-for-completion-timeout-in-seconds: 7200`, job limit 330 minutes). Shorten it if approvals are always quick; a missed window means a new run and two new approvals.
9. **MFA on SignPath.** SignPath's documentation does not say whether it enforces MFA itself; with Google sign-in, the Google account's 2-Step Verification is the control.
10. **Contact address.** The site and the repository have no email address, so the policy page sends reports to GitHub issues, GitHub's private vulnerability reporting (3.1) and SignPath's support address. Add an address to the page if one is wanted.
11. **Publisher string.** Tauri writes CompanyName `diarist` (lower case, from the identifier) into the exe. SignPath requires only the product name; `bundle.publisher: "Diarist"` in `tauri.conf.json` would tidy it, but changes nothing users see in the signature.

## 9. Sources (read 16 Oct 2026)

- SignPath Foundation conditions (terms): https://signpath.org/terms (source: https://github.com/SignPath/fdn-website/blob/main/docs/terms.md)
- Application page: https://signpath.org/apply; the earlier form's fields: https://github.com/SignPath/fdn-website/blob/main/docs/assets/OSSRequestForm-v4.xlsx; switch to HubSpot: https://github.com/SignPath/fdn-website/pull/24
- SignPath Foundation home and about: https://signpath.org/, https://signpath.org/about
- GitHub trusted build system, action inputs, policies: https://docs.signpath.io/trusted-build-systems/github
- The action (v3.0 of 10 Sep 2026, commit f6d04783b4569d051e0c80105fe66e82819d0092): https://github.com/signpath/github-action-submit-signing-request; changelog: https://docs.signpath.io/changelog/?component=github_actions_action
- Origin verification: https://docs.signpath.io/origin-verification
- Projects, signing policies, approvals: https://docs.signpath.io/projects
- Users, sign-in, CI users: https://docs.signpath.io/users
- Artifact configuration syntax and reference: https://docs.signpath.io/artifact-configuration/syntax, https://docs.signpath.io/artifact-configuration/reference
- actions/upload-artifact v7.0.1 (`archive`, `artifact-id`): https://github.com/actions/upload-artifact
- Tauri 2 Windows code signing (`certificateThumbprint`, `signCommand`): https://v2.tauri.app/distribute/sign/windows/
- Tauri CLI reference (`signer sign`, `bundle`, `--no-bundle`): https://v2.tauri.app/reference/cli/
- Tauri CLI 2.12.0 changelog (`--no-binary-patching`; `signer sign --app-version`; `TAURI_SIGNING_PRIVATE_KEY` must be the literal key for `signer sign`): https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-cli/CHANGELOG.md
- Binary patching breaks signatures: https://github.com/tauri-apps/tauri/issues/13868, https://github.com/tauri-apps/tauri/issues/15591 (maintainer: the patch is only needed with several installer types per platform)
- NSIS uninstaller signing in Tauri's template (`!uninstfinalize`): https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi
- Updater target lookup and signature check: https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/updater.rs
- SmartScreen reputation: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation
- Smart App Control: https://support.microsoft.com/en-us/topic/what-is-smart-app-control-285ea03d-fa88-4d56-882e-6698afdb7003
- Solo-maintainer policy page example: https://super-productivity.com/code-signing/
- Tauri projects signed through SignPath: WinScript https://github.com/flick9000/winscript/blob/main/.github/workflows/publish.yml; Seelen UI https://github.com/eythaann/Seelen-UI/blob/master/.github/workflows/release.yml; Mini Diarium https://github.com/fjrevoredo/mini-diarium/blob/master/docs/releasing/SIGNPATH_FIRST_TIME_SETUP.md
- Application outcomes: https://github.com/Arsarneq/docent/issues/72, https://github.com/getdictus/dictus-desktop/issues/38, https://github.com/Reid-n0rc/n1mm-scope-bridge/issues/130
