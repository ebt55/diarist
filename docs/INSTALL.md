# Installing meet-assist

Version 0.1.0. For which GPU and which models, see "Choosing your package and
models" in the [README](../README.md#choosing-your-package-and-models).

## 1. Install

1. Download `meet-assist_0.1.0_x64-setup.exe` from the releases page.
2. Run it. Windows SmartScreen warns because the installer is not code-signed
   yet: choose "More info", then "Run anyway". It installs for your user only,
   without administrator rights, into `%LOCALAPPDATA%\meet-assist` (you can
   pick another folder). It adds a Start menu entry and, if you tick it, a
   desktop shortcut. If WebView2 is missing (rare on Windows 10 and 11), the
   installer downloads it from Microsoft.
3. Start meet-assist.

The installer is about 10 MB. The speech engine and the models are not in
it; the app downloads them on first use.

## 2. First start: engine and models

The setup screen checks:

- **GPU**: name, memory, driver and the CUDA version the driver supports. The
  CUDA runtime needs an NVIDIA driver with CUDA 13 (R580 or newer). With an
  AMD or Intel GPU the Vulkan runtime is preselected; without a GPU, the CPU
  runtime. You can change the choice; Vulkan and CPU are untested by us.
- **Disk**: free space where the data folder is. The defaults need about
  1.1 GB while downloading and 1 GB after.

Then press **Install engine** (98 MB) and **Install recommended models**
(770 MB: the speaker model and the English speech model). Each item shows its
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
   itself into `engine\`).
3. Press **Check again** (setup screen) or **Refresh** (Settings). The app
   checks the hash, extracts a runtime zip, and marks the item installed.

### Firewalls

Per-app firewalls such as simplewall ask for permission the first time each
program connects. Two different programs do the downloading:

| Download | Program | Hosts it connects to (observed) |
|---|---|---|
| Speech engine (runtime zip) | `meet-assist.exe` | `github.com`, then GitHub's download servers (`objects.githubusercontent.com`, `release-assets.githubusercontent.com`) |
| Models | `C:\Windows\System32\curl.exe`, started by the engine's `nemo-speech.exe` | `huggingface.co`, then its download servers (`*.hf.co`, for example `us.aws.cdn.hf.co`) |
| Check for newer runtime | `meet-assist.exe` | `api.github.com` |
| Check for updates | `meet-assist.exe` | `github.com` |

If the firewall prompt was missed or denied, allow both programs and press
Retry; or fetch the files by hand as above. Diagnostics, "Why isn't it
working?", names the program when a download failed for a network reason.

Some networks block certificate revocation checks. Windows' `curl.exe` then
refuses the connection (`CRYPT_E_REVOCATION_OFFLINE`); fetching the model by
hand always works.

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
| Program | `%LOCALAPPDATA%\meet-assist` (chosen in the installer) |
| Data folder: database, recordings, engine, models, logs | `%LOCALAPPDATA%\app.meetassist.desktop` |
| A moved data folder | stored in `HKCU\Software\meet-assist\DataRoot` |
| Portable mode | create `portable.txt` next to `meet-assist.exe`: data goes to `data\` next to it (or the folder named on the file's first line) |

**Move data folder** (Settings, Storage) copies everything to an empty folder,
checks every file, restarts meet-assist from there and then deletes the old
copy.

Recordings are kept as lossless FLAC for 30 days (Settings, "Keep audio for"),
then deleted unless the meeting is pinned (P); transcripts are never deleted.
Re-processing a meeting needs its audio.

## 6. Updating

Settings, About, **Check for updates** asks GitHub for a newer release and
installs it after you confirm (updates are signed; the app checks the
signature). Your data folder is not touched. You can also run a newer
installer over the old one.

## 7. Uninstalling

Use Windows Settings, Apps, meet-assist, Uninstall (or `uninstall.exe` in
the program folder).

- **"Delete the application data" unticked (default)**: the program is
  removed and your transcripts and recordings stay. The uninstaller asks
  whether to remove the downloaded engine and models (about 1.6 GB); they can
  be downloaded again.
- **Ticked**: the data folder is removed too, including transcripts and
  recordings. For a moved data folder only meet-assist's own items are
  removed (database, audio, engine, models, manifests, logs, the WebView
  profile), never other files you keep there.

Silent uninstall: `uninstall.exe /S` keeps everything; add `/DELETEAPPDATA`
to remove the data folder, or `/REMOVEMODELS` to remove only the engine and
models.
