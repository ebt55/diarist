# Updater manifest

`latest.json` is what installed copies of Diarist read when they check for
updates (`plugins.updater.endpoints` in `apps/desktop/src-tauri/tauri.conf.json`
points at this file on `main` through raw.githubusercontent.com, because
GitHub's "latest release" link skips pre-releases).

The release process updates it: a tag push `v*` runs
`.github/workflows/release.yml`, which builds the installer and its `.sig`,
attaches both to a draft pre-release and opens a pull request that rewrites
this file. Merge that pull request only after the draft is published.

A release built by hand goes the other way round: create the draft
pre-release with the installer and its `.sig` attached, publish the draft,
then commit this file on `main` with that `.sig`'s contents and the asset
URL. Draft assets are not public, so the file must never name a release
before it is published: every installed copy's check would fail. Publishing
pushes the tag; the workflow sees the installer already on the release and
builds nothing. If its guard job
fails with "Could not list releases", nothing was built; re-run the workflow.

The file must match the published asset: `version` is the release version,
`platforms.windows-x86_64.signature` is the exact contents of
`Diarist_<version>_x64-setup.exe.sig`, and `url` is that installer's download
address on the release (`https://github.com/ebt55/diarist/releases/download/v<version>/Diarist_<version>_x64-setup.exe`).
A wrong signature or a URL that does not answer makes every installed copy
report a failed update. Do not edit it by hand except to roll back to the
previous release's contents.
