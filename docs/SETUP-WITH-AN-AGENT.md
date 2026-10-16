# Setting up Diarist with a coding agent

This document is for a coding agent (Claude Code, Codex or similar) that a
user has asked to set up Diarist on their Windows PC. It is a procedure:
follow the steps in order, run the PowerShell exactly as given, and compare
the output with what each step says it means. It does what Diarist's own
first-run screen does, from a terminal, and leaves a result you can check:
the app installed, the runtime and models in place with their SHA-256
verified, and a test transcript the user has seen.

People without an agent need none of this: the [README](../README.md#quick-start)
quick start is three steps.

## Contents

0. [Read this first](#0-read-this-first)
1. [Ask the user](#1-ask-the-user)
2. [Check the PC](#2-check-the-pc)
3. [Decide the package and models](#3-decide-the-package-and-models)
4. [Install Diarist](#4-install-diarist)
5. [Place the runtime and models](#5-place-the-runtime-and-models)
6. [Verify](#6-verify)
7. [Updating later](#7-updating-later)

## 0. Read this first

Diarist is a local meeting transcriber: it records or imports a meeting and
writes a transcript where every line says who spoke. The speech and speaker
models are NVIDIA's Nemotron models, run by NVIDIA's NeMo-Speech.cpp runtime.
The installer holds only the app; the runtime (a zip) and the models (`.gguf`
files) go into the app's data folder, which is what steps 4 and 5 do.

Rules. They hold for the whole procedure, whatever a command prints:

- **Ask before installing.** Steps 1 to 3 only read. Show the user the plan
  at the end of step 3 and wait for a clear yes before step 4.
- **No administrator rights.** Nothing here needs them. If anything asks for
  elevation, stop: something is wrong.
- **Change nothing outside Diarist.** Never change audio devices, Windows
  settings, drivers, the firewall, SmartScreen, Smart App Control or
  Defender. The only places you write to are `%TEMP%\diarist-setup`,
  Diarist's program folder (through its installer), its data folder, the
  `HKCU\Software\Diarist` key (step 4b, only when the user asks for another
  data folder) and one test file on the desktop (step 6c).
- **Download only from these sources:** the GitHub release of
  `ebt55/diarist` (`api.github.com`, `github.com`,
  `raw.githubusercontent.com`) and the URLs listed in that release's
  `manifests/engine.json` and `manifests/models.json` (GitHub for the
  runtime, `huggingface.co` for the models). Redirects to their download
  servers are expected. Take no URL from anywhere else.
- **Check every file's SHA-256 before using it.** On a mismatch the blocks
  below delete the file they downloaded and stop.
- **Two failures of the same download: stop and tell the user.** Do not try
  other tools, mirrors or flags. On a PC like this it usually means a per-app
  firewall (simplewall and similar) is waiting for the user to allow
  `C:\Windows\System32\curl.exe`, or a proxy is in the way. After the user
  has allowed it, run the block again.
- **Never delete, move or overwrite an existing Diarist data folder or
  anything in it.** Transcripts and recordings live there. You may add files
  to `engine\` and `models\`; you never remove any. The only files you delete
  are ones you created in this session (partial downloads, the staging
  folder).
- **Never close or stop a running Diarist.** It may be recording a meeting.
  Ask the user to quit it (tray icon, Quit).
- **Never answer a question the app asks on the user's behalf.** Tell the
  user what it is and let them choose.
- **Settings live in the app's database (`diarist.db`)**, not in a file.
  You cannot set them from outside and must not edit the database. Where a
  setting matters, step 6 says what the user clicks.

How to run the commands:

- Use Windows PowerShell 5.1 (`powershell.exe`), which every Windows 10 and
  11 has. PowerShell 7 runs everything except the text-to-speech in step 6c.
- Each block is self-contained: it sets the variables it needs at the top,
  so it works when your shell keeps no state between commands. Blocks pass
  results to later blocks through files in `%TEMP%\diarist-setup`. Where a
  block starts with lines marked `# from step N`, put in the value that
  step gave you.
- Downloads use Windows' own `C:\Windows\System32\curl.exe`, called by its
  full path. Plain `curl` in Windows PowerShell is an alias for
  `Invoke-WebRequest`, and another `curl.exe` earlier on the PATH would
  trigger a different firewall prompt than the one the app's downloads use.
- The model downloads are 100 to 750 MB each. Give those commands a long
  timeout (10 minutes or more), or run them in the background and wait.

## 1. Ask the user

Ask these three questions together, and wait for the answers:

1. **"Are your meetings in English only, or also in other languages?"**
   This picks the speech model (step 3).
2. **"Do you use headphones for calls?"** Diarist records the microphone and
   the system audio separately. With loudspeakers the microphone also hears
   the other people, and their words can end up under the user's name.
   Recommend headphones for live recording for now. The answer changes
   nothing in the setup; do not change any audio setting.
3. **"Diarist keeps its data (transcripts, recordings, the runtime and the
   models) in `%LOCALAPPDATA%\app.diarist.desktop`. Is that fine, or do you
   want it in another folder, for example on another drive?"** Plan about
   2 GB for the runtime and models plus about 90 MB per recorded hour
   (recordings are kept 30 days by default). Another folder is set in step 4b.

## 2. Check the PC

Run this block. It only reads.

```powershell
# Windows and memory
"Windows version: " + [System.Environment]::OSVersion.Version
"64-bit Windows: " + [System.Environment]::Is64BitOperatingSystem
"RAM: {0:N1} GB" -f ((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)

# NVIDIA GPU: name, VRAM, driver, compute capability, CUDA (needs an NVIDIA driver)
$smi = Get-Command nvidia-smi.exe -ErrorAction SilentlyContinue
if ($smi) {
    & $smi.Source --query-gpu=name,memory.total,driver_version --format=csv,noheader
    & $smi.Source --query-gpu=compute_cap --format=csv,noheader
    & $smi.Source | Select-String 'CUDA.*Version'
} else {
    "nvidia-smi: not found"
}

# Every display adapter, any vendor, and its dedicated memory
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, AdapterRAM | Format-Table -AutoSize
Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*' -ErrorAction SilentlyContinue |
    Select-Object DriverDesc, @{ n = 'VRAM_GB'; e = { [math]::Round($_.'HardwareInformation.qwMemorySize' / 1GB, 1) } } |
    Format-Table -AutoSize

# Free space per drive
Get-PSDrive -PSProvider FileSystem | Select-Object Name, Root, @{ n = 'FreeGB'; e = { [math]::Round($_.Free / 1GB, 1) } } | Format-Table -AutoSize

# What could block an unsigned program, and the downloader
$sac = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' -Name VerifiedAndReputablePolicyState -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
"Smart App Control: " + $(switch ($sac) { 0 { 'off' } 1 { 'on' } 2 { 'evaluation' } default { 'not present' } })
"curl.exe present: " + (Test-Path "$env:SystemRoot\System32\curl.exe")
```

Then this one, which looks for an existing Diarist and its data folder the
way the app finds it (portable marker, then the registry, then the default):

```powershell
$loc = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Diarist' -ErrorAction SilentlyContinue).InstallLocation
$app = if ($loc) { $loc.Trim('"') } else { "$env:LOCALAPPDATA\Diarist" }
$exe = Join-Path $app 'diarist.exe'
"Program folder: $app"
"Diarist installed: " + (Test-Path $exe)
if (Test-Path $exe) { "Installed version: " + (Get-Item $exe).VersionInfo.ProductVersion }
"Diarist running: " + [bool](Get-Process diarist -ErrorAction SilentlyContinue)

$marker = Join-Path $app 'portable.txt'
$reg = (Get-ItemProperty 'HKCU:\Software\Diarist' -Name DataRoot -ErrorAction SilentlyContinue).DataRoot
if (Test-Path $marker) {
    $line = Get-Content $marker | Where-Object { $_.Trim() } | Select-Object -First 1
    $data = if (-not $line) { Join-Path $app 'data' }
            elseif ([System.IO.Path]::IsPathRooted($line.Trim())) { $line.Trim() }
            else { Join-Path $app $line.Trim() }
    "Data folder (portable mode): $data"
} elseif ($reg) {
    $data = $reg
    "Data folder (moved, HKCU\Software\Diarist\DataRoot): $data"
} else {
    $data = "$env:LOCALAPPDATA\app.diarist.desktop"
    "Data folder (default): $data"
}
"Data folder exists: " + (Test-Path $data)
"Transcripts database exists: " + (Test-Path (Join-Path $data 'diarist.db'))
Get-ChildItem (Join-Path $data 'engine'), (Join-Path $data 'models') -Recurse -Include nemo-speech.exe, *.gguf, *.onnx -ErrorAction SilentlyContinue |
    ForEach-Object { '{0,11}  {1}' -f $_.Length, $_.FullName }

# Data from Diarist's earlier name, meet-assist
$old = (Get-ItemProperty 'HKCU:\Software\meet-assist' -Name DataRoot -ErrorAction SilentlyContinue).DataRoot
if (-not $old) { $old = "$env:LOCALAPPDATA\app.meetassist.desktop" }
"meet-assist data: " + (Test-Path (Join-Path $old 'meet-assist.db'))
```

What the output means:

| Line | Meaning |
|---|---|
| `Windows version: 10.0.<build>` | The build (third number) must be 17134 or higher (Windows 10 1803, or any Windows 11). Lower: stop, Diarist does not run there. `64-bit Windows` must be `True`. |
| `nvidia-smi` line, e.g. `NVIDIA GeForce RTX 3080, 10240 MiB, 610.88` | GPU name, VRAM, driver version. A driver of 580 or higher supports CUDA 13, which the CUDA package needs. |
| Compute capability, e.g. `8.6` | The CUDA package has kernels for Turing (7.5), Ampere (8.6), Ada (8.9) and Blackwell (12.0). Below 7.5 (GTX 10 series and older) use Vulkan. An error here instead of a number means a driver too old to answer: Vulkan. |
| `CUDA Version: 13.0` or `CUDA UMD Version: 13.3` | The highest CUDA version the driver supports (newer drivers print the second form). 13.0 or higher confirms the CUDA package. |
| `nvidia-smi: not found` | No NVIDIA driver. Use the adapter table. |
| Adapter table | Any AMD or Intel adapter, or an NVIDIA one without a working driver, means Vulkan. Only "Microsoft Basic Display Adapter" or nothing means no usable GPU: CPU. `AdapterRAM` is a 32-bit field and stops at 4 GB (4293918720), so read VRAM from the `VRAM_GB` table (or nvidia-smi) instead. |
| `FreeGB` | The drive of the data folder needs about 2 GB free, plus about 90 MB per recorded hour. The setup itself also puts about 7 MB into `%TEMP%`. |
| `Smart App Control: on` | Windows may block the unsigned installer and the unsigned NVIDIA runtime. Tell the user before going on; do not change the setting. `off`, `evaluation` and `not present` are fine. |
| `curl.exe present: False` | Windows older than 1803; see the first row. |
| `Program folder` | `%LOCALAPPDATA%\Diarist` unless the user picked another folder in the installer; an update installs into the same folder. |
| `Diarist installed: True` | Step 4 updates it in place; the data folder is kept. |
| `Diarist running: True` | Ask the user to quit it (tray icon, Quit) before step 4. Do not stop it yourself. |
| `Transcripts database exists: True` | The user already has Diarist data. Keep it: do not set another data folder in step 4b (the app's Settings, Storage, Move… does that safely). Step 5 skips files that are already there. |
| Engine and model paths listed | Already installed. Step 5 checks them and skips them. |
| `meet-assist data: True` | The user ran Diarist under its earlier name. At its first start Diarist asks once whether to move that data in, and the move needs empty `engine\` and `models\` folders. Do step 4, skip step 5, start the app and let the user answer; afterwards run step 5 for whatever is still missing. |

## 3. Decide the package and models

The runtime package, from step 2:

| What step 2 showed | Package | Why |
|---|---|---|
| NVIDIA GPU, compute capability 7.5 or higher, driver 580 or newer (CUDA 13.0 or higher) | `cuda` | The tested package; measured on an RTX 3080, about 30 times faster than real time. |
| AMD or Intel GPU, an NVIDIA GPU with an older driver, or compute capability below 7.5 | `vulkan` | Runs on any current GPU driver; tested by Diarist on an NVIDIA card only (about 25 times faster than real time there), not on AMD or Intel. With an older NVIDIA driver, the user may prefer to update it from nvidia.com (they do that, not you) and use `cuda`. |
| No usable GPU | `cpu` | Works, about 5 times faster than real time on a 12-core desktop CPU; live recording is not recommended. |

The models. Always `nemotron-3-diarization` (who spoke when, up to 8
speakers). The speech model, from question 1 and the VRAM:

| Situation | Install | In the app afterwards (step 6b) |
|---|---|---|
| English only, 6 GB VRAM or more | `nemotron-3-diarization`, `nemotron-en` | Nothing: both are selected automatically. |
| English only, less than 6 GB VRAM | `nemotron-3-diarization`, `nemotron-en`, `nemotron-3.5` | Turn on Low-VRAM mode: imports then use Nemotron 3.5, live recording keeps the English model. |
| Other or mixed languages | `nemotron-3-diarization`, `nemotron-3.5` | Nothing: Nemotron 3.5 is selected when it is the only speech model. |
| Both English-only and other meetings | all three | The English model is selected by default; the user switches in Settings, Models, Speech to text. |
| CPU package | as by language above | Expect about 12 minutes per hour of audio on a fast desktop CPU, more on a laptop. |

Never install `parakeet-tdt`. It is experimental: it punctuates, but it
silently skipped a 46-second span of a 77-minute recording.

The VRAM figures behind the table, measured on an RTX 3080 10 GB, not
counting the desktop's own use (about 1.5 GB):

| Scenario | VRAM |
|---|---|
| Live recording, two channels, Nemotron 3.5 and the diarizer | 1.05 GB idle, 1.26 GB with two sessions, spikes of 0.5 to 0.6 GB per session; budget 2 GB (the English model sits at 1.18 GB) |
| File transcription, Nemotron 3.5 | 1.82 GB peak (77-minute file) |
| File transcription, English model | 2.6 GB peak |
| Diarization pass alone | 0.7 to 0.85 GB |
| Experimental Parakeet | 4.9 GB peak |

Estimated, not measured: a 4 GB card handles file transcription with
Nemotron 3.5 (with Low-VRAM mode on), and live recording is borderline next
to the desktop's own use; 6 GB runs live and file mode comfortably; 8 GB and
up runs everything.

Now tell the user the plan and wait for a yes. For example:

> I'll install Diarist 0.1.0-beta.2 (7 MB, per user, no administrator
> rights) into `%LOCALAPPDATA%\Diarist`, then download the CUDA runtime
> (98 MB, from NVIDIA's GitHub releases) and two models from Hugging Face,
> Nemotron 3 Diarization (102 MB) and Nemotron Speech Streaming EN 0.6B
> (668 MB), into `%LOCALAPPDATA%\app.diarist.desktop`. Every file is checked
> against the SHA-256 Diarist publishes. Nothing else on the PC changes.
> Go ahead?

Take the version from step 4a's output and the sizes from step 5a's if you
want them exact; the numbers above are for 0.1.0-beta.2.

## 4. Install Diarist

### 4a. Find the newest release and download the installer

Diarist releases are pre-releases for now, and the GitHub API's
`releases/latest` skips those, so this reads the full list. The installer's
SHA-256 is in the release notes; GitHub also publishes its own digest of
each asset. Both must agree.

```powershell
$ErrorActionPreference = 'Stop'
$stage = "$env:TEMP\diarist-setup"
New-Item -ItemType Directory -Force $stage | Out-Null
$curl = "$env:SystemRoot\System32\curl.exe"

$all = Invoke-RestMethod -Uri 'https://api.github.com/repos/ebt55/diarist/releases' -Headers @{ 'User-Agent' = 'diarist-agent-setup' }
$rel = $all | Where-Object { -not $_.draft } | Sort-Object { [datetime]$_.published_at } -Descending | Select-Object -First 1
$asset = $rel.assets | Where-Object { $_.name -match '^Diarist_.+_x64-setup\.exe$' } | Select-Object -First 1
if (-not $asset) { throw 'The newest release has no Diarist_*_x64-setup.exe. Stop and tell the user.' }

$fromNotes = [regex]::Match("$($rel.body)", 'SHA-256 of\W+' + [regex]::Escape($asset.name) + '\W+([0-9a-fA-F]{64})').Groups[1].Value.ToLower()
$fromGitHub = ("$($asset.digest)" -replace '^sha256:', '').ToLower()
if (-not $fromNotes -and -not $fromGitHub) { throw 'No SHA-256 for the installer. Stop and tell the user.' }
if ($fromNotes -and $fromGitHub -and $fromNotes -ne $fromGitHub) { throw "Release notes say $fromNotes, GitHub says $fromGitHub. Stop and tell the user." }
$sha = if ($fromNotes) { $fromNotes } else { $fromGitHub }

$installer = Join-Path $stage $asset.name
& $curl -L --fail --retry 3 -sS -o "$installer.part" $asset.browser_download_url
if ($LASTEXITCODE -ne 0) { throw "Download failed (curl exit code $LASTEXITCODE)." }
$got = (Get-FileHash "$installer.part" -Algorithm SHA256).Hash.ToLower()
if ($got -ne $sha) { Remove-Item "$installer.part"; throw "SHA-256 mismatch: got $got, expected $sha. The file was deleted." }
Move-Item "$installer.part" $installer -Force
Unblock-File $installer

[ordered]@{ version = $rel.tag_name.TrimStart('v'); tag = $rel.tag_name; installer = $installer; sha256 = $sha } |
    ConvertTo-Json | Set-Content "$stage\release.json"
"Release notes SHA-256: $fromNotes"
"GitHub SHA-256:        $fromGitHub"
"Downloaded SHA-256:    $got"
Get-Content "$stage\release.json"
```

The last lines must show three equal hashes (or two, when one source is
missing) and the release's version and tag. The installer is not
code-signed: `Unblock-File` removes the "downloaded from the internet" mark
so SmartScreen does not stop the silent run (curl does not set that mark,
but a copy the user fetched with a browser has it; the hash check is what
makes running it safe).

If the API answers `403` (it allows 60 requests an hour without a login),
wait and try again later. The app's own update check reads the newest
version from
`https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json`;
that file has the installer's URL but no SHA-256, so it is no substitute for
the hash.

### 4b. Only if the user wants another data folder

Skip this unless the user answered question 3 with another folder **and**
step 2 showed `Transcripts database exists: False`. It points Diarist at the
new folder before its first start, the same registry value the app's own
"Move data folder" writes.

```powershell
$ErrorActionPreference = 'Stop'
$folder = 'D:\Diarist'   # the folder the user named: new or empty
if (Test-Path "$env:LOCALAPPDATA\app.diarist.desktop\diarist.db") { throw 'Diarist already has data in the default folder. Use Settings, Storage, Move... in the app instead.' }
if ((Test-Path $folder) -and (Get-ChildItem $folder -Force | Select-Object -First 1)) { throw "$folder is not empty. Ask the user for a new or empty folder." }
New-Item -ItemType Directory -Force $folder | Out-Null
if (-not (Test-Path 'HKCU:\Software\Diarist')) { New-Item -Path 'HKCU:\Software\Diarist' | Out-Null }
Set-ItemProperty -Path 'HKCU:\Software\Diarist' -Name DataRoot -Value $folder
"Data folder: " + (Get-ItemProperty 'HKCU:\Software\Diarist').DataRoot
```

From here on, the data folder is this folder. The installer later adds a
subkey `HKCU\Software\Diarist\Diarist` (its own install path); leave both
alone.

### 4c. Run the installer

The installer is an NSIS setup that installs per user into
`%LOCALAPPDATA%\Diarist` without administrator rights; `/S` runs it
silently. Run over an existing install, it updates the program in its folder
and keeps the data folder.

```powershell
$ErrorActionPreference = 'Stop'
$stage = "$env:TEMP\diarist-setup"
$rel = Get-Content "$stage\release.json" -Raw | ConvertFrom-Json
if ((Get-FileHash $rel.installer -Algorithm SHA256).Hash.ToLower() -ne $rel.sha256) { throw 'The installer changed since it was checked. Run 4a again.' }
if (Get-Process diarist -ErrorAction SilentlyContinue) { throw 'Diarist is running. Ask the user to quit it (tray icon, Quit), then run this again.' }
$p = Start-Process -FilePath $rel.installer -ArgumentList '/S' -Wait -PassThru
"Installer exit code: $($p.ExitCode)"
$loc = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Diarist' -ErrorAction SilentlyContinue).InstallLocation
$app = if ($loc) { $loc.Trim('"') } else { "$env:LOCALAPPDATA\Diarist" }
$exe = Join-Path $app 'diarist.exe'
"diarist.exe present: " + (Test-Path $exe) + " ($exe)"
if (Test-Path $exe) { "Version: " + (Get-Item $exe).VersionInfo.ProductVersion }
```

Expected: exit code `0`, `diarist.exe present: True`, and the release's
version. The program folder holds `diarist.exe` and `uninstall.exe`. The
installer does not start the app. If WebView2 is missing (rare on Windows
10 and 11), the installer fetches it from Microsoft. The installer contains
only the app; as of 0.1.0-beta.2 there is no command-line tool to call.

## 5. Place the runtime and models

The app can do all of this itself: its first-run screen downloads the same
files from the same addresses and checks the same hashes. This step saves the
user the clicks and gives you a result you can verify before the app
starts. The runtime goes into `engine\<package name>\` (the folder holding
`bin\nemo-speech.exe`), each model into
`models\<repo>\<revision>\<file>`, the same folder the app's own download
uses. The app recognises a model anywhere under `models\` by its file name,
size and SHA-256, so the folder is for tidiness; the hash is what counts.

### 5a. Fetch the manifests of this release

The URLs, sizes and hashes come from the manifests at the release's tag, so
they always match the installed version.

```powershell
$ErrorActionPreference = 'Stop'
$stage = "$env:TEMP\diarist-setup"
$rel = Get-Content "$stage\release.json" -Raw | ConvertFrom-Json
$curl = "$env:SystemRoot\System32\curl.exe"
foreach ($f in 'engine.json', 'models.json') {
    & $curl -L --fail --retry 3 -sS -o "$stage\$f" "https://raw.githubusercontent.com/ebt55/diarist/$($rel.tag)/manifests/$f"
    if ($LASTEXITCODE -ne 0) { throw "Download of $f failed (curl exit code $LASTEXITCODE)." }
}
$engines = Get-Content "$stage\engine.json" -Raw | ConvertFrom-Json
$models = Get-Content "$stage\models.json" -Raw | ConvertFrom-Json
$engines | Select-Object name, backend, version, tested, @{ n = 'MB'; e = { [math]::Ceiling($_.size_bytes / 1MB - 0.1) } } | Format-Table -AutoSize
$models | Select-Object name, role, required, experimental, @{ n = 'MB'; e = { [math]::Ceiling($_.size_bytes / 1MB - 0.1) } } | Format-Table -AutoSize
```

The two tables list every runtime package and model with its size, rounded
as the app shows it. For 0.1.0-beta.2: `cuda` 98, `vulkan` 19, `cpu` 6;
`nemotron-3-diarization` 102, `nemotron-en` 668, `nemotron-3.5` 708,
`parakeet-tdt` 681. The names in the `name` column are the ones step 3 uses.

### 5b. The runtime

Set the data folder (from step 2, or the folder from step 4b) and the
package from step 3, then run:

```powershell
$ErrorActionPreference = 'Stop'
$data = "$env:LOCALAPPDATA\app.diarist.desktop"   # from step 2 (or 4b)
$backend = 'cuda'                                 # from step 3: cuda, vulkan or cpu

$stage = "$env:TEMP\diarist-setup"
$curl = "$env:SystemRoot\System32\curl.exe"
$engines = Get-Content "$stage\engine.json" -Raw | ConvertFrom-Json
$e = $engines | Where-Object { $_.backend -eq $backend } |
    Sort-Object @{ e = { [bool]$_.tested }; Descending = $true }, @{ e = { [version]$_.version }; Descending = $true } |
    Select-Object -First 1
if (-not $e) { throw "No $backend package in the manifest." }
$root = Join-Path $data 'engine'
$target = Join-Path $root $e.name
if (Test-Path "$target\bin\nemo-speech.exe") {
    "Already installed: $target"
} else {
    New-Item -ItemType Directory -Force "$root\downloads" | Out-Null
    $zip = "$root\downloads\$($e.name).zip"
    & $curl -L --fail --retry 3 -sS -o "$zip.part" $e.url
    if ($LASTEXITCODE -ne 0) { throw "Download failed (curl exit code $LASTEXITCODE): $($e.url)" }
    $got = (Get-FileHash "$zip.part" -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $e.sha256 -or (Get-Item "$zip.part").Length -ne $e.size_bytes) {
        Remove-Item "$zip.part"
        throw "SHA-256 mismatch for $($e.name): got $got, expected $($e.sha256). The file was deleted."
    }
    Move-Item "$zip.part" $zip -Force
    "SHA-256 OK: $($e.name).zip"
    $tmp = "$target.agent-tmp"
    if (Test-Path $tmp) { throw "$tmp is left over from an earlier attempt of yours; delete it and run this again." }
    Expand-Archive -LiteralPath $zip -DestinationPath $tmp
    $inner = if (Test-Path "$tmp\bin\nemo-speech.exe") { $tmp } else {
        Get-ChildItem $tmp -Directory | Where-Object { Test-Path "$($_.FullName)\bin\nemo-speech.exe" } |
            Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $inner) { throw "The zip has no bin\nemo-speech.exe; left as it is in $tmp." }
    Move-Item -LiteralPath $inner -Destination $target
    if (Test-Path $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
    Remove-Item -LiteralPath $zip
    if (-not (Get-ChildItem "$root\downloads")) { Remove-Item "$root\downloads" }
    "Installed: $target"
}
& "$target\bin\nemo-speech.exe" --json doctor
```

The last command asks the runtime about this PC. In its JSON, `version` is
the runtime version (`0.2.0` for 0.1.0-beta.2). For the CUDA package,
`driver_runtime_compatible` must be `true` and `devices` must list the GPU
with `"type": "gpu"`; if not, the driver is too old or the GPU is not
supported, and the Vulkan package is the fallback. If Windows refuses to run
`nemo-speech.exe` at all, Smart App Control or an antivirus blocked it: tell
the user.

### 5c. The models

Set the data folder and the model names from step 3, then run. Each model
takes a minute or more; run one name at a time if your command timeout is
short.

```powershell
$ErrorActionPreference = 'Stop'
$data = "$env:LOCALAPPDATA\app.diarist.desktop"            # from step 2 (or 4b)
$want = @('nemotron-3-diarization', 'nemotron-en')         # from step 3

$stage = "$env:TEMP\diarist-setup"
$curl = "$env:SystemRoot\System32\curl.exe"
$models = Get-Content "$stage\models.json" -Raw | ConvertFrom-Json
foreach ($name in $want) {
    $m = $models | Where-Object { $_.name -eq $name }
    if (-not $m) { throw "No model named $name in the manifest." }
    $dir = Join-Path (Join-Path $data 'models') ((@($m.repo -split '/') + $m.revision) -join '\')
    $file = Join-Path $dir $m.file
    if (Test-Path $file) {
        $have = (Get-FileHash $file -Algorithm SHA256).Hash.ToLower()
        if ($have -eq $m.sha256) { "Already there, SHA-256 OK: $file"; continue }
        throw "$file exists with SHA-256 $have instead of $($m.sha256). Not touched; tell the user."
    }
    New-Item -ItemType Directory -Force $dir | Out-Null
    & $curl -L --fail --retry 3 -sS -o "$file.part" $m.url
    if ($LASTEXITCODE -ne 0) { throw "Download failed (curl exit code $LASTEXITCODE): $($m.url)" }
    $got = (Get-FileHash "$file.part" -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $m.sha256 -or (Get-Item "$file.part").Length -ne $m.size_bytes) {
        Remove-Item "$file.part"
        throw "SHA-256 mismatch for ${name}: got $got, expected $($m.sha256). The file was deleted."
    }
    Move-Item "$file.part" $file
    "SHA-256 OK: $file"
}
```

Every model must end with `SHA-256 OK` or `Already there, SHA-256 OK`. The
app checks each file again at its first start (it hashes a file placed by
hand and writes the result next to it as `<file>.sha256`) and uses it as the
recommended model only when the hash matches.

### 5d. What is in place

```powershell
$data = "$env:LOCALAPPDATA\app.diarist.desktop"   # from step 2 (or 4b)
Get-ChildItem (Join-Path $data 'engine'), (Join-Path $data 'models') -Recurse -Include nemo-speech.exe, *.gguf |
    ForEach-Object { '{0,11}  {1}' -f $_.Length, $_.FullName.Substring($data.Length + 1) }
```

One `engine\<package>\bin\nemo-speech.exe` per installed package and one
`models\nvidia\...\<file>.gguf` per model from step 3, with the size in
bytes from the manifest (for example
`107012128  models\nvidia\Nemotron-3-Diarization\f667...\Nemotron-3-Diarization.q8_0.gguf`).

## 6. Verify

### 6a. Start the app

```powershell
$loc = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Diarist' -ErrorAction SilentlyContinue).InstallLocation
$app = if ($loc) { $loc.Trim('"') } else { "$env:LOCALAPPDATA\Diarist" }
Start-Process (Join-Path $app 'diarist.exe')
```

Wait about 15 seconds, then read the end of the app log:

```powershell
$data = "$env:LOCALAPPDATA\app.diarist.desktop"   # from step 2 (or 4b)
Get-Content (Join-Path $data 'logs\diarist.log') -Tail 40
Get-ChildItem (Join-Path $data 'models') -Recurse -Filter *.sha256 | Select-Object Name
```

The newest start line reads `Diarist <version> started; data root <folder>
(Default)`, or `(Registry)` after step 4b; the folder must be the one you
placed the files in. A `.sha256` file next to each model means the app has
hashed it at its first scan. A log line `model scan: ... not used as ...`
means a file has the right name and size but other bytes; tell the user. If
the app shows a dialog "Move your existing meet-assist data to Diarist?",
that is the user's decision (see step 2); do not answer it.

### 6b. What the user should see, and what to click

Tell the user what to look at; you cannot click for them.

- With everything in place the app opens on the library, "Your meetings,
  searchable", **without** the line "The speech engine and models are not
  installed yet" and its **Set up** button. If that line is there, something
  was not recognised: the **Set up** button shows which row is still "Not
  installed".
- **Diagnostics** (the page icon next to the gear, or Ctrl+Shift+D), **Why
  isn't it working?**: every row reads **pass** except **Live engine**,
  which reads **skip** until the first recording. The **Speech-to-text
  model** and **Speaker model** rows say "SHA-256 matches the published
  one", and **Speech engine** says the runtime "answered doctor". This is
  the quickest complete check.
- **Settings** (gear icon, or Ctrl+,), section **Models**: each model placed
  in step 5c shows **✓ Installed**. Under **Speech to text** and **Speaker
  diarization** the files are listed by file name with the chip
  **recommended**, which means the SHA-256 matched Diarist's list (a file
  with the right name but other bytes would show **added by you** instead).
  The **Engine** group lists the runtime folder with **version 0.2.0** and
  the chip **tested**, and its package row shows **✓ Installed**.
- **Open the first-run screen** (a link under the model list) shows the
  setup screen: the GPU with its VRAM, driver and CUDA version, the free disk
  space, **1 Speech engine** and **2 Speech and speaker models** with each
  row **✓ Installed**, and **✓ Ready.** For "other languages" setups the
  English model's row reads "Not installed"; that is expected, and
  **✓ Ready.** still shows.

Then, as step 3 decided:

- **Less than 6 GB VRAM, English:** Settings, section **Startup and engine**,
  tick **Low-VRAM mode**.
- **Both English and other languages:** Settings, **Models**, **Speech to
  text**: pick Nemotron 3.5 for other-language meetings (and back for
  English ones).
- **Vulkan or CPU package** next to a CUDA one, or a choice to change: the
  **Engine** group in Settings, Models has one radio button per runtime.

### 6c. Smoke test

This writes a test recording of about 15 seconds with Windows' built-in
text to speech (Windows PowerShell 5.1 only; PowerShell 7 lacks
`System.Speech`):

```powershell
Add-Type -AssemblyName System.Speech
$wav = Join-Path ([Environment]::GetFolderPath('Desktop')) 'diarist-test.wav'
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$en = $s.GetInstalledVoices() | Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -like 'en-*' } | Select-Object -First 1
if ($en) { $s.SelectVoice($en.VoiceInfo.Name) }
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$s.SetOutputToWaveFile($wav, $fmt)
$s.Speak('This is a test of Diarist on this computer. The weekly meeting starts at ten in the morning. We will talk about the budget, the new office and the plan for next year. Please send your notes before Friday. Thank you all for coming.')
$s.Dispose()
"Wrote $wav, about {0:N0} seconds, voice: {1}" -f (((Get-Item $wav).Length - 44) / 32000), $(if ($en) { $en.VoiceInfo.Name } else { 'default' })
```

The last line should say about 16 seconds and name an English voice
(`Microsoft David Desktop` or `Microsoft Zira Desktop` on most PCs).

Ask the user to press **Ctrl+O** in Diarist (the dialog is titled "Import
recordings") and pick `diarist-test.wav` on their desktop, or drag the file
onto the window. A correct result: a meeting called "diarist-test" appears
in the library as "16 s, 1 speaker", within a few seconds with the CUDA
package (longer with Vulkan or CPU; the app log says `import done: ... in
<seconds> s`). Opened, it shows one speaker, "Speaker 1", and the text above
split into about five lines at the pauses, near word for word ("Diarist"
may come out as "diarist"). On real meetings the Nemotron models punctuate
much less than on this clean synthetic voice. With no English voice
installed the test speaks another language, and only Nemotron 3.5
transcribes that.

The user can delete the meeting (Delete key) and the WAV afterwards. You may
delete `%TEMP%\diarist-setup`, which only holds the installer and the
manifests you downloaded.

### If anything is off

Ask the user to open **Diagnostics** (Ctrl+Shift+D). **Why isn't it
working?** lists every check with a fix button on each failing row;
**Check again** re-runs them. For a bug report, **Copy diagnostics** copies
a text report to paste into an issue at
<https://github.com/ebt55/diarist/issues>, together with what you ran and
what it printed. The app log is `logs\diarist.log` in the data folder.

## 7. Updating later

In the app: Settings, section **About**, **Check for updates**, then
**Install and restart**. Updates are signed; the app checks the signature,
and the data folder is not touched.

From a terminal: run steps 4a and 4c again (the new installer updates the
old install in place), then 5a, 5b and 5c with the same choices. Files whose
hashes still match are skipped; a new runtime or model version lands in its
own folder next to the old one. Never remove the old ones yourself: the user
can delete them once the new ones work.
