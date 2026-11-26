# Pensine on a computer: the desktop node

The same memory as the node under Home Assistant, on a Mac or a Windows computer: the node (`memcp serve`) keeps the
memory, serves its page, reads the voice notes and watches the folders the person named ([INTAKE.md](INTAKE.md)). It is the one
program: it runs as a task of the person's own user, starts at login, starts again after a crash and writes a log. Nothing needs an
administrator, nothing listens beyond this computer (127.0.0.1 only), and nothing leaves it unless a speech service is chosen for
the voice notes.

| | Mac | Windows |
|---|---|---|
| installer | `packaging/macos/install-mac.sh` | `packaging/windows/install.ps1` |
| programs | `~/Library/Application Support/Pensine/bin` | `%LOCALAPPDATA%\Pensine\bin` |
| memory | `~/Library/Application Support/Pensine/data` | `%LOCALAPPDATA%\Pensine\data` |
| settings | `~/Library/Application Support/Pensine/memcp.toml` | `%LOCALAPPDATA%\Pensine\memcp.toml` |
| logs | `~/Library/Logs/Pensine` | `%LOCALAPPDATA%\Pensine\logs` |
| task | launchd agent `~/Library/LaunchAgents/org.memcp.node.plist` | Task Scheduler, folder `\Pensine\`, task `Pensine node` |
| keys | Keychain, service `memcp`, account the connection's address | `keys\desktop.key`, `keys\device.key`, readable by the person only |
| assistants | `http://127.0.0.1:8791/mcp/desktop` with the key of the Keychain | the same address, the key of `keys\desktop.key` |

## Install

Mac (Apple silicon):

```sh
curl -fsSLO https://raw.githubusercontent.com/WilliamFriconneau/pensine-core/main/packaging/macos/install-mac.sh
sh install-mac.sh
```

Windows (PowerShell, as yourself, not as an administrator):

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/WilliamFriconneau/pensine-core/main/packaging/windows/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1
```

Both download the release's programs and check them against its `SHA256SUMS` (or take them from a build of your own:
`--from DIR`, `-From DIR`), make the memory (`--spaces`, `--language`), write the settings file, start the node, then:

1. **pair this computer with its own memory**, as a phone does. That makes the person's Inbox, the one box every
   capture goes to; the computer's key writes only there and reads nothing;
2. **make the connection of the assistants of this computer** (`desktop`, all rights): the memory's tools at the node's address.
   Its key is in the Keychain on a Mac, in a file only the person can read on Windows.

At the end they show the page's address (`http://127.0.0.1:8791/admin`) and a console key that opens it, once.

Running an installer again updates the programs and leaves the memory and the keys as they are.

**From an earlier version** (the one that ran a second program, `memcp-capture`, with its own port, settings and tasks): the next run of
the installer removes the old tasks (`org.memcp.bridge`, `org.memcp.transcribe`, `Pensine bridge`, `Pensine transcribe`), carries the voice
settings of the old `bridge/bridge.conf` into `memcp.toml`, moves the keys (Keychain service `memcp-capture` to `memcp`; `bridge.key` and
`device.key` to `keys\`), keeps the old folder as `bridge.migrated`, and keeps an existing connection named `desktop-bridge` (its address
is its name). `--bridge-port` and `MEMCP_BRIDGE_PORT` are read, said to be unused, and ignored; the assistants reach the node on its own port.

## Voice notes and files

A recording sent by a phone, an app, or `memcp intake send note.m4a` waits in Inbox with the tag `inbox/audio`. The node reads it every
minute and puts its text beside it (« Dictée du … : … »), then erases the recording a week after its text was filed. Two ways:

- **on this computer**, nothing leaves it: whisper.cpp and ffmpeg, with a model.
  Mac: `brew install whisper-cpp ffmpeg`, then `sh install-mac.sh --whisper-model ~/models/ggml-large-v3-turbo-q5_0.bin`.
  Windows: `whisper-cli.exe` and `ffmpeg.exe` on the PATH, then `install.ps1 -WhisperModel C:\models\….bin`.
  Models: <https://huggingface.co/ggerganov/whisper.cpp> (`large-v3-turbo-q5_0` reads French well, about 550 MB).
- **by a speech service** of the OpenAI kind (`…/v1/audio/transcriptions`): `--speech-api URL` / `-SpeechApi URL`,
  key in `MEMCP_SPEECH_API_KEY` (it is kept in a file only the person reads, never in `memcp.toml`). The recordings leave the computer
  for that service: choose one you trust.

Both write `transcribe = true` and its settings into `memcp.toml`; edit it to change them, and restart the node.

**Watched folders**: add `watch = ["~/Dictation -> personal"]` to `memcp.toml`. A file added to the folder becomes a note, a recording or a
file of that space ([INTAKE.md](INTAKE.md#folders-watched)).

A file (`memcp intake send devis.pdf`) is kept in Inbox as it is, and the person or the janitor sends it to a space from the page.

## Assistants

An assistant of this computer reaches the node at `http://127.0.0.1:8791/mcp/desktop`, with the key of its connection:

```sh
claude mcp add --transport http memcp http://127.0.0.1:8791/mcp/desktop \
  --header "Authorization: Bearer $(security find-generic-password -s memcp -a http://127.0.0.1:8791/mcp/desktop -w)"
```

An assistant that only runs commands can use `memcp --data DIR mcp` (standard input and output; the key from `MEMCP_TOKEN`).

## Look, stop, remove

- Mac: `launchctl list | grep org.memcp`; `launchctl kickstart -k gui/$(id -u)/org.memcp.node` to restart the node;
  logs in `~/Library/Logs/Pensine`.
- Windows: `Get-ScheduledTask -TaskPath \Pensine\`; `Restart` with `Stop-ScheduledTask` then `Start-ScheduledTask`;
  logs in `%LOCALAPPDATA%\Pensine\logs`.
- Remove: `sh install-mac.sh --uninstall` or `install.ps1 -Uninstall` stops the task and removes it, the programs and
  the keys; **the memory is kept**. Add `--purge` / `-Purge` to delete the memory and the logs too (asked first).

## Tested

`.github/workflows/desktop.yml` installs both for real on the CI's Mac and Windows machines with the programs of the
commit, sends a note and a file to Inbox with `memcp intake text` and `intake send`, asks for the tools at the node's address
with the key of the `desktop` connection, then removes everything. A voice note read by whisper.cpp was checked by hand on a Mac (the CI
has no speech model); `crates/client/tests/real_nodes.rs` runs the real programs on Linux, with a speech service and with `ffmpeg` and
`whisper-cli` stood in for by two scripts.
