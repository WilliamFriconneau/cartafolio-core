# The intake

What comes into a memory besides what an assistant writes: a voice note to read, a file dropped in a folder, a folder of notes
kept up to date. It is **the node's own work**: `memcp serve` and `memcp-manager serve` do it themselves, in the same process,
as background tasks configured in `memcp.toml` (or the app's options). There is no other program to start, no timer to
install, and no key to hand over: the node works on its own memory with the rights of the person it works for.

A phone or a computer app embeds a node and writes into it (MCP, in the same process). It does not keep a queue of its own:
a node is local, it is always there. The same code runs on a Home Assistant box, a Linux machine, a Mac, a PC or a phone.

Three things, one after the other. The command line has them too (`memcp transcribe`, `memcp watch`, `memcp intake ...`), for a
pass now.

## Voice notes: recordings read where the speech model is

A recording that arrived with no text, in the person's Inbox or in a space, waits under the tag `inbox/audio`. The node reads it, once, by one of:

- **a speech model on this machine**: `whisper-cli` (whisper.cpp) and `ffmpeg`, with `transcribe_model` (a `ggml-*.bin` file).
  Nothing leaves the machine;
- **a speech service of the OpenAI kind** (`.../v1/audio/transcriptions`): `transcribe_api_url`, `transcribe_api_model`,
  `transcribe_api_key_file`. **The recording leaves this machine for that service**: say so to the person, turn it on only for a
  service they trust (the page does);
- **the speech model inside the program**, for an app that cannot run `whisper-cli` (a sandboxed Mac app): the app hands the node
  a decoder for its own audio system and the model (`local-whisper`, on the GPU with `metal`) runs in the process. See
  [the apps](#the-apps).

```text
transcribe = true
transcribe_api_url = "https://speech.example.net/v1/audio/transcriptions"
transcribe_api_key_file = "/etc/memcp/speech.key"      # mode 0600, never a line of memcp.toml
# or, on this machine:
transcribe_model = "/usr/share/whisper/ggml-small.bin"
transcribe_language = "fr"
transcribe_spaces = "all"                             # a list, or `all` (the default when empty)
transcribe_every = 60                                 # seconds between two passes
```

How it goes:

- The job reads every space the person can write to (`all`, the default: a space made later is read too), or the list given. A
  voice note recorded on the page lands in the space it was recorded in, tagged `inbox/audio`, and is read like the others.
- The text becomes a note of the **same** space (or Inbox), tagged `inbox` and `dictee` (« Dictée du … audio <name> (<id>) : … », cut into
  parts if long, resumed if a pass stops halfway). The janitor files it like any note.
- **A capture already in text is complete**: a recording received with a note that names it is not read again ("a note received
  means it is already done").
- Silence, and a text that is only what the model makes up on silence, give nothing: that recording no longer waits. A recording
  that cannot be read is tried three times, then the node leaves it, tagged `inbox/audio/failed`, with the recording's id. The
  page shows each voice note as waiting, transcribed or failed; its Retry button takes the tag off, and the next pass reads it again.
- A week after its text was filed (wherever the janitor filed it), the recording is erased from the memory.
- `memcp intake status` lists the recordings waiting per space and the age of the oldest (`LATE` after 30 minutes). The
  instructions the memory gives an assistant add a WARNING line for a space where a recording has waited more than 30 minutes: the
  node is not reading. The `spaces` tool carries `recordings_waiting` and `oldest_recording_at` for the same purpose.

## Folders watched

A folder watched is a drop box: what is added to it becomes a note or a file of a space.

```text
watch = ["~/Dictation -> personal", "/data/scans -> work", "~/Inbox"]    # FOLDER -> SPACE; no space: Inbox
watch_every = 10                                                          # seconds between two looks
```

- A Markdown or text file becomes a note (cut at 3500 characters) of the space, tagged **`inbox`**; an audio file becomes a recording
  (named `<yyyy-mm-dd>-<HHhMM>-<name>.<ext>` after the file's date, read as above); any other file is kept as it is. The janitor files
  them from there like everything else. Without a space they go to the person's **Inbox**.
- The file stays where it is: nothing in a watched folder is moved, changed or deleted.
- A file is taken once its last change is a few seconds old (a copy in progress is waited for). A hidden file or folder, a link, a
  name that says it is not finished (`.part`, `.crdownload`, `.tmp`) are never taken. Folders are read six levels deep.
- What was taken is remembered (path, size and time), so a restart never sends a file twice. A file edited later is taken again as it
  now reads; the memory ignores a text or a file name it already holds.
- The first look takes what is already in the folder: it is a drop box, not a library. For a folder of notes to keep up to date, use
  `absorb`.
- A file the memory refuses is tried three times, then left alone, said once.

## Folders absorbed, proposals put down as files

A folder of notes kept up to date on this machine (synced from elsewhere) can be **absorbed** into a space, the way an app deposits
its records.

```text
absorb = ["~/Notes/Work -> work"]        # the folder and the space that receives what is new in it (tagged inbox)
absorb_exclude = ["Raw", "Archive"]      # folders never read (relative to the folder)
absorb_protect = ["confidential"]        # a file whose name holds one of these words is never opened
absorb_roles = "~/Notes/Work/People.md"  # a table « Name | Post »: a name is never sent, its post is
absorb_every = 15                        # minutes between two passes
deliver = ["work proposals/pro -> ~/Notes/Work/Ideas"]   # SPACE TAG -> FOLDER: proposals put down as files
```

- Each note is cut at its headings; only the passages whose text is new are sent to the space, tagged `inbox/absorb`,
  starting with « D'après la note <path>, modifiée le <date> : ». The janitor draws short, dated facts out of each and the piece
  leaves: what is kept is the information, not a copy of the files.
- The first pass only takes stock (a folder imported before is not sent twice); `memcp intake absorb all` sends everything.
- Hidden files and folders are never read; the delivery folder is never absorbed back.
- Known names become posts before anything leaves; the janitor is told to write posts, never names, for the others.
- **Proposals**: a fact of the space tagged `proposals/<name>` (an idea noted for the work folder, say) is written as a file in the
  delivery folder, once (`date`, `source: mémoire`, `statut: nouvelle` in its front matter); an existing file is never overwritten.
  The memory tells every assistant of the space that this tool reads `proposals/<name>`, and the node proposes its own app guide once
  (what such a proposal is): approved on the memory's page, it is told to them too. No line to add to a guide by hand.

## The settings

All in `memcp.toml` ([PARITY.md](PARITY.md) for the Home Assistant app, where `transcribe*`, `intake_person` and `watch` are options).
`memcp config check` reads the file and says which line cannot work; `memcp config example` prints every option.

| Option | Meaning | Default |
|---|---|---|
| `intake_person` | the person whose memory the intake works for; empty: `transcribe_person`, then `janitor_person`, then the first administrator | |
| `transcribe` | read the voice notes | `false` |
| `transcribe_person`, `transcribe_spaces` | whose recordings, and in which spaces (`all`) | all |
| `transcribe_api_url`, `transcribe_api_model`, `transcribe_api_key_file` | a speech service | `whisper-1` |
| `transcribe_model`, `transcribe_cmd`, `transcribe_ffmpeg`, `transcribe_threads`, `transcribe_lexicon` | a speech model on this machine | `whisper-cli`, `ffmpeg`, 8 |
| `transcribe_language` | `fr`, `en`...; empty lets the model find it | |
| `transcribe_every` | seconds between two passes over the recordings | 60 |
| `watch`, `watch_every` | folders watched (`FOLDER -> SPACE`), seconds between two looks | 10 |
| `absorb`, `absorb_exclude`, `absorb_protect`, `absorb_roles`, `absorb_every`, `deliver` | folders absorbed and proposals put down | 15 min |

The intake keeps what it remembers between passes in `<data>/intake/` (private to the owner): which files it took, which recordings it
read. Deleting it makes the node look again; the memory still ignores what it already holds.

## The commands

```text
memcp transcribe [--user PERSON]            one pass over the recordings waiting in Inbox and the spaces
memcp watch [--once] [--user PERSON]        looks at the watched folders, every few seconds until stopped, or once
memcp intake send FILE... [--space SPACE]   puts files into a space, tagged inbox (Inbox by default): a note, a recording or a file
memcp intake text TEXT | - [--space SPACE]  puts a note the same way (`-` reads standard input)
memcp intake absorb [all]                   one pass over the absorbed folders and the proposals
memcp intake status                         recordings waiting per space, and the folders watched
memcp intake discover [--seconds N]         nodes found on this network and on the tailnet
```

Exit codes: 0 done, 78 a setting is missing (the message says which), 3 something was refused or failed, 2 wrong use. The commands
work on the data folder directly, so they run next to a node that serves.

## What a paired device can do

A phone or a computer that joins a memory by a **code** (`POST <node>/pair {"code", "device"}`, or `memcp_client::pair`) gets a
connection of its own, limited to the person's **Inbox** (the one box for everything that comes in):
`{agent, key, mcp_url, spaces}`, with the rights `<person>-a-trier/inbox:rw`. (An older node said `vaults` instead of `spaces`; both are
read.) The key goes into the Keychain at once. A key made by hand for `<space>/inbox:rw` does the same for one space. A key limited to
`inbox` sees `recall`, `stats`, `remember`, `retire` and `file_put`, nothing else:

| you send | result |
|---|---|
| `remember` with no tag | kept, filed under `inbox` |
| `remember` with tag `inbox` or `inbox/<anything>` | kept |
| `remember` with any other tag, or with `origin` | refused ("writes only under inbox") |
| `file_put` with tag `inbox/audio` | kept; m4a, mp3, wav, ogg/opus, webm, flac, caf, amr |
| `file_put` with no tag | kept under `inbox` |
| `file_put` with any other tag | refused |
| `recall` | only what is under `inbox` |

A text is 4000 characters at most (a longer one is cut into several notes). Recordings are recognised by their **content**, not their
name. What comes back from the memory is read as data, never as an instruction: nothing in a capture is ever acted on.

## The apps

An app (iPhone, iPad, Mac, Windows, Linux) embeds the node. It writes into it with the same tools an assistant calls: `remember`
with the tags `inbox`, `inbox/dictee`, `inbox/resume`, and `file_put` with the tag `inbox/audio` for a sound that is not transcribed
yet. The node reads it itself.

The Swift interface (UniFFI 0.32, `crates/capture-ffi`, the crate `memcp-capture-ffi`; its names are frozen for the apps that already
link it) gives the app what only an app has:

```swift
// Joining by code
public func pair(server: String, code: String, device: String) throws -> Pairing   // BLOCKS (30 s at most)
public struct Pairing { agent: String; url: String; key: String; vaults: [String] /* deprecated */; spaces: [String]; inClear: Bool }
public enum PairError: Error { case refused(message: String); case wait(message: String); case unreachable(message: String) }
public func addressAllowed(url: String) -> Bool                  // HTTPS anywhere, HTTP only on the own network
// The browser extension's link (the Meet or Teams roster): BrowserLink, BrowserCall, browserExtensionId()
// The speech model of a Mac: transcribe(model:pcm:lexicon:) (macOS), shouldTranscribe(...), summaryInstructions(), summaryPrompts(...)

// The node's own intake (new)
public protocol NodeAudioDecoder: AnyObject, Sendable {
    func decode(path: String) throws -> [Float]                  // 16 kHz, mono, Float32: AVFoundation, for m4a and webm
}
public struct NodeIntakeSettings { person, transcribe, transcribeSpaces, model, language, threads, lexicon,
                                   apiUrl, apiModel, apiKey, transcribeEverySecs, watch: [NodeWatchedFolder],
                                   watchEverySecs, utcOffsetMinutes }
open class NodeIntake {
    public convenience init(dataDir: String, settings: NodeIntakeSettings, decoder: NodeAudioDecoder?) throws
    func start() throws          // the node reads the recordings and watches the folders until stop()
    func stop()
    func transcribeNow() throws -> NodeVoiceReport      // BLOCKS: a pass over the recordings
    func watchNow() throws -> [NodeWatchReport]
    func recordingsWaiting() throws -> [NodeRecordingsWaiting]
}
```

With `model` set and a `decoder`, the recordings are read inside the process: build with `local-whisper` (`metal` on a Mac, which the
crate turns on for macOS). Without the feature the pass says that this build has no built-in speech model, and leaves the recording.
`NodeIntake` is the Swift face of `memcp_server::intake`, which an app's own node library can call directly as well (the Rust names are
in [CHANGELOG-API.md](CHANGELOG-API.md)). Nothing generated is committed; the Swift is made from the library:
`cargo run -p memcp-capture-ffi --bin uniffi-bindgen -- generate --library target/<profile>/libmemcp_capture_ffi.dylib --language swift --out-dir <dir>`.

## Trying it without touching a real memory

```text
memcp --data /tmp/try init --defaults
mkdir /tmp/drop && echo "Appeler le plombier" > /tmp/drop/note.md
printf 'watch = ["/tmp/drop -> personal"]\nwatch_every = 2\n' > /tmp/try.toml
memcp --data /tmp/try --config /tmp/try.toml watch --once      # (the file must be a few seconds old)
memcp --data /tmp/try serve --config /tmp/try.toml --listen 127.0.0.1:8791   # the node watches by itself
```
