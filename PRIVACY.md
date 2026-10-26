# Privacy

Diarist is built so that your meetings never leave your computer.

## What stays on your computer

Everything the app records or produces: microphone and system audio,
imported files, transcripts, speaker names, edits, settings and logs. They are
stored in the data folder (`%LOCALAPPDATA%\app.diarist.desktop`, or the
folder you moved it to) and nowhere else. Speech recognition and speaker
detection run on your own GPU or CPU. There is no account, no cloud service,
no analytics, no crash reporting and no telemetry of any kind.

If Diarist or its speech engine crashes, it writes a crash report: a text
file in the data folder's `logs` folder (`crash-<time>.txt`, or
`engine-crash-<time>.txt` for the engine), with the app version, Windows
version, GPU and driver, what went wrong and the last lines of the app log.
A crash in native code also leaves a minidump (`crash-<time>.dmp`), a copy
of part of the program's memory that may contain transcript text; it is for
your own debugging. Crash reports are never transmitted. The next launch
tells you a report exists; **Copy report** puts the text report (never the
minidump) on the clipboard with your user folder shown as `~`, for you to
paste into an issue if you choose. The newest five reports of each kind are
kept, and they are deleted with the data folder.

## When the app uses the network

Only when you ask it to, and only to fetch files or version lists:

| You press | Connects to | What is sent |
|---|---|---|
| Install engine, or a runtime in Check for newer runtime | `github.com` and GitHub's download servers | an ordinary file download request |
| Install or Download a model | `huggingface.co` and its download servers (up to 0.1.0-beta.4 through Windows' `curl.exe`, since 0.1.0-beta.5 by Diarist itself) | an ordinary file download request, with the user agent `diarist/<version>` |
| Add from URL | the address you typed | an ordinary file download request |
| Check for newer runtime | `api.github.com` | a request for the NeMo-Speech.cpp release list, with the user agent `diarist/<version>` |
| Check for updates | `raw.githubusercontent.com` (the file `ebt55/diarist/main/updater/latest.json`) | an ordinary file request |
| Install and restart, after Check for updates found a new version | `github.com` and GitHub's download servers (the release's installer) | an ordinary file download request |

Like any download, these requests reveal your IP address to the server
(GitHub, Hugging Face, or the site you chose). Nothing about your meetings,
your computer or your use of the app is ever sent. The app never checks for
updates or new runtimes by itself.

## Recording other people

Diarist records whatever your microphone and your computer's audio carry,
including other people in a call. Recording laws differ between countries and
workplaces; many require telling or asking the other participants. That is
your responsibility.

## Deleting your data

- Delete a meeting in the app (Ctrl+Delete): its transcript and audio go.
- Recordings older than the retention setting (30 days by default) are
  deleted automatically unless pinned; transcripts stay until you delete them.
- Uninstalling with "Delete the application data" ticked removes the whole
  data folder. Unticked, your transcripts and recordings stay.
