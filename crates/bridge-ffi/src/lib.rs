//! The bridge, as the Mac app's Swift sees it. Nothing but a shell: every type and every call is the
//! bridge's own (`memcp-bridge`), renamed for Swift. The logic, the outbox and the network are Rust; the key
//! is read from the Keychain by Swift and handed over for the length of the run, never stored here.
//!
//! `Intake.flush()` blocks while it sends: call it from a background task, after each `submit…` and every
//! minute while the app runs.

use std::path::Path;
use std::sync::Arc;

use memcp_bridge::{Config, Intake as Core};

uniffi::setup_scaffolding!();

/// What the app knows about the connection and the place it keeps what waits. No `Debug`: it holds the key.
#[derive(Clone, uniffi::Record)]
pub struct IntakeConfig {
    /// The address of the connection: `https://<server>/mcp/<name>`.
    pub url: String,
    /// The connection's key, read from the Keychain.
    pub key: String,
    /// A folder of the app's own (Application Support); only what waits lives there.
    pub outbox: String,
    /// Minutes east of UTC where the user is (`TimeZone.current.secondsFromGMT() / 60`), for the dates.
    pub utc_offset_minutes: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ItemKind {
    Text,
    Document,
    Audio,
    /// A call of a tool of the memory kept by the bridge's gateway.
    Call,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ItemState {
    Queued,
    Sending,
    Sent,
    /// The memory refused it in its own words (see `detail`); not sent again unless `retry`.
    Failed,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct IntakeItem {
    pub id: String,
    pub kind: ItemKind,
    pub title: String,
    /// Unix time.
    pub created_at: i64,
    pub bytes: u64,
    pub state: ItemState,
    /// Why it failed, or how many tries it has had.
    pub detail: String,
    /// When the memory took it (Unix time), 0 while it waits.
    pub sent_at: i64,
    /// What this Mac heard in a recording; empty when the memory's side transcribes it.
    pub heard: String,
    /// The recording that went, kept on this Mac to be listened to again; empty when there is none.
    pub audio: String,
    /// Its summary, made on this Mac; empty when there is none.
    pub summary: String,
}

/// What this Mac made of a recording, and what of it goes to one memory.
#[derive(Debug, Clone, uniffi::Record)]
pub struct HeardInput {
    pub text: String,
    pub summary: String,
    pub send_audio: bool,
    pub send_text: bool,
    pub send_summary: bool,
}

/// How a `flush()` went.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct IntakeFlush {
    pub sent: u32,
    /// Still queued: they will be tried again.
    pub waiting: u32,
    /// Refused by the memory in this flush (listed with the reason until `retry` or `forget`).
    pub failed: u32,
    /// The memory could not be reached (or asked to wait): everything stays queued.
    pub unreachable: bool,
    /// The key is refused or revoked: **stop sending and ask the person for a new key.**
    pub refused: bool,
    /// Another flush was running (this app or the command line on the same outbox): nothing was tried now.
    pub busy: bool,
}

/// What pairing gave: the connection's address and key (put the key in the Keychain at once). `vaults` names the one
/// box the key writes to, the person's « À trier » (`<person>-a-trier`): it is no vault, and there is nothing to choose.
#[derive(Clone, uniffi::Record)]
pub struct Pairing {
    pub agent: String,
    pub url: String,
    pub key: String,
    pub vaults: Vec<String>,
    /// The key would travel in clear (HTTP outside this machine and the own network): warn the person, never refuse.
    pub in_clear: bool,
}

/// Why pairing failed, in plain words.
#[derive(Debug, uniffi::Error)]
pub enum PairError {
    /// The code is unknown, used or expired, or the address is refused: the person must check it.
    Refused { message: String },
    /// Too many tries from this address: wait a minute.
    Wait { message: String },
    /// The server cannot be reached, or answered something else.
    Unreachable { message: String },
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (PairError::Refused { message }
        | PairError::Wait { message }
        | PairError::Unreachable { message }) = self;
        write!(f, "{message}")
    }
}

impl std::error::Error for PairError {}

/// Pairs this device with a memory: `server` is the address the person's page shows ("Connect a bridge"), `code` the
/// one-time code it gave, `device` this computer's name (it names the connection). **Blocks** (a network call, 30 s at
/// most): call it from a background task. Once only per code.
#[uniffi::export]
pub fn pair(server: String, code: String, device: String) -> Result<Pairing, PairError> {
    use memcp_client::Error as E;
    match memcp_bridge::pair_allowing(&server, &code, &device, true) {
        Ok(p) => Ok(Pairing {
            in_clear: memcp_bridge::in_clear(&p.url),
            agent: p.agent,
            url: p.url,
            key: p.key,
            vaults: p.vaults,
        }),
        Err(E::Tool(m)) => Err(PairError::Refused { message: m }),
        Err(E::RateLimited(_)) => Err(PairError::Wait {
            message: "too many tries from this address: wait a minute".into(),
        }),
        Err(e) => Err(PairError::Unreachable {
            message: e.to_string(),
        }),
    }
}

/// An error the app can show as it is; `message` is in plain words.
#[derive(Debug, uniffi::Error)]
pub enum IntakeError {
    /// The outbox could not be written.
    Storage { message: String },
    /// What was given is not a capture, or the address is not allowed.
    Invalid { message: String },
    /// Too big for the memory, which would refuse it.
    TooBig { message: String },
}

impl std::fmt::Display for IntakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (IntakeError::Storage { message }
        | IntakeError::Invalid { message }
        | IntakeError::TooBig { message }) = self;
        write!(f, "{message}")
    }
}

impl std::error::Error for IntakeError {}

impl From<memcp_bridge::Error> for IntakeError {
    fn from(e: memcp_bridge::Error) -> Self {
        let message = e.to_string();
        match e {
            memcp_bridge::Error::Io(_) => IntakeError::Storage { message },
            memcp_bridge::Error::Invalid(_) => IntakeError::Invalid { message },
            memcp_bridge::Error::TooBig(_) => IntakeError::TooBig { message },
            memcp_bridge::Error::Folder(_) => IntakeError::Storage { message },
        }
    }
}

impl From<memcp_bridge::Item> for IntakeItem {
    fn from(i: memcp_bridge::Item) -> Self {
        IntakeItem {
            id: i.id,
            kind: match i.kind {
                memcp_bridge::Kind::Text => ItemKind::Text,
                memcp_bridge::Kind::Document => ItemKind::Document,
                memcp_bridge::Kind::Audio => ItemKind::Audio,
                memcp_bridge::Kind::Call => ItemKind::Call,
            },
            title: i.title,
            created_at: i.created_at,
            bytes: i.bytes,
            state: match i.state {
                memcp_bridge::State::Queued => ItemState::Queued,
                memcp_bridge::State::Sending => ItemState::Sending,
                memcp_bridge::State::Sent => ItemState::Sent,
                memcp_bridge::State::Failed => ItemState::Failed,
            },
            detail: i.detail,
            sent_at: i.sent_at,
            heard: i.heard,
            audio: i.audio,
            summary: i.summary,
        }
    }
}

/// The outbox and what sends from it. One per app, kept for as long as it runs.
#[derive(uniffi::Object)]
pub struct Intake {
    core: Core,
}

#[uniffi::export]
impl Intake {
    /// Opens the outbox (making its folder, private to the user, if it is not there).
    #[uniffi::constructor]
    pub fn new(config: IntakeConfig) -> Result<Arc<Intake>, IntakeError> {
        let core = Core::open(Config {
            url: config.url,
            key: config.key,
            outbox: config.outbox.into(),
            utc_offset_minutes: config.utc_offset_minutes,
        })?;
        Ok(Arc::new(Intake { core }))
    }

    /// A note, dated in the user's day. Kept before this returns. Returns its id.
    pub fn submit_text(&self, text: String) -> Result<String, IntakeError> {
        Ok(self.core.submit_text(&text)?)
    }

    /// A file or a recording (by its extension). **Copied** into the outbox: the original may be moved or
    /// deleted at once. Returns its id.
    pub fn submit_file(&self, path: String, note: String) -> Result<String, IntakeError> {
        Ok(self.core.submit_file(Path::new(&path), &note)?)
    }

    /// One recording, or one part of a long one: `part` of `parts` is 1 of 1 for a recording in one piece.
    /// `started_at` is Unix time, `seconds` its length. At most 24 MB a file: cut a long one into parts.
    /// `heard`: what this Mac made of it (`transcribe`, a summary) and what of it goes to this memory.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_audio(
        &self,
        path: String,
        started_at: i64,
        seconds: u32,
        title: String,
        part: u8,
        parts: u8,
        heard: HeardInput,
    ) -> Result<String, IntakeError> {
        let heard = memcp_bridge::Heard {
            text: heard.text,
            summary: heard.summary,
            send_audio: heard.send_audio,
            send_text: heard.send_text,
            send_summary: heard.send_summary,
        };
        Ok(self.core.submit_audio_heard(
            Path::new(&path),
            started_at,
            seconds,
            &title,
            part,
            parts,
            &heard,
        )?)
    }

    /// What waits (and what the memory refused for good), the oldest first, then the last 50 sent, the
    /// newest first.
    pub fn items(&self) -> Vec<IntakeItem> {
        self.core.items().into_iter().map(Into::into).collect()
    }

    /// How many captures wait to be sent.
    pub fn waiting(&self) -> u32 {
        self.core.waiting()
    }

    /// Sends what waits, in order. **Blocks** while it does: call it from a background task.
    pub fn flush(&self) -> IntakeFlush {
        let r = self.core.flush();
        IntakeFlush {
            sent: r.sent,
            waiting: r.waiting,
            failed: r.failed,
            unreachable: r.unreachable,
            refused: r.refused,
            busy: r.busy,
        }
    }

    /// The key would travel in clear (an HTTP address outside this machine and the own network): show a warning.
    pub fn in_clear(&self) -> bool {
        self.core.in_clear()
    }

    /// Forgets a capture: one that waits is dropped, one that went is taken off the list. Waits for a flush that is
    /// sending it.
    pub fn forget(&self, id: String) {
        let _ = self.core.forget(&id);
    }

    /// Puts a refused capture back in the queue (`id`), or all of them (`nil`). How many went back.
    pub fn retry(&self, id: Option<String>) -> Result<u32, IntakeError> {
        Ok(self.core.retry(id.as_deref())?)
    }
}

/// The memory's tools served on this Mac only (`POST http://127.0.0.1:<port>/mcp`), for an assistant that must never hold
/// the memory's address nor its key (a work assistant, say): the app keeps both, the assistant gets a local address.
/// It shares the connection's outbox with the app. Stops with `stop()` or when the app lets it go.
#[derive(uniffi::Object)]
pub struct LocalMcp {
    server: std::sync::Mutex<memcp_bridge::HttpServer>,
}

#[uniffi::export]
impl LocalMcp {
    /// Listens on `127.0.0.1:<port>` before it returns: a port already taken is an error at once.
    #[uniffi::constructor]
    pub fn new(config: IntakeConfig, port: u16) -> Result<Arc<LocalMcp>, IntakeError> {
        let gateway = memcp_bridge::Gateway::new(Config {
            url: config.url,
            key: config.key,
            outbox: config.outbox.into(),
            utc_offset_minutes: config.utc_offset_minutes,
        })?;
        let server = memcp_bridge::HttpServer::start(gateway, &format!("127.0.0.1:{port}"))?;
        Ok(Arc::new(LocalMcp {
            server: std::sync::Mutex::new(server),
        }))
    }

    pub fn stop(&self) {
        if let Ok(mut s) = self.server.lock() {
            s.stop();
        }
    }
}

/// The browser extension's link (Google Meet, Teams on the web): who is in the call and who speaks, heard from the
/// extension only, on `127.0.0.1:8790`. Stops with `stop()` or when the app lets it go.
#[derive(uniffi::Object)]
pub struct BrowserLink {
    link: std::sync::Mutex<memcp_bridge::BrowserLink>,
}

/// The call the extension reports now.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BrowserCall {
    /// `meet` or `teams`.
    pub platform: String,
    pub title: String,
    /// Where the call is (the Meet code, or the Teams title): another room is another meeting.
    pub room: String,
}

#[uniffi::export]
impl BrowserLink {
    /// Listens before it returns: a port already taken is an error at once.
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<BrowserLink>, IntakeError> {
        let link = memcp_bridge::BrowserLink::start(memcp_bridge::browser::EXTENSION_PORT)?;
        Ok(Arc::new(BrowserLink {
            link: std::sync::Mutex::new(link),
        }))
    }

    /// When the extension last said something, in Unix seconds; 0 if never since the app started.
    pub fn last_seen(&self) -> i64 {
        self.link.lock().map_or(0, |l| l.last_seen() / 1000)
    }

    /// The call in the browser now, if there is one.
    pub fn call(&self) -> Option<BrowserCall> {
        let c = self.link.lock().ok()?.call()?;
        Some(BrowserCall {
            platform: c.platform,
            title: c.title,
            room: c.room,
        })
    }

    /// Who was there between `from` and `to` (Unix seconds) and when each one spoke, as text to put before the
    /// transcript; empty when the extension saw nothing.
    pub fn roster(&self, from: i64, to: i64) -> String {
        self.link
            .lock()
            .map(|l| l.roster(from.saturating_mul(1000), to.saturating_mul(1000)))
            .unwrap_or_default()
    }

    pub fn stop(&self) {
        if let Ok(mut l) = self.link.lock() {
            l.stop();
        }
    }
}

/// The extension's id, fixed by its key: the one the app hears.
#[uniffi::export]
pub fn browser_extension_id() -> String {
    memcp_bridge::browser::EXTENSION_ID.to_string()
}

/// The outbox folder of one connection under `base` (the app's own data folder, Application Support): pass it as
/// `IntakeConfig.outbox`. Named after a fingerprint of the address, never the address: one connection never lists or
/// sends another's captures.
#[uniffi::export]
pub fn connection_outbox(base: String, url: String) -> String {
    memcp_bridge::connection_outbox(Path::new(&base), &url)
        .to_string_lossy()
        .into_owned()
}

/// Moves the captures left in the outbox the app used before (`old`, the folder that was `IntakeConfig.outbox`) into the
/// connection's own (`new`, from `connection_outbox`), all of them, whatever vault they were made for: they go to
/// « À trier ». Call it once at start, before `Intake(config:)`; calling it again moves nothing. How many moved.
#[uniffi::export]
pub fn adopt_old_captures(old: String, new: String) -> Result<u32, IntakeError> {
    Ok(memcp_bridge::adopt_old_captures(
        Path::new(&old),
        Path::new(&new),
    )?)
}

/// Whether this address keeps the key safe: HTTPS anywhere, plain HTTP only to this machine or the own network. For a
/// warning on the setup screen: nothing is refused for it.
#[uniffi::export]
pub fn address_allowed(url: String) -> bool {
    memcp_client::allowed_url(&url)
}

// MARK: - Markdown, for a folder of notes (Obsidian)

/// One line of the live text of a meeting or a dictation.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LiveLineInput {
    pub me: bool,
    pub text: String,
}

/// One part of a recording, as it is handed to the core.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MarkdownPart {
    pub path: String,
    pub seconds: u32,
}

fn markdown(folder: String, utc_offset_minutes: i32) -> memcp_bridge::markdown::Markdown {
    memcp_bridge::markdown::Markdown {
        folder: folder.into(),
        utc_offset_minutes,
    }
}

/// What a recording says, read on this Mac by Whisper (on its GPU). `pcm`: 16 kHz, mono, 32-bit float samples,
/// little-endian, no header. Empty for nothing audible. **Blocks** for a while: call it from a background task.
#[cfg(target_os = "macos")]
#[uniffi::export]
pub fn transcribe(
    model: String,
    pcm: String,
    lexicon: Option<String>,
) -> Result<String, IntakeError> {
    let threads =
        std::thread::available_parallelism().map_or(4, |n| u32::try_from(n.get()).unwrap_or(4));
    Ok(memcp_bridge::local::transcribe(
        Path::new(&model),
        Path::new(&pcm),
        "fr",
        threads,
        lexicon.as_deref().map(Path::new),
    )?
    .unwrap_or_default())
}

/// Whether this Mac reads a recording itself: on mains, or on a battery not low and not in Low Power Mode.
#[uniffi::export]
pub fn should_transcribe(on_mains: bool, battery_percent: u8, low_power: bool) -> bool {
    memcp_bridge::plan::should_transcribe(on_mains, battery_percent, low_power)
}

/// What the summarising model of the system is told, once per session.
#[uniffi::export]
pub fn summary_instructions() -> String {
    memcp_bridge::plan::SUMMARY_INSTRUCTIONS.to_string()
}

/// The requests to make, one per piece of the transcript, in order.
#[uniffi::export]
pub fn summary_prompts(text: String) -> Vec<String> {
    let pieces = memcp_bridge::plan::summary_pieces(&text);
    let n = pieces.len();
    pieces
        .iter()
        .enumerate()
        .map(|(i, p)| memcp_bridge::plan::summary_prompt(p, i, n))
        .collect()
}

/// The request that joins the summaries of the pieces; `None` for a single one (it is the summary).
#[uniffi::export]
pub fn summary_join_prompt(summaries: Vec<String>) -> Option<String> {
    memcp_bridge::plan::summary_join_prompt(&summaries)
}

/// A note as one dated `.md` in `folder` (a folder the person chose, often an Obsidian vault). Returns the file.
#[uniffi::export]
pub fn markdown_note(
    folder: String,
    utc_offset_minutes: i32,
    text: String,
    at: i64,
) -> Result<String, IntakeError> {
    Ok(markdown(folder, utc_offset_minutes)
        .note(&text, at)?
        .to_string_lossy()
        .into_owned())
}

/// A document or a recording copied into `folder`, with a `.md` beside it that embeds it.
#[uniffi::export]
pub fn markdown_file(
    folder: String,
    utc_offset_minutes: i32,
    path: String,
    note: String,
    at: i64,
) -> Result<String, IntakeError> {
    Ok(markdown(folder, utc_offset_minutes)
        .file(Path::new(&path), &note, at)?
        .to_string_lossy()
        .into_owned())
}

/// A meeting or a dictation: its parts copied and embedded, and its live text, said to be approximate.
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn markdown_meeting(
    folder: String,
    utc_offset_minutes: i32,
    title: String,
    started_at: i64,
    parts: Vec<MarkdownPart>,
    live: Vec<LiveLineInput>,
    heard: String,
    summary: String,
) -> Result<String, IntakeError> {
    let parts: Vec<(std::path::PathBuf, u32)> = parts
        .into_iter()
        .map(|p| (p.path.into(), p.seconds))
        .collect();
    let live: Vec<memcp_bridge::markdown::LiveLine> = live
        .into_iter()
        .map(|l| memcp_bridge::markdown::LiveLine {
            me: l.me,
            text: l.text,
        })
        .collect();
    Ok(markdown(folder, utc_offset_minutes)
        .meeting_heard(&title, started_at, &parts, &live, &heard, &summary)?
        .to_string_lossy()
        .into_owned())
}
