//! What is captured (a note, a file, a recording) is kept on this computer until the memory has it, then
//! it goes. Nothing else is stored here: no copy of the memory, no database, only the outbox.
//!
//! Everything goes **into « À trier »**, the person's one box for everything that comes in, which belongs to no
//! space: the capture program never chooses one, its key reaches only that box. The server's janitor sends each capture to the
//! inbox of a space and files it there; the capture program transcribes the recordings. A capture that is already text is complete: it is never rewritten.
//!
//! - [`Intake::submit_text`], [`Intake::submit_file`], [`Intake::submit_audio`] write the capture to the
//!   outbox before they return;
//! - [`Intake::flush`] sends what waits, in order, and says how it went. A memory that cannot be reached
//!   leaves everything queued; a refused key stops at once; a refusal of the memory in its own words (a file it
//!   does not keep) marks that item failed and goes on with the next;
//! - a capture sent twice (the connection was cut before the answer) is kept once: the memory ignores a text
//!   it already holds and a file it already holds under the same name;
//! - [`Gateway`] is the same buffer in front of the memory's tools, for an assistant that runs a command;
//! - the `memcp-capture` program is both, on Linux and on macOS.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use memcp_client::{Client, Error as ClientError, Retry};
use serde_json::{Value, json};

mod absorb;
pub mod browser;
pub mod discover;
mod gateway;
mod http;
#[cfg(feature = "local-whisper")]
pub mod local;
pub mod markdown;
mod outbox;
pub mod plan;
mod private;
pub mod relay;
#[cfg(unix)]
mod socket;
mod time;
mod voice;
mod whisper;

pub use absorb::{
    Absorbed, Delivery, Source as AbsorbSource, absorb, deliver, propose_guide, read_roles,
    without_names,
};
pub use browser::BrowserLink;
pub use gateway::Gateway;
pub use http::{HttpServer, serve_http};
pub use memcp_client::{Paired, pair, pair_allowing};
use outbox::{Entry, Outbox};
#[cfg(unix)]
pub use socket::{bind as bind_socket, connect as connect_socket, serve_socket};
pub use voice::{VoiceReport, transcribe};
pub use whisper::VoiceSettings;

/// The biggest recording the memory keeps, and the biggest other file.
pub const MAX_AUDIO: u64 = 24 * 1024 * 1024;
pub const MAX_FILE: u64 = 4 * 1024 * 1024;
/// The biggest text of one note the memory keeps is 4000 characters: longer is cut into several.
const NOTE_MAX: usize = 3800;
/// An item the memory could not read this many times is given up.
const ATTEMPTS: u32 = 5;
const AUDIO_EXTENSIONS: [&str; 8] = ["m4a", "mp3", "wav", "ogg", "opus", "flac", "caf", "amr"];

/// No `Debug`: it holds the key.
#[derive(Clone)]
pub struct Config {
    /// The address of the connection: `https://<server>/mcp/<name>`.
    pub url: String,
    /// The connection's key. Read from the Keychain by the caller; never written here.
    pub key: String,
    /// A folder of its own, where only what waits lives.
    pub outbox: PathBuf,
    /// Minutes east of UTC where the user is, for the dates the notes carry.
    pub utc_offset_minutes: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Document,
    Audio,
    /// A call of a tool of the memory kept by the [`Gateway`].
    Call,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Document => "document",
            Kind::Audio => "audio",
            Kind::Call => "call",
        }
    }
    fn from_word(w: &str) -> Option<Kind> {
        Some(match w {
            "text" => Kind::Text,
            "document" => Kind::Document,
            "audio" => Kind::Audio,
            "call" => Kind::Call,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Sending,
    Sent,
    /// The memory refused it in its own words; it is not sent again unless retried.
    Failed,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Sending => "sending",
            State::Sent => "sent",
            State::Failed => "failed",
        }
    }
    fn from_word(w: &str) -> Option<State> {
        Some(match w {
            "queued" => State::Queued,
            "sending" => State::Sending,
            "sent" => State::Sent,
            "failed" => State::Failed,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub created_at: i64,
    pub bytes: u64,
    pub state: State,
    /// Why it failed, or how many tries it has had.
    pub detail: String,
    /// When the memory took it (Unix time), 0 while it waits. It went to « À trier », where the janitor files it.
    pub sent_at: i64,
    /// What this machine heard in a recording (Whisper on this machine), empty when the memory's side transcribes it.
    pub heard: String,
    /// The recording that went, kept here to be listened to again; empty when there is none.
    pub audio: String,
    /// Its summary, made on this machine; empty when there is none.
    pub summary: String,
}

/// What this machine made of a recording, and what of it goes to this memory.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    /// The transcript (Whisper here); empty when the memory's side transcribes it.
    pub text: String,
    /// Its summary (the system's model here); may be empty.
    pub summary: String,
    pub send_audio: bool,
    pub send_text: bool,
    pub send_summary: bool,
}

/// How a [`Intake::flush`] went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flush {
    pub sent: u32,
    /// Still queued: they will be tried again.
    pub waiting: u32,
    /// Refused by the memory in this flush (they stay listed, with the reason, until retried or forgotten).
    pub failed: u32,
    /// The memory could not be reached (or asked to wait): everything stays queued.
    pub unreachable: bool,
    /// The key is refused or revoked: nothing more is sent until there is a new key.
    pub refused: bool,
    /// Another flush was running (in this process or another on the same outbox): nothing was tried now.
    pub busy: bool,
}

#[derive(Debug)]
pub enum Error {
    /// The outbox could not be written.
    Io(String),
    /// What was given is not a capture (an empty text, a part that does not exist).
    Invalid(String),
    /// Too big for the memory, which would refuse it.
    TooBig(String),
    /// The folder of Markdown could not be written.
    Folder(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(m) => write!(f, "the outbox cannot be written: {m}"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::TooBig(m) => write!(f, "too big: {m}"),
            Error::Folder(m) => write!(f, "the folder cannot be written: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub struct Intake {
    config: Config,
    outbox: Outbox,
    /// One flush at a time: a second one called meanwhile answers at once.
    flushing: Mutex<()>,
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// A file name that is safe to keep: no separators, no control characters.
fn clean(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // Cut to 80 characters, keeping the extension: it says what the file is.
    let (stem, ext) = match s.rsplit_once('.') {
        Some((a, b)) if !a.is_empty() && b.len() <= 8 && !b.contains(' ') => {
            (a.to_string(), format!(".{b}"))
        }
        _ => (s.clone(), String::new()),
    };
    let keep = 80usize.saturating_sub(ext.chars().count());
    let stem: String = stem.chars().take(keep).collect();
    format!("{}{ext}", stem.trim_matches('-').trim())
}

/// `text` in pieces of at most `size` characters, cut at the end of a sentence where it can be.
fn split(text: &str, size: usize) -> Vec<String> {
    if text.chars().count() <= size {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    let mut cur = String::new();
    for word in text.split_inclusive(char::is_whitespace) {
        if cur.chars().count() + word.chars().count() > size && !cur.is_empty() {
            parts.push(cur.trim().to_string());
            cur = String::new();
        }
        // A word longer than a piece is cut where it stands.
        let mut rest = word;
        while rest.chars().count() > size {
            let cut: String = rest.chars().take(size).collect();
            rest = &rest[cut.len()..];
            parts.push(cut);
        }
        if !(cur.is_empty() && rest.trim().is_empty()) {
            cur.push_str(rest);
        }
    }
    parts.retain(|p| !p.trim().is_empty());
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

/// The outbox of one connection under `root` (the app's data folder): a folder named after a fingerprint of the address
/// (FNV-1a, stable across versions), never the address itself, so that one connection never lists or sends another's
/// captures.
pub fn connection_outbox(root: &Path, url: &str) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.trim_end_matches('/').bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    root.join("connections").join(format!("{h:016x}"))
}

/// Moves the captures left in the outbox all connections shared before (`old`, its `queue` folder) into `new`, all of
/// them, whatever space they were made for: they go to « À trier » like any capture. How many moved. Safe to call at
/// every start: once moved, there is nothing left to move.
pub fn adopt_old_captures(old: &Path, new: &Path) -> Result<u32, Error> {
    let queue = old.join("queue");
    if !queue.is_dir() || old == new {
        return Ok(0);
    }
    let from = Outbox::open(old)?;
    let to = Outbox::open(new)?;
    let _a = from.lock(true)?;
    let _b = to.lock(true)?;
    let mut n = 0;
    for e in from.queue() {
        let blob = from.blob(&e.id);
        to.put(&e, blob.as_deref())?;
        from.remove(&e.id);
        n += 1;
    }
    Ok(n)
}

/// A client of the memory for this bridge: it never refuses the person's address (they are warned instead).
pub fn client_for(url: &str, key: &str) -> Client {
    let mut c = Client::new(url, key);
    c.allow_clear = true;
    c
}

/// Whether `url` sends the key in clear outside this machine and the own network (HTTP to anywhere else): to warn about.
pub fn in_clear(url: &str) -> bool {
    !memcp_client::allowed_url(url)
}

impl Intake {
    /// Opens the outbox (making its folder, private to the user, if it is not there).
    /// An address that is neither HTTPS nor of the own network is taken, never refused: [`Intake::in_clear`] says so, and the
    /// app or the command line warns the person (the key would travel in clear).
    pub fn open(config: Config) -> Result<Intake, Error> {
        let outbox = Outbox::open(&config.outbox)?;
        Ok(Intake {
            config,
            outbox,
            flushing: Mutex::new(()),
        })
    }

    /// The key would travel in clear (an HTTP address outside this machine and the own network): to warn about.
    pub fn in_clear(&self) -> bool {
        in_clear(&self.config.url)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn entry(&self, kind: Kind, title: &str, bytes: u64, payload: Value) -> Entry {
        let created_at = now();
        Entry {
            id: Outbox::new_id(created_at),
            kind,
            title: title.chars().take(60).collect(),
            created_at,
            bytes,
            attempts: 0,
            state: State::Queued,
            detail: String::new(),
            sent_at: 0,
            payload,
        }
    }

    /// A note. It starts with the date, in the user's day, and goes into the inbox as it is written.
    pub fn submit_text(&self, text: &str) -> Result<String, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("a note is not empty".into()));
        }
        let (y, m, d, _, _) = time::civil(now(), self.config.utc_offset_minutes);
        let stamped = format!("{y:04}-{m:02}-{d:02}, {text}");
        let texts = split(&stamped, NOTE_MAX);
        let e = self.entry(
            Kind::Text,
            text,
            u64::try_from(stamped.len()).unwrap_or(0),
            json!({ "texts": texts }),
        );
        self.outbox.put(&e, None)?;
        Ok(e.id)
    }

    /// A file (a document, or a recording if its extension says so). It is **copied** into the outbox:
    /// the original may be moved or deleted at once.
    pub fn submit_file(&self, path: &Path, note: &str) -> Result<String, Error> {
        let audio = AUDIO_EXTENSIONS.contains(&extension(path).as_str());
        let name = clean(
            &path
                .file_name()
                .map_or(String::new(), |n| n.to_string_lossy().into()),
        );
        if name.is_empty() {
            return Err(Error::Invalid("a file needs a name".into()));
        }
        let (kind, tag, default_note) = if audio {
            (Kind::Audio, "inbox/audio", "Enregistrement déposé")
        } else {
            (Kind::Document, "inbox", "Fichier déposé")
        };
        let note = if note.trim().is_empty() {
            default_note
        } else {
            note.trim()
        };
        self.file(kind, &name, note, tag, path)
    }

    /// One recording, or one part of a long one. `started_at` is Unix time, `seconds` its length;
    /// `part` of `parts` is `1` of `1` for a recording in one piece.
    pub fn submit_audio(
        &self,
        path: &Path,
        started_at: i64,
        seconds: u32,
        title: &str,
        part: u8,
        parts: u8,
    ) -> Result<String, Error> {
        let all = Heard {
            send_audio: true,
            ..Heard::default()
        };
        self.submit_audio_heard(path, started_at, seconds, title, part, parts, &all)
    }

    /// [`Intake::submit_audio`] with what this machine heard in it: the text goes with the recording, as notes of the
    /// inbox that name the file, so the memory's side does not transcribe it again. Empty: the memory's side does.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_audio_heard(
        &self,
        path: &Path,
        started_at: i64,
        seconds: u32,
        title: &str,
        part: u8,
        parts: u8,
        heard: &Heard,
    ) -> Result<String, Error> {
        if parts == 0 || part == 0 || part > parts {
            return Err(Error::Invalid(format!(
                "part {part} of {parts} does not exist"
            )));
        }
        let (y, m, d, hh, mm) = time::civil(started_at, self.config.utc_offset_minutes);
        let title = clean(title);
        let title = if title.is_empty() {
            "dictee".to_string()
        } else {
            title
        };
        let ext = match extension(path).as_str() {
            "" => "m4a".to_string(),
            e => e.to_string(),
        };
        let piece = if parts > 1 {
            format!("-part-{part}-of-{parts}")
        } else {
            String::new()
        };
        let name = format!("{y:04}-{m:02}-{d:02}-{hh:02}h{mm:02}-{title}{piece}.{ext}");
        let more = if parts > 1 {
            format!(" (partie {part}/{parts})")
        } else {
            String::new()
        };
        let note = format!("Dictée du {y:04}-{m:02}-{d:02} à {hh:02}h{mm:02}, {seconds} s{more}");
        let text = heard.text.trim();
        let summary = heard.summary.trim();
        let mut notes: Vec<Value> = Vec::new();
        if heard.send_text && !text.is_empty() {
            // As the memory's side writes it (voice.rs): the head names the file, so the recording counts as transcribed.
            let head = format!(
                "Dictée du {y:04}-{m:02}-{d:02} à {hh:02}h{mm:02}, {seconds} s, audio {name}"
            );
            let pieces = split(
                text,
                NOTE_MAX.saturating_sub(head.chars().count() + 30).max(200),
            );
            let n = pieces.len();
            for (i, p) in pieces.iter().enumerate() {
                let of = if n > 1 {
                    format!(" (partie {}/{n})", i + 1)
                } else {
                    String::new()
                };
                notes.push(json!({ "text": format!("{head}{of} : {p}"), "tags": ["inbox", "inbox/dictee"] }));
            }
        }
        if heard.send_summary && !summary.is_empty() {
            // Not naming the file: a summary alone must not stop the memory's side from transcribing it.
            let head = format!(
                "Résumé de l'enregistrement du {y:04}-{m:02}-{d:02} à {hh:02}h{mm:02}{more}"
            );
            for p in split(
                summary,
                NOTE_MAX.saturating_sub(head.chars().count() + 10).max(200),
            ) {
                notes.push(
                    json!({ "text": format!("{head} : {p}"), "tags": ["inbox", "inbox/resume"] }),
                );
            }
        }
        let keep = json!({ "notes": notes, "heard": text, "summary": summary });
        if !heard.send_audio {
            // The sound stays here: what is said goes as notes, and the item keeps its record.
            return self.notes_only(&name, keep);
        }
        self.file_with(Kind::Audio, &name, &note, "inbox/audio", path, keep)
    }

    /// A recording whose sound is not sent: its notes only, as one item that says what it was.
    fn notes_only(&self, name: &str, keep: Value) -> Result<String, Error> {
        let mut payload = keep;
        payload["name"] = json!(name);
        let e = self.entry(Kind::Audio, name, 0, payload);
        self.outbox.put(&e, None)?;
        Ok(e.id)
    }

    fn file(
        &self,
        kind: Kind,
        name: &str,
        note: &str,
        tag: &str,
        path: &Path,
    ) -> Result<String, Error> {
        self.file_with(kind, name, note, tag, path, json!({}))
    }

    fn file_with(
        &self,
        kind: Kind,
        name: &str,
        note: &str,
        tag: &str,
        path: &Path,
        more: Value,
    ) -> Result<String, Error> {
        let size = std::fs::metadata(path)
            .map_err(|e| Error::Io(e.to_string()))?
            .len();
        let limit = if kind == Kind::Audio {
            MAX_AUDIO
        } else {
            MAX_FILE
        };
        if size == 0 {
            return Err(Error::Invalid("the file is empty".into()));
        }
        if size > limit {
            return Err(Error::TooBig(format!(
                "{name} is {} MB, the memory keeps {} MB at most",
                size.div_ceil(1024 * 1024),
                limit / 1024 / 1024
            )));
        }
        let bytes = std::fs::read(path).map_err(|e| Error::Io(e.to_string()))?;
        let mut payload = json!({ "name": name, "note": note, "tags": [tag] });
        if let (Some(p), Some(m)) = (payload.as_object_mut(), more.as_object()) {
            p.extend(m.clone());
        }
        let e = self.entry(kind, name, size, payload);
        self.outbox.put(&e, Some(&bytes))?;
        Ok(e.id)
    }

    /// A call of a tool of the memory, kept to be made when the memory can be reached. `file` is the bytes
    /// an argument `data_base64` will carry (so they are not written twice as text).
    pub fn queue_call(
        &self,
        tool: &str,
        args: &Value,
        file: Option<&[u8]>,
    ) -> Result<String, Error> {
        let bytes = file.map_or(0, |b| b.len() as u64);
        let e = self.entry(
            Kind::Call,
            tool,
            bytes,
            json!({ "tool": tool, "args": args }),
        );
        self.outbox.put(&e, file)?;
        Ok(e.id)
    }

    /// What waits (and what the memory refused for good), the oldest first, then the last 50 sent, the
    /// newest first.
    pub fn items(&self) -> Vec<Item> {
        let mut v: Vec<Item> = self.outbox.queue().iter().map(Entry::item).collect();
        v.extend(self.outbox.sent().iter().map(|e| {
            let mut i = e.item();
            i.audio = self
                .outbox
                .audio(&e.id)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            i
        }));
        v
    }

    /// How many items wait to be sent.
    pub fn waiting(&self) -> u32 {
        let n = self
            .outbox
            .queue()
            .iter()
            .filter(|e| e.state != State::Failed)
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Forgets an item: one that waits is dropped, one that went is taken off the list. Waits for a flush that is
    /// sending it to finish.
    pub fn forget(&self, id: &str) -> Result<(), Error> {
        if !Outbox::valid_id(id) {
            return Err(Error::Invalid(format!(
                "{id} is not an item of this outbox"
            )));
        }
        let _lock = self.outbox.lock(true)?;
        self.outbox.remove(id);
        Ok(())
    }

    /// Puts a refused item back in the queue, to be tried again (after the reason was dealt with). `None`: all of them.
    /// How many went back.
    pub fn retry(&self, id: Option<&str>) -> Result<u32, Error> {
        let _lock = self.outbox.lock(true)?;
        let mut n = 0;
        for mut e in self.outbox.queue() {
            if e.state == State::Failed && id.is_none_or(|i| i == e.id) {
                e.state = State::Queued;
                e.attempts = 0;
                e.detail = String::new();
                self.outbox.update(&e)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// The state of one item, if it is still known.
    pub fn item(&self, id: &str) -> Option<Item> {
        self.items().into_iter().find(|i| i.id == id)
    }

    /// Sends what waits, in order. Blocks while it does. See [`Flush`].
    pub fn flush(&self) -> Flush {
        let mut client = client_for(&self.config.url, &self.config.key);
        client.retry = Retry {
            tries: 3,
            first: Duration::from_secs(2),
            longest: Duration::from_secs(30),
        };
        self.flush_with(&mut client)
    }

    /// A flush that does not wait: one try, no pause. For a person or an assistant who is waiting for the
    /// answer; what could not go stays queued.
    pub fn flush_quick(&self) -> Flush {
        let mut client = client_for(&self.config.url, &self.config.key);
        client.retry = Retry {
            tries: 1,
            first: Duration::ZERO,
            longest: Duration::ZERO,
        };
        self.flush_with(&mut client)
    }

    /// [`Intake::flush`] with a client already made (its retries are the caller's).
    pub fn flush_with(&self, client: &mut Client) -> Flush {
        let mut report = Flush::default();
        let busy = |mut r: Flush| {
            r.busy = true;
            r.waiting = self.waiting();
            r
        };
        let Ok(_one) = self.flushing.try_lock() else {
            return busy(report);
        };
        let _lock = match self.outbox.lock(false) {
            Ok(Some(l)) => l,
            Ok(None) => return busy(report),
            Err(_) => {
                report.waiting = self.waiting();
                return report;
            }
        };
        // Until the memory has answered once in this flush, an answer that cannot be read is the network's (a captive
        // portal, a proxy's page), not a refusal: everything stays queued.
        let mut reached = false;
        for mut entry in self.outbox.queue() {
            if entry.state == State::Failed {
                continue;
            }
            match self.send(client, &mut entry) {
                Ok(()) => {
                    reached = true;
                    // A record that cannot be moved leaves the item queued: sent again, kept once.
                    if self.outbox.archive(&entry).is_err() {
                        break;
                    }
                    report.sent += 1;
                }
                Err(ClientError::Refused) => {
                    report.refused = true;
                    break;
                }
                Err(ClientError::Unreachable(_) | ClientError::RateLimited(_)) => {
                    report.unreachable = true;
                    break;
                }
                Err(ClientError::Unreadable(_)) if !reached => {
                    report.unreachable = true;
                    break;
                }
                Err(e @ ClientError::Tool(_)) => {
                    reached = true;
                    entry.state = State::Failed;
                    entry.detail = e.to_string();
                    if self.outbox.update(&entry).is_err() {
                        break;
                    }
                    report.failed += 1;
                }
                Err(e @ (ClientError::Unreadable(_) | ClientError::Rpc { .. })) => {
                    entry.attempts += 1;
                    entry.detail = format!("{e} (try {})", entry.attempts);
                    if entry.attempts >= ATTEMPTS {
                        entry.state = State::Failed;
                        report.failed += 1;
                    }
                    // Not counted if it cannot be written: stop rather than try forever.
                    if self.outbox.update(&entry).is_err() {
                        break;
                    }
                }
            }
        }
        report.waiting = self.waiting();
        report
    }

    fn send(&self, client: &mut Client, entry: &mut Entry) -> Result<(), ClientError> {
        let p = &entry.payload.clone();
        // No space is named: the key reaches only « À trier ». A capture made for a space, before, goes there too.
        match entry.kind {
            Kind::Text => {
                for text in p["texts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    client.call("remember", json!({ "text": text, "tags": ["inbox"] }))?;
                }
            }
            Kind::Document | Kind::Audio => {
                let only_notes = entry.kind == Kind::Audio && entry.bytes == 0;
                let bytes = if only_notes {
                    Vec::new()
                } else {
                    self.outbox
                        .blob(&entry.id)
                        .ok_or_else(|| ClientError::Tool("the file of this item is gone".into()))?
                };
                let tags: Vec<&str> = p["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                use base64::Engine as _;
                // Each step is kept as it is done: a send cut halfway resumes, the file is never put twice.
                if p["put"] != true && !only_notes {
                    let kept = client.call(
                        "file_put",
                        json!({
                            "name": p["name"].as_str().unwrap_or("file"),
                            "note": p["note"].as_str().unwrap_or(""),
                            "tags": tags,
                            "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                        }),
                    )?;
                    entry.payload["put"] = json!(true);
                    if let Some(id) = kept["file"]["id"].as_str() {
                        entry.payload["file_id"] = json!(id);
                    }
                    let _ = self.outbox.update(entry);
                }
                // The text names the recording by its id too, as the memory's side writes it (voice.rs): the
                // recording is then found and erased a week after its text was filed, wherever it was filed.
                let file_id = entry.payload["file_id"].as_str().map(String::from);
                let name = p["name"].as_str().unwrap_or("");
                let notes: Vec<Value> = p["notes"].as_array().cloned().unwrap_or_default();
                let done = usize::try_from(p["notes_sent"].as_u64().unwrap_or(0)).unwrap_or(0);
                for (i, n) in notes.iter().enumerate().skip(done) {
                    // A note of the first version of 5/11 was its text alone.
                    let (mut text, tags) = match n.as_str() {
                        Some(t) => (t.to_string(), json!(["inbox", "inbox/dictee"])),
                        None => (
                            n["text"].as_str().unwrap_or("").to_string(),
                            n["tags"].clone(),
                        ),
                    };
                    if let Some(id) = &file_id
                        && !name.is_empty()
                        && !text.contains(id.as_str())
                    {
                        let named = format!("audio {name}");
                        text = text.replacen(&named, &format!("{named} ({id})"), 1);
                    }
                    client.call("remember", json!({ "text": text, "tags": tags }))?;
                    entry.payload["notes_sent"] = json!(i + 1);
                    let _ = self.outbox.update(entry);
                }
            }
            Kind::Call => {
                let mut args = p["args"].clone();
                if let Some(bytes) = self.outbox.blob(&entry.id) {
                    use base64::Engine as _;
                    args["data_base64"] =
                        json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                }
                client.call(p["tool"].as_str().unwrap_or(""), args)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{clean, split};

    #[test]
    fn a_long_text_is_cut_at_a_word_and_nothing_is_lost() {
        let text = "mot ".repeat(2000);
        let parts = split(text.trim(), 100);
        assert!(parts.len() > 50);
        assert!(parts.iter().all(|p| p.chars().count() <= 100));
        assert_eq!(parts.join(" "), text.trim());
        let one_word = "x".repeat(250);
        let cut = split(&one_word, 100);
        assert_eq!(
            cut.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [100, 100, 50]
        );
        assert_eq!(split("court", 100), ["court"]);
        // A word of exactly the size, then a space: no empty part between.
        let exact = format!("{} {}", "a".repeat(100), "b".repeat(100));
        assert_eq!(split(&exact, 100).len(), 2, "{:?}", split(&exact, 100));
    }

    #[test]
    fn a_name_is_made_safe_without_losing_its_words() {
        assert_eq!(clean("../etc/passwd"), "..-etc-passwd");
        assert_eq!(clean("  Réunion   du  lundi\n"), "Réunion du lundi");
        assert_eq!(clean("a:b\\c"), "a-b-c");
        assert_eq!(clean("///"), "");
        // A long name keeps its extension.
        let long = format!("{}.pdf", "r".repeat(120));
        assert!(clean(&long).ends_with(".pdf") && clean(&long).chars().count() == 80);
    }
}
