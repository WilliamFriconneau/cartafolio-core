# MemCP on a computer: the desktop node

The same memory as the server under Home Assistant, on a Mac or a Windows computer: the node (`memcp serve`) keeps the
memory and serves its page; the capture program (`memcp-capture`) gives the memory's tools to the assistants of this computer and,
if asked, reads the voice notes. Both run as tasks of the person's own user, start at login, start again after a crash
and write a log. Nothing needs an administrator, nothing listens beyond this computer (127.0.0.1 only), and nothing
leaves it unless a speech service is chosen for the voice notes.

| | Mac | Windows |
|---|---|---|
| installer | `packaging/macos/install-mac.sh` | `packaging/windows/install.ps1` |
| programs | `~/Library/Application Support/MemCP/bin` | `%LOCALAPPDATA%\MemCP\bin` |
| memory | `~/Library/Application Support/MemCP/data` | `%LOCALAPPDATA%\MemCP\data` |
| bridge (settings, what waits) | `~/Library/Application Support/MemCP/bridge` | `%LOCALAPPDATA%\MemCP\bridge` |
| logs | `~/Library/Logs/MemCP` | `%LOCALAPPDATA%\MemCP\logs` |
| tasks | launchd agents `~/Library/LaunchAgents/org.memcp.{node,bridge,transcribe}.plist` | Task Scheduler, folder `\MemCP\` |
| keys | Keychain, service `memcp-capture`, account the connection's address | `bridge.key`, `device.key` in the capture program folder, readable by the person only |
| assistants | `memcp-capture connect 127.0.0.1:8792` | `memcp-capture.exe connect 127.0.0.1:8792` (no Unix socket on Windows) |

## Install

Mac (Apple silicon):

```sh
curl -fsSLO https://raw.githubusercontent.com/WilliamFriconneau/memcp-core/main/packaging/macos/install-mac.sh
sh install-mac.sh
```

Windows (PowerShell, as yourself, not as an administrator):

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/WilliamFriconneau/memcp-core/main/packaging/windows/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1
```

Both download the release's programs and check them against its `SHA256SUMS` (or take them from a build of your own:
`--from DIR`, `-From DIR`), make the memory (`--spaces`, `--language`), start the node, then:

1. **pair this computer with its own memory**, as a phone does. That makes the person's « À trier », the one box every
   capture goes to; the computer's key writes only there and reads nothing;
2. **make the capture program's connection** (`desktop-bridge`, all rights): the memory's tools for the assistants of this
   computer, and the voice notes to read. Its key is in the Keychain on a Mac, in a file only the person can read on
   Windows; the settings file holds the address only on a Mac;
3. start the capture program, and the reading of voice notes if a model or a service was given.

At the end they show the page's address (`http://127.0.0.1:8791/admin`) and a console key that opens it, once.

Running an installer again updates the programs and leaves the memory and the keys as they are.

## Voice notes and files

A recording sent by a phone, the Mac app, or `memcp-capture send note.m4a` waits in « À trier » under `inbox/audio`. The
transcribe task reads it every five minutes and puts its text beside it (« Dictée du … : … »), then erases the recording
a week after its text was filed. Two ways:

- **on this computer**, nothing leaves it: whisper.cpp and ffmpeg, with a model.
  Mac: `brew install whisper-cpp ffmpeg`, then `sh install-mac.sh --whisper-model ~/models/ggml-large-v3-turbo-q5_0.bin`.
  Windows: `whisper-cli.exe` and `ffmpeg.exe` on the PATH, then `install.ps1 -WhisperModel C:\models\….bin`.
  Models: <https://huggingface.co/ggerganov/whisper.cpp> (`large-v3-turbo-q5_0` reads French well, about 550 MB).
- **by a speech service** of the OpenAI kind (`…/v1/audio/transcriptions`): `--speech-api URL` / `-SpeechApi URL`,
  key in `MEMCP_SPEECH_API_KEY`. The recordings leave the computer for that service: choose one you trust.

A file (`memcp-capture send devis.pdf --note "…"`) is kept in « À trier » as it is, and the person or the janitor sends
it to a vault from the page.

## Assistants

An assistant that runs a command starts `memcp-capture connect 127.0.0.1:8792`: it reaches the memory through the capture program
and knows neither its address nor its key. For Claude Code:

```sh
claude mcp add memcp -- "$HOME/Library/Application Support/MemCP/bin/memcp-capture" connect 127.0.0.1:8792
```

## Look, stop, remove

- Mac: `launchctl list | grep org.memcp`; `launchctl kickstart -k gui/$(id -u)/org.memcp.node` to restart the node;
  logs in `~/Library/Logs/MemCP`.
- Windows: `Get-ScheduledTask -TaskPath \MemCP\`; `Restart` with `Stop-ScheduledTask` then `Start-ScheduledTask`;
  logs in `%LOCALAPPDATA%\MemCP\logs`.
- Remove: `sh install-mac.sh --uninstall` or `install.ps1 -Uninstall` stops the tasks and removes them, the programs and
  the keys; **the memory is kept**. Add `--purge` / `-Purge` to delete the memory and the logs too (asked first).

## Tested

`.github/workflows/desktop.yml` installs both for real on the CI's Mac and Windows machines with the programs of the
commit, sends a note and a file to « À trier » through the capture program, asks for the tools through `memcp-capture connect`,
then removes everything. A voice note read by whisper.cpp was checked by hand on a Mac (the CI has no speech model).
