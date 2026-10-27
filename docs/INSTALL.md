# Installing Diarist

Version 0.1.0-beta.5 (a beta pre-release). For which GPU and which models, see "Choosing your package and
models" in the [README](../README.md#choosing-your-package-and-models).

## 1. Install

1. Download `Diarist_0.1.0-beta.5_x64-setup.exe` from the
   [releases page](https://github.com/ebt55/diarist/releases).
2. Run it. Windows SmartScreen warns because the installer is not code-signed
   yet: choose "More info", then "Run anyway". It installs for your user only,
   without administrator rights, into `%LOCALAPPDATA%\Diarist` (you can
   pick another folder). It adds a Start menu entry and, if you tick it, a
   desktop shortcut. If WebView2 is missing (rare on Windows 10 and 11), the
   installer downloads it from Microsoft.
3. Start Diarist.

The installer is about 7 MB. The speech engine and the models are not in
it; the app downloads them on first use.

## 2. First start: engine and models

A coding agent can do this step for you from a terminal, with hash checks:
see [SETUP-WITH-AN-AGENT.md](SETUP-WITH-AN-AGENT.md).

The setup screen checks:

- **GPU**: name, memory, driver and the CUDA version the driver supports. The
  CUDA runtime needs an NVIDIA driver with CUDA 13 (R580 or newer). With an
  AMD or Intel GPU the Vulkan runtime is preselected; without a GPU, the CPU
  runtime. You can change the choice. Vulkan and CPU were tested on an
  NVIDIA card and a desktop CPU only; the CPU runtime transcribes at about 5
  times real time ([KNOWN-ISSUES.md](KNOWN-ISSUES.md)).
- **Disk**: free space where the data folder is. The defaults need about
  1.4 GB while downloading and 1.3 GB after (measured with the CUDA runtime
  on 28 Oct 2026: 1.27 GB, because the unpacked runtime is larger than its
  download).

Then press **Install engine** (98 MB) and **Install recommended models**
(1.0 GB: the speaker model, the English speech model and the punctuation
model, which is seven files in one folder). Each item shows its
progress. An interrupted download resumes where it stopped: press **Retry**,
or install again later. Every file is checked against its published SHA-256
before it is used.

For other or mixed languages, add **Nemotron 3.5** in Settings, Models, and
select it there.

You do not have to finish the setup: **Set up later** opens the library, and
Import and Record each say what is still missing, with a button that gets it.

### If a download fails

Each item shows the address it comes from, its SHA-256, its size and the
folder it goes to, with **Retry**, **Copy link** and **Open folder**;
**Copy all links** copies every missing address. To fetch a file by hand:

1. Copy its link and download it with a browser.
2. Press Open folder on its row and move the downloaded file there (any
   subfolder of `models\` works for models; for the runtime, put the zip
   itself into `engine\`). The punctuation model's files go into the one
   folder its row names, `tokenizer.json` and the licence files at its top
   and `model_quantized.onnx` in its `onnx` subfolder.
3. Press **Check again** (setup screen) or **Refresh** (Settings). The app
   checks the hash, extracts a runtime zip, and marks the item installed.

### Firewalls

Per-app firewalls such as simplewall ask for permission the first time each
program connects. One program does all the downloading, `diarist.exe` (up to
0.1.0-beta.4 the models came through Windows' `curl.exe`, started by the
engine; since 0.1.0-beta.5 Diarist fetches them itself):

| Download | Program | Hosts it connects to |
|---|---|---|
| Speech engine (runtime zip) | `diarist.exe` | `github.com`, then GitHub's download servers (`objects.githubusercontent.com`, `release-assets.githubusercontent.com`) |
| Models | `diarist.exe` | `huggingface.co`, then its download servers (`*.hf.co`, for example `us.aws.cdn.hf.co`); for files Diarist hosts on its own releases page, `github.com` and GitHub's download servers as above |
| Check for newer runtime | `diarist.exe` | `api.github.com` |
| Check for updates | `diarist.exe` | `raw.githubusercontent.com` (the file `ebt55/diarist/main/updater/latest.json`); installing the update then fetches the installer from `github.com` and GitHub's download servers |

If the firewall prompt was missed or denied, allow `diarist.exe` and press
Retry; a partial download resumes. Or fetch the files by hand as above.
Diagnostics, "Why isn't it working?", names the program and the hosts when a
download failed for a network reason.

Some networks block certificate revocation checks. Up to 0.1.0-beta.4,
Windows' `curl.exe` then refused the model downloads
(`CRYPT_E_REVOCATION_OFFLINE`). Diarist's own downloader checks each
server's certificate against the Windows certificate store: it refuses a
server certificate that Windows knows to be revoked; a revocation server
that cannot be reached no longer stops a download. Only the server's own
certificate is checked for revocation, not the certificates that issued it.
Every download is https only, redirects included. Fetching a file by hand
always works.

### Offline machines

Copy the runtime zip and the `.gguf` files from a machine with internet
access (the addresses and hashes are on the setup screen and in
`manifests/` in the source), drop them in as above, and press Check again.

## 3. Using it

- **Ctrl+O** or drag files in: import a recording.
- **R**: record a meeting (microphone plus system audio). **Ctrl+.** stops
  (it asks first). **Ctrl+Alt+R** starts a recording from any app; pressed
  again it asks before stopping: the in-app question when the window is
  visible, otherwise press it a second time within 3 seconds.
- Without headphones the microphone also hears the meeting; Diarist notices
  that and removes the echoed words from "me" (Settings, "Recording with
  speakers"), though headphones still give the cleanest result.
- The window can be closed while recording; recording continues in the
  notification area.
- **Settings**: start with Windows (in the notification area), where audio is
  recorded from, how long audio is kept, the data folder, models and runtime.
- **Diagnostics** (Ctrl+Shift+D): "Why isn't it working?" and Copy
  diagnostics.

## 4. Low GPU memory and slow transcription

- Settings, "Low-VRAM mode": imports use Nemotron 3.5 when it is installed
  (peaks at about 1.8 GB instead of 2.6 GB).
- The live engine loads when you open the record sheet and unloads after 10
  minutes without recording (Settings, "Unload the live engine after").
- When GPU memory runs out, the NVIDIA driver may move work to system memory
  ("sysmem fallback"), which makes transcription many times slower instead of
  failing. To get an error instead: NVIDIA Control Panel, Manage 3D settings,
  Program Settings, Add, choose `nemo-speech.exe` (in the data folder,
  `engine\<runtime>\bin\`), set "CUDA - Sysmem Fallback Policy" to "Prefer No
  Sysmem Fallback", Apply.

## 5. Where things are

| What | Where |
|---|---|
| Program | `%LOCALAPPDATA%\Diarist` (chosen in the installer) |
| Data folder: database, recordings, engine, models, logs | `%LOCALAPPDATA%\app.diarist.desktop` |
| A moved data folder | stored in `HKCU\Software\Diarist\DataRoot` |
| Portable mode | create `portable.txt` next to `diarist.exe`: data goes to `data\` next to it (or the folder named on the file's first line) |

**Move data folder** (Settings, Storage) copies everything to an empty folder,
checks every file, restarts Diarist from there and then deletes the old
copy.

Recordings are kept as lossless FLAC for 30 days (Settings, "Keep audio for"),
then deleted unless the meeting is pinned (P); transcripts are never deleted.
Re-processing a meeting needs its audio.

## 6. Updating

Settings, About, **Check for updates** reads
`https://raw.githubusercontent.com/ebt55/diarist/main/updater/latest.json`
(the newest release, beta pre-releases included), downloads the installer it
names from GitHub and installs it after you confirm (updates are signed; the
app checks the signature). Your data folder is not touched. You can also run
a newer installer over the old one.

**Coming from meet-assist (now Diarist).** Diarist installs next to the
version with the earlier name rather than over it. Its first start offers
once to move that version's data (database, recordings, engine and models,
including a folder you had moved it to) into Diarist's data folder, or to
start fresh. Moving copies and checks everything, restarts, then deletes the
old copy; starting fresh leaves the old folder as it is. Afterwards uninstall
the old version (Windows Settings, Apps); its uninstaller never touches
Diarist's folder.

## 7. Uninstalling

Use Windows Settings, Apps, Diarist, Uninstall (or `uninstall.exe` in
the program folder).

- **"Delete the application data" unticked (default)**: the program is
  removed and your transcripts and recordings stay. The uninstaller asks
  whether to remove the downloaded engine and models (about 1.3 GB with the
  defaults); they can
  be downloaded again.
- **Ticked**: the data folder is removed too, including transcripts and
  recordings. For a moved data folder only Diarist's own items are
  removed (database, audio, engine, models, manifests, logs, the WebView
  profile), never other files you keep there.

Silent uninstall: `uninstall.exe /S` keeps everything; add `/DELETEAPPDATA`
to remove the data folder, or `/REMOVEMODELS` to remove only the engine and
models.
