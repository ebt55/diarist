# Privacy

Diarist is built so that your meetings never leave your computer.

## What stays on your computer

Everything the app records or produces: microphone and system audio,
imported files, transcripts, speaker names, edits, settings and logs. They are
stored in the data folder (`%LOCALAPPDATA%\app.diarist.desktop`, or the
folder you moved it to) and nowhere else. Speech recognition and speaker
detection run on your own GPU or CPU. There is no account, no cloud service,
no analytics, no crash reporting and no telemetry of any kind.

## When the app uses the network

Only when you ask it to, and only to fetch files or version lists:

| You press | Connects to | What is sent |
|---|---|---|
| Install engine, or a runtime in Check for newer runtime | `github.com` and GitHub's download servers | an ordinary file download request |
| Install or Download a model | `huggingface.co` and its download servers (through Windows' `curl.exe`) | an ordinary file download request |
| Add from URL | the address you typed | an ordinary file download request |
| Check for newer runtime | `api.github.com` | a request for the NeMo-Speech.cpp release list, with the user agent `diarist/<version>` |
| Check for updates | `github.com` (the release's `latest.json`) | an ordinary file request |

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
