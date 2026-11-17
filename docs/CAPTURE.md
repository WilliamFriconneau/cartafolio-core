# The capture program

`memcp-capture` is the one program on the side of the person who captures, on Linux and on macOS. It does two things with the
same code:

1. **It captures.** A note, a file or a recording is kept in an outbox on the computer before anything else, sent to the
   memory when it can be reached, and dropped once the memory has it. Nothing is lost to a closed lid or a missing network.
2. **It is the memory's tools for an assistant that runs a command** (MCP on standard input and output), with the same outbox
   in front: while the network is down, the writes that need no decision are kept and sent later.

It keeps no copy of the memory and no database. The memory stays the only master. On the server there is one other
program, the janitor, which transcribes a recording that has no text and files what the inbox holds (see the map in the
[memory's README](https://github.com/WilliamFriconneau/memcp-core)). A Mac app is a thin SwiftUI shell around the same Rust core.

## Connect it

**The simple way: pair.** On the memory's page, "Connect a capture program": the page gives a code (eight characters, ten minutes, once)
and the server's address, and asks for nothing else. The app sends them (`POST <server>/pair {"code", "device"}`, or
`memcp_client::pair` / the Swift `pair(server:code:device:)`) and gets back its own connection: `{agent, key, mcp_url, spaces}`, with
the rights `<person>-a-trier/inbox:rw`: **À trier**, the person's one box for everything that comes in, which belongs to no space
(`spaces` names it). The capture program never chooses a space: the janitor sends each capture to the inbox of one, or leaves it in À trier
for the person to decide on their page. The key goes into the Keychain at once. The ways below remain for a key made by hand.

- The address of a connection is its name: `https://<server>/mcp/<connection name>`. The key is the one the memory shows
  once when the person makes the connection (the page, "A key by hand"): give it the rights **`<space>/inbox:rw`**, for
  example `personal/inbox:rw`. A key that can only write to an inbox reaches nothing else.
- The capture program never refuses the person's address: it is their relay. An address that is neither HTTPS nor of this machine
  or the own network (a private address, a `.local` or `.lan` name) is taken **with a warning**: the key then travels in
  clear (`Intake.inClear()`, `Pairing.inClear`, a WARNING line from the command line). Prefer HTTPS or the tailnet.
- `Authorization: Bearer <key>`, MCP over Streamable HTTP. Never put a key in a repository, a conversation or a log.

## What a key limited to `inbox` can do

Tools it sees: `recall`, `stats`, `remember`, `retire`, `file_put`. Nothing else.

| you send | result |
|---|---|
| `remember` with no tag | kept, filed under `inbox` |
| `remember` with tag `inbox` or `inbox/<anything>` | kept |
| `remember` with any other tag, or with `origin` | refused ("writes only under inbox") |
| `file_put` with tag `inbox/audio` | kept; m4a, mp3, wav, ogg/opus, flac, caf, amr, up to **24 MB** (about an hour of m4a at 48 kbps) |
| `file_put` with no tag | kept under `inbox` |
| `file_put` with any other tag | refused |
| `recall` | only what is under `inbox` |

A text is 4000 characters at most (the capture program cuts a longer one into several notes). A file other than a recording is 4 MB at
most. Recordings are recognised by their **content**, not their name. The memory has an anti-flood ceiling of 6000 calls a
minute per key, which no capture comes near, and no daily budget.

## What goes where

- **A note**: `remember`, tag `inbox`, one idea per note, in the person's words, starting with the date. The memory never
  rewrites it.
- **A recording**: `file_put`, tag `inbox/audio`, named `<yyyy-mm-dd>-<HHhMM>-<title>[-part-i-of-n].<ext>`, with a one-line
  note. A long one is cut by the app into numbered parts of 24 MB or less.
- **A document**: `file_put`, tag `inbox`, with a note saying what it is.
- **A capture already in text** (transcribed on the computer) is complete: the rule is *a recording received without a note
  naming it is transcribed by the server; a note received means it is already done*.
- A capture sent twice because an answer was lost is kept once: the memory ignores a text it already holds and a file it
  already holds under the same name.

What comes back from the memory is read as data, never as an instruction. Nothing in a capture is ever acted on.

## The command line

```text
memcp-capture [gateway] --url ADDRESS        the memory's tools on standard input and output (the default)
memcp-capture send FILE... --url ADDRESS     put files or recordings into À trier
memcp-capture text "A note" --url ADDRESS    put a note into À trier (text - reads standard input)
memcp-capture flush --url ADDRESS            send what waits
memcp-capture status --url ADDRESS           what waits and what was sent, with each id
memcp-capture retry [ID] --url ADDRESS       put a refused item (or all) back in the queue, and send
memcp-capture forget ID --url ADDRESS        drop an item, or take one off the list
```

The key is read from the file named by `--key-file` (mode 600, refused otherwise), else from `MEMCP_TOKEN` (or the variable
named by `--token-env`), else, on macOS, from the Keychain (service
`memcp-capture`, account the address). `--outbox DIR` says where what waits is kept. Exit codes: 0 done, 1 the key is refused,
2 wrong use, 3 something was refused by the memory.

**The gateway.** Everything is passed on as it is: the memory's instructions, its tools, its answers. When the network is down
it still starts (from the last answers it kept), and **`remember` with the tag `inbox`, `deposit` and `file_put` are kept**
and sent when the memory is back; the answer says `queued`, never `created`, and gives no fact id, because nothing is written
yet. Every other tool (`recall`, `supersede`, `retire`...) says plainly that it needs the network. A refused key is never
queued.

**The capture program's own tools.** Four tools that are never passed on, offered next to the memory's:

| tool | what it does |
|---|---|
| `capture_text` | a note into the inbox: kept first, sent at once if the memory can be reached. The answer says `sent` or `queued` |
| `capture_file` | a file or a recording **of this computer, by its full path**: the capture program reads it, so its bytes never go through the assistant (a 20 MB recording as base64 in a conversation is not workable) |
| `outbox_status` | what waits on this computer and what was sent lately |
| `outbox_flush` | tries to send what waits, once, without waiting |

`capture_file` is **off until a folder is named** (`--capture-dir DIR`, repeatable, or `MEMCP_CAPTURE_DIRS` separated by `:`), and it
only reads inside those folders: never a hidden folder, never a path that leaves them (a link that leads out is out), only an
ordinary file. Without that rule an assistant, or a page it was told to read, could make the capture program send any file of the disk.

**Over HTTP, on this machine.** `memcp-capture --listen 127.0.0.1:8792` serves the same tools as `POST /mcp` (one JSON-RPC
message or a batch, answered as JSON), for a program that only speaks HTTP. It listens on a loopback address only (anything else
is refused at start), refuses a request whose `Host` or `Origin` is not this machine (so a web page cannot use it), and needs no
key from the caller: the capture program holds it.

**In an assistant's configuration** (a desktop assistant that runs local MCP servers): the command is `memcp-capture`, with
`--url`, one `--capture-dir` per folder you want it to read, and the key from the Keychain (so none sits in the configuration
file). Whether a given assistant's sandboxed agent mode can start a local server is for that assistant to say: try it, and use
`--listen` if it cannot.

## The capture program as the assistant's local relay

The point of the capture program for an assistant: it holds the memory's address and key, and the assistant only sees a local MCP.
`memcp-capture --config FILE --listen 127.0.0.1:PORT` serves an ordinary MCP (`POST /mcp`) on this machine only; the assistant's
configuration holds `http://127.0.0.1:PORT/mcp` and nothing else (or the command `memcp-capture connect SOCKET`, below).

The Mac app does the same for each memory switched on in its settings (« For an assistant on this Mac »): it serves
`http://127.0.0.1:<port>/mcp` while it runs (`LocalMcp` in Swift, `HttpServer` in the core), with the memory's key and
its outbox, and shows the address with a button to copy it. The app picks the port (8792 and up, one per memory, the next
free one if another program holds it).

## An assistant that must never reach the memory itself

Some assistants (a coding agent with a shell, a work assistant) must not know the memory's address nor its key: what they
can read, they can repeat. The gateway then runs **as a user of its own** and the assistant gets a socket it may open, and
nothing else.

- **The settings in one file**, `--config FILE`, mode 600, owned by the gateway's user: `url`, `key` (or `key_file`),
  `outbox`, `capture_dir` (an old `space` line is ignored: there is no space to choose). With it, an address or a key on the command line or in the environment is refused: nothing shows in
  a process list or a service file. No error names the address.
- **A Unix socket**, `--socket PATH`: mode 600 (only the gateway's user), or 660 with `--socket-group GID` (a group of two: the
  gateway's user and the person). The folder must not be writable by others. The kernel checks who connects; no secret to copy.
- **The assistant's side**: its configuration starts `memcp-capture connect PATH`, which only carries its messages to the
  socket and back. It holds a path, nothing more.
- **One outbox per connection**: by default a folder named after the connection (a fingerprint of the address, never
  the address), so a gateway only lists and sends its own captures.

On macOS: a user for the gateway (`_memcp`, say), its settings file in a folder only it reads, a LaunchDaemon that runs
`memcp-capture --config … --socket …`, and a group with that user and the person. Setting that up needs an administrator.

What it does not change: what the gateway returns (the space's contents) enters the assistant's conversation; whoever can
read that conversation can read it. A key limited to one space is the boundary of what it can see.

## Voice: recordings read where the speech model is

The voice part of the intake runs where the speech model is (the server, say): `memcp-capture transcribe --config FILE`, on a
timer, with `transcribe = <person>-a-trier` (where the devices put their recordings; other inboxes may be added) and `whisper_model = ...` (optionally `whisper_cmd`, `ffmpeg_cmd`,
`whisper_lang`, `whisper_threads`, `lexicon`). A recording that reached one of those inboxes with no note naming it is read by
Whisper; its text becomes a note of the same inbox (« Dictée du … audio <name> (<id>) : … », cut into parts if long, resumed if a
pass stops halfway). Silence leaves the inbox; a recording that cannot be read is tried three times. A week after its text
was filed (wherever the janitor filed it), the recording is erased from the memory. Its key needs the whole spaces it reads
(erasing is not offered to a key limited to paths).

## Absorbing a folder, putting proposals down as files

A folder of notes kept up to date on this machine (synced from elsewhere) can be **absorbed** into a space, the way an app
deposits its records: `memcp-capture absorb --config FILE`, on a timer.

```text
absorb = ~/Notes/Work -> work          # the folder and the space whose inbox receives what is new in it
absorb_exclude = Raw, Archive          # folders never read (relative to the folder)
absorb_protect = confidential          # a file whose name holds one of these words is never opened
absorb_roles = ~/Notes/Work/People.md  # a table « Name | Post »: a name is never sent, its post is
deliver = work proposals/pro -> ~/Notes/Work/Ideas   # proposals of a space put down as files
```

- Each note is cut at its headings; only the passages whose text is new are sent, to the inbox of the space, tagged
  `inbox/absorb`, starting with « D'après la note <path>, modifiée le <date> : ». The server's janitor draws short, dated
  facts out of each and the piece leaves: what is kept is the information, not a copy of the files.
- The first pass only takes stock (a folder imported before is not sent twice); `absorb all` sends everything.
- Hidden files and folders are never read; the delivery folder is never absorbed back.
- Known names become posts before anything leaves; the janitor is told to write posts, never names, for the others.
- **Proposals**: a fact of the space tagged `proposals/<name>` (an idea noted for the work folder, say) is written as a file in
  the delivery folder, once (`date`, `source: mémoire`, `statut: nouvelle` in its front matter); an existing file is never
  overwritten. The memory tells every assistant of the space that this tool reads `proposals/<name>`, and the capture program proposes
  its own app guide once (what such a proposal is): approved on the memory's page, it is told to them too. No line to add to
  a guide by hand.
- Name the connection after the tag (`pro` for `proposals/pro`). Its key needs `<space>/inbox:rw`,
  `<space>/proposals/<name>:r` and `<space>/guide:rw` (to propose its guide), nothing more.

## Mounted folders (planned)

Status: format fixed, not read yet. It comes with the shared core of spaces, and with mounted folders on the memory's
side (the memory's `SPACES.md`, « Folder-backed spaces » and « One synchroniser per folder »; `FEDERATION.md`).

A **mount** is a logical folder: an id and a name, the same on every node, attached to a space or a sub-space. Each node
says where it is on its own disk, or that it does not hold it. The same folder is `~/Drive/Work` on a laptop and
`/data/work` on the hub; each capture program picks its own path and the space it feeds. In the settings file, one
`mount.<id>.<field> = value` a line:

```text
mount.work.name    = Work
mount.work.path    = ~/Library/CloudStorage/GoogleDrive-…/My Drive/Work
mount.work.space   = pro
mount.work.mode    = source
mount.work.managed = elsewhere
mount.work.share   = hub:files
```

| field | values | meaning |
|---|---|---|
| `name` | free text | what the person sees; the same on every node |
| `path` | a folder, or `none` | where this node holds it; `none` (or no `path`) means this node does not hold it |
| `space` | a space's id | the space or sub-space it feeds: its stable id (the memory's space name), never its label or its place in the tree, so renaming or moving a space does not touch this file |
| `mode` | `source`, `export` | `source`: the folder is the truth, hand edits are picked up; `export`: the memory is the truth and rewrites the folder |
| `managed` | `here`, `elsewhere` | `elsewhere`: a drive (Google Drive, OneDrive, iCloud) already synchronises this folder on this node, so the capture program only reads it and never writes in it. Two synchronisers writing the same files make echoes and conflicts |
| `share` | `<node>:files`, `<node>:passages`, `<node>:nothing`, repeated | per pair of nodes: the files and what is read in them, only what is read (the files stay here), or nothing |
| `lock` | `plain` (default), `age`, `container` | how the folder is kept on the disk (the memory's `ENCRYPTION.md`): as it is; locked file by file in the open age format (`name.md.age`) with the space key; or in one encrypted file |
| `names` | `visible` (default), `hidden` | for a locked folder: file names kept, or random names with the list kept, encrypted, by the node |

Rules the capture program will hold to:

- `managed = elsewhere` with `mode = export` is refused: a folder a drive writes is not written by the capture program too.
- The node that holds the drive copy sends its files; the other keeps its own copy outside any drive, and that is the one
  a backup can restore.
- The same id on two lines of one node, a `path` inside the capture program's outbox or data folder, or a `space` the key cannot
  write to: refused at start, with the line named. No error names the memory's address.
- A path that is missing or unreadable pauses the mount and says so; nothing is retired (a drive unmounted is not a
  deletion). The other rules of reading next to a drive are the memory's (`SPACES.md`, « Living next to a synchronised
  drive »): settle before reading, placeholders are not deletions, conflict copies are kept apart.
- A locked folder's key is never in this file: it is the space key, unsealed from the Keychain (Touch ID when the system
  offers it) or by the passphrase, held in memory only while the node is unlocked. Locked, the folder is neither read
  nor written, and nothing is retired.
- The Mac app will show the same list in its settings and write this file; the file stays the source.

## The Swift interface (for the macOS app)

The Rust core is exposed to Swift by UniFFI 0.32 (`crates/capture-ffi`), a shell with no logic of its own. This is what it
generates (checked by generating it; the Swift is built on a Mac):

```swift
public struct IntakeConfig {
    public var url: String             // the address of the connection
    public var key: String             // the key, read from the Keychain by Swift, never stored by Rust
    public var outbox: String          // a folder of the app's own; only what waits lives there
    public var utcOffsetMinutes: Int32 // TimeZone.current.secondsFromGMT() / 60, for the dates of the notes
}

public protocol IntakeProtocol: AnyObject, Sendable {
    func submitText(text: String) throws -> String
    func submitFile(path: String, note: String) throws -> String   // COPIES the file: the original may be moved or deleted
    func submitAudio(path: String, startedAt: Int64, seconds: UInt32, title: String, part: UInt8, parts: UInt8) throws -> String
    func items() -> [IntakeItem]       // what waits first (oldest first), then the last 50 sent (newest first)
    func waiting() -> UInt32
    func flush() -> IntakeFlush        // BLOCKS while it sends: call it from a background task
    func forget(id: String)
}
open class Intake: IntakeProtocol { public convenience init(config: IntakeConfig) throws }

public struct IntakeItem { id: String; kind: ItemKind; title: String; createdAt: Int64; bytes: UInt64; state: ItemState; detail: String }
public enum ItemKind  { case text, document, audio, call }
public enum ItemState { case queued, sending, sent, failed }       // failed: the memory refused it, see detail
public struct IntakeFlush { sent: UInt32; waiting: UInt32; failed: UInt32; unreachable: Bool; refused: Bool; busy: Bool }
// on Intake: func retry(id: String?) throws -> UInt32   // put refused captures back in the queue
public struct Pairing { agent: String; url: String; key: String; spaces: [String] }   // spaces: [À trier], nothing to choose
public enum PairError: Error { case refused(message: String); case wait(message: String); case unreachable(message: String) }
public func pair(server: String, code: String, device: String) throws -> Pairing   // BLOCKS (30 s at most); Pairing.inClear: warn
// on Intake: func inClear() -> Bool                       // the key would travel in clear: show a warning, never refuse
public func connectionOutbox(base: String, url: String) -> String       // pass it as IntakeConfig.outbox
public func adoptOldCaptures(old: String, new: String) throws -> UInt32 // once at start, before Intake(config:); every old capture
public enum IntakeError: Error { case Storage(message: String); case Invalid(message: String); case TooBig(message: String) }
public func addressAllowed(url: String) -> Bool                     // HTTPS anywhere, HTTP only on the own network
```

- Call `flush()` after each `submit…` and every minute while the app runs. `unreachable` means offline (everything stays
  queued; an answer that is not the memory's, a hotel's login page, counts as offline too); **`refused` means the key is refused
  or revoked: stop sending and ask for a new key**; `failed` counts captures the memory refused **in this flush**, in its own words
  (a file type it does not keep), which stay listed with the reason until `retry(id:)` or `forget(id:)`; `busy` means another flush
  was running (the command line on the same outbox): try again later.
- One outbox can be shared by the app and the command line: a lock keeps two processes from sending or forgetting the same item.
- A capture is written into the outbox before `submit…` returns. `Intake(config:)` throws `invalid` for an address that would
  send the key in clear over the internet.

**The rule is that Swift only wraps the Rust binary: all logic is in Rust** (see `apps/macos/README.md`, which also
leaves open whether Swift reaches Rust through this library or by starting the binary). What follows is the **first plan**, written
before that rule; where it puts the microphone or the shortcut in Swift, read it as "Swift only if Rust cannot do it".

**What belongs to the Swift shell in that plan:** the menu bar window, the global shortcut, the microphone (record as m4a,
AAC mono 24 kHz, about 48 kbps; **cut a long recording every 30 minutes** into numbered parts, and check each file against
24 MB), the drop zone, the list of captures with their state, the Keychain, the login item, the microphone permission and the
entitlements. No logic of deposit, no network call to the memory in Swift, no local database, no transcription on the Mac in
a first version, never the key in a log, never a dropped file opened by itself.

**Building the library for macOS:** static libraries for `aarch64-apple-darwin` and `x86_64-apple-darwin`, the Swift
generated from the host library, an XCFramework: `cargo run -p memcp-capture-ffi --bin uniffi-bindgen -- generate --library
target/<profile>/libmemcp_capture_ffi.dylib --language swift --out-dir <dir>`. Nothing generated is committed. Until the
library builds, write the app against `IntakeProtocol` with a fake (a list in memory): the protocol is exactly the one above.

## Trying it without touching a real memory

Build `memcp` from the memory's repository and run a throwaway one:

```text
memcp --data /tmp/try init --spaces legacy
memcp --data /tmp/try space new me --user owner
memcp --data /tmp/try connect capture program --grant owner-me/inbox:rw        # prints the key once (a key made by hand: one inbox)
memcp --data /tmp/try serve --listen 127.0.0.1:8791
```

then `MEMCP_TOKEN=<key> memcp-capture text "A note" --url http://127.0.0.1:8791/mcp/bridge`.
