#![warn(missing_docs)]
//! The node's helpers, as the apps' Swift sees them. Nothing but a shell: the logic is the node's (`memcp-server`, its
//! `intake` module, and `memcp-client` for joining by code), renamed for Swift.
//!
//! What this crate offers:
//!
//! - **Joining by code**: [`pair`], [`address_allowed`] (and the [`Pairing`] and [`PairError`] they use).
//! - **The browser extension's link** (the Meet or Teams roster): [`BrowserLink`], [`BrowserCall`],
//!   [`browser_extension_id`].
//! - **The speech model of a Mac** and the rules around it: [`transcribe`] (macOS), [`should_transcribe`], the
//!   `summary_*` prompts.
//! - **The node's own intake**, new: [`node::NodeIntake`], voice notes read by the node itself and folders it watches.
//!
//! The names above are frozen for the apps that already link them (the pairing's `vaults` field stays, deprecated, next to
//! `spaces`); the old outbox, the local relay and the Markdown writers are gone, since an app embeds the node and writes
//! into it directly.

#[cfg(target_os = "macos")]
use std::path::Path;
use std::sync::Arc;

pub mod node;

uniffi::setup_scaffolding!();

/// What pairing gave: the connection's address and key (put the key in the Keychain at once). `spaces` names the one
/// box the key writes to, the person's « À trier » (`<person>-a-trier`): there is nothing to choose.
#[derive(Clone, uniffi::Record)]
pub struct Pairing {
    /// Name of the new connection (`peer-...`).
    pub agent: String,
    /// Address of the connection (`https://<server>/mcp/<agent>`).
    pub url: String,
    /// The key, shown once: put it in the Keychain at once.
    pub key: String,
    /// The same list as `spaces`, under its old name. Deprecated: read `spaces`.
    pub vaults: Vec<String>,
    /// The one box the key writes to (the person's inbox); never a space to choose.
    pub spaces: Vec<String>,
    /// The key would travel in clear (HTTP outside this machine and the own network): warn the person, never refuse.
    pub in_clear: bool,
}

/// Why pairing failed, in plain words.
#[derive(Debug, uniffi::Error)]
pub enum PairError {
    /// The code is unknown, used or expired, or the address is refused: the person must check it.
    /// Plain words for the person.
    Refused {
        /// Plain words for the person.
        message: String,
    },
    /// Too many tries from this address: wait a minute.
    /// Plain words for the person.
    Wait {
        /// Plain words for the person.
        message: String,
    },
    /// The server cannot be reached, or answered something else.
    /// Plain words for the person.
    Unreachable {
        /// Plain words for the person.
        message: String,
    },
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

/// Pairs this device with a memory: `server` is the address the person's page shows, `code` the one-time code it gave,
/// `device` this computer's name (it names the connection). **Blocks** (a network call, 30 s at most): call it from a
/// background task. Once only per code.
#[uniffi::export]
pub fn pair(server: String, code: String, device: String) -> Result<Pairing, PairError> {
    use memcp_client::Error as E;
    match memcp_client::pair_allowing(&server, &code, &device, true) {
        Ok(p) => Ok(Pairing {
            in_clear: !memcp_client::allowed_url(&p.url),
            agent: p.agent,
            url: p.url,
            key: p.key,
            vaults: p.spaces.clone(),
            spaces: p.spaces,
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

/// Whether this address keeps the key safe: HTTPS anywhere, plain HTTP only to this machine or the own network. For a
/// warning on the setup screen: nothing is refused for it.
#[uniffi::export]
pub fn address_allowed(url: String) -> bool {
    memcp_client::allowed_url(&url)
}

/// An error the app can show as it is; `message` is in plain words.
#[derive(Debug, uniffi::Error)]
pub enum IntakeError {
    /// Something on this computer could not be written.
    /// Plain words for the person.
    Storage {
        /// Plain words for the person.
        message: String,
    },
    /// What was given cannot be worked on, or the address is not allowed.
    /// Plain words for the person.
    Invalid {
        /// Plain words for the person.
        message: String,
    },
    /// Too big for the memory, which would refuse it.
    /// Plain words for the person.
    TooBig {
        /// Plain words for the person.
        message: String,
    },
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

impl From<memcp_server::intake::Error> for IntakeError {
    fn from(e: memcp_server::intake::Error) -> Self {
        let message = e.to_string();
        match e {
            memcp_server::intake::Error::Io(_) => IntakeError::Storage { message },
            memcp_server::intake::Error::Invalid(_) => IntakeError::Invalid { message },
        }
    }
}

/// The browser extension's link (Google Meet, Teams on the web): who is in the call and who speaks, heard from the
/// extension only, on `127.0.0.1:8790`. Stops with `stop()` or when the app lets it go.
#[derive(uniffi::Object)]
pub struct BrowserLink {
    link: std::sync::Mutex<memcp_server::intake::browser::BrowserLink>,
}

/// The call the extension reports now.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BrowserCall {
    /// `meet` or `teams`.
    pub platform: String,
    /// Title of the call, as the page shows it.
    pub title: String,
    /// Where the call is (the Meet code, or the Teams title): another room is another meeting.
    pub room: String,
}

#[uniffi::export]
impl BrowserLink {
    /// Listens before it returns: a port already taken is an error at once.
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<BrowserLink>, IntakeError> {
        let link = memcp_server::intake::browser::BrowserLink::start(
            memcp_server::intake::browser::EXTENSION_PORT,
        )?;
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

    /// Stops listening (idempotent); the port is free afterwards.
    pub fn stop(&self) {
        if let Ok(mut l) = self.link.lock() {
            l.stop();
        }
    }
}

/// The extension's id, fixed by its key: the one the app hears.
#[uniffi::export]
pub fn browser_extension_id() -> String {
    memcp_server::intake::browser::EXTENSION_ID.to_string()
}

/// What a recording says, read on this Mac by Whisper (on its GPU). `pcm`: 16 kHz, mono, 32-bit float samples,
/// little-endian, no header. Empty for nothing audible. **Blocks** for a while: call it from a background task.
/// (The node reads the recordings kept in a memory by itself: see [`node::NodeIntake`].)
#[cfg(target_os = "macos")]
#[uniffi::export]
pub fn transcribe(
    model: String,
    pcm: String,
    lexicon: Option<String>,
) -> Result<String, IntakeError> {
    let threads =
        std::thread::available_parallelism().map_or(4, |n| u32::try_from(n.get()).unwrap_or(4));
    Ok(memcp_server::intake::local::transcribe(
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
    memcp_server::intake::plan::should_transcribe(on_mains, battery_percent, low_power)
}

/// What the summarising model of the system is told, once per session.
#[uniffi::export]
pub fn summary_instructions() -> String {
    memcp_server::intake::plan::SUMMARY_INSTRUCTIONS.to_string()
}

/// The requests to make, one per piece of the transcript, in order.
#[uniffi::export]
pub fn summary_prompts(text: String) -> Vec<String> {
    let pieces = memcp_server::intake::plan::summary_pieces(&text);
    let n = pieces.len();
    pieces
        .iter()
        .enumerate()
        .map(|(i, p)| memcp_server::intake::plan::summary_prompt(p, i, n))
        .collect()
}

/// The request that joins the summaries of the pieces into one; `None` when there is only one.
#[uniffi::export]
pub fn summary_join_prompt(summaries: Vec<String>) -> Option<String> {
    memcp_server::intake::plan::summary_join_prompt(&summaries)
}
