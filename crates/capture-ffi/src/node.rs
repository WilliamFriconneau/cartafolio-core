//! The node's own intake for an app that embeds the node: the recordings kept in its memory are read by the node itself
//! (the speech model inside the program, on the GPU of a Mac; or a speech service by API), and the folders it watches
//! become notes and files. The app writes into its node (MCP, in the same process) and does not transcribe anything
//! itself: it only gives the node what only the app has, a way to decode a recording ([`NodeAudioDecoder`]: AVFoundation).
//!
//! All the logic is `memcp_server::intake`; this is the shell Swift calls.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use memcp_core::Brain;
use memcp_core::text;
use memcp_server::intake::whisper::{Decoder, VoiceSettings};
use memcp_server::intake::{self, Intake, Running, Settings, Watched};

/// An error the app can show as it is; `message` is in plain words.
#[derive(Debug, uniffi::Error)]
pub enum NodeIntakeError {
    /// The settings or the data folder cannot be used.
    /// Plain words for the person.
    Invalid {
        /// Plain words for the person.
        message: String,
    },
    /// The work failed (a file of the node, the memory).
    /// Plain words for the person.
    Failed {
        /// Plain words for the person.
        message: String,
    },
}

impl std::fmt::Display for NodeIntakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (NodeIntakeError::Invalid { message } | NodeIntakeError::Failed { message }) = self;
        write!(f, "{message}")
    }
}

impl std::error::Error for NodeIntakeError {}

impl From<uniffi::UnexpectedUniFFICallbackError> for NodeIntakeError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        NodeIntakeError::Failed {
            message: e.to_string(),
        }
    }
}

fn failed(e: impl std::fmt::Display) -> NodeIntakeError {
    NodeIntakeError::Failed {
        message: e.to_string(),
    }
}

/// What only the app can do: turn a recording (m4a, webm, wav...) into samples the speech model reads.
#[uniffi::export(with_foreign)]
pub trait NodeAudioDecoder: Send + Sync {
    /// The samples of the recording at `path`: 16 kHz, mono, 32-bit float. An error says why it cannot be read.
    fn decode(&self, path: String) -> Result<Vec<f32>, NodeIntakeError>;
}

/// A folder the node watches: a file added to it becomes a note or a file of `space`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NodeWatchedFolder {
    /// The folder.
    pub path: String,
    /// The space that gets what comes in; `None`: the person's Inbox.
    pub space: Option<String>,
}

/// What the node's intake is told. No `Debug`: it may hold the key of a speech service.
#[derive(Clone, uniffi::Record)]
pub struct NodeIntakeSettings {
    /// The person the intake works for (their login name in the node); `None`: the first administrator.
    pub person: Option<String>,
    /// Read the recordings that wait in the inboxes.
    pub transcribe: bool,
    /// The spaces whose recordings are read; empty: every space the person writes to, one made later included.
    pub transcribe_spaces: Vec<String>,
    /// A `ggml-*.bin` whisper.cpp model: the recordings are read inside the program (needs the decoder), on the GPU of a
    /// Mac. `None`: the speech service below, else `whisper-cli` and `ffmpeg` of the machine.
    pub model: Option<String>,
    /// The language of the recordings (`fr`, `en`); empty lets the model find it.
    pub language: String,
    /// Threads the speech model uses; 0: 4.
    pub threads: u32,
    /// A Markdown table of terms and their wrong spellings, to help and to correct the model.
    pub lexicon: Option<String>,
    /// A speech service of the OpenAI kind (`.../v1/audio/transcriptions`): the recording leaves this machine for it.
    pub api_url: Option<String>,
    /// The model asked at that service; empty: `whisper-1`.
    pub api_model: String,
    /// The key of that service (kept by the app, handed over for the length of the run).
    pub api_key: Option<String>,
    /// Seconds between two passes over the recordings; 0: 60.
    pub transcribe_every_secs: u64,
    /// The folders watched.
    pub watch: Vec<NodeWatchedFolder>,
    /// Seconds between two looks at the watched folders; 0: 10.
    pub watch_every_secs: u64,
    /// Minutes east of UTC (`TimeZone.current.secondsFromGMT() / 60`), for the dates of the notes.
    pub utc_offset_minutes: i32,
}

/// How a pass over the recordings went.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NodeVoiceReport {
    /// Recordings turned into a note.
    pub transcribed: u32,
    /// Recordings with nothing audible: they left the inbox.
    pub nothing_audible: u32,
    /// Recordings erased after their text was filed.
    pub erased: u32,
    /// What went wrong, in plain words.
    pub problems: Vec<String>,
}

/// How a look at one watched folder went.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NodeWatchReport {
    /// The folder.
    pub path: String,
    /// The space that got what came in.
    pub space: String,
    /// Notes kept.
    pub notes: u32,
    /// Files and recordings kept.
    pub files: u32,
    /// Still being written: next time.
    pub waiting: u32,
    /// What went wrong, in plain words.
    pub problems: Vec<String>,
}

/// Recordings that wait for a text in one space.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NodeRecordingsWaiting {
    /// The space.
    pub space: String,
    /// How many.
    pub count: u64,
    /// How long the oldest has waited, in seconds.
    pub oldest_age_secs: i64,
}

struct AppDecoder(Arc<dyn NodeAudioDecoder>);

fn settings_of(
    data: &Path,
    s: &NodeIntakeSettings,
    decoder: Option<Arc<dyn NodeAudioDecoder>>,
) -> Settings {
    let mut voice = VoiceSettings {
        whisper_lang: s.language.clone(),
        whisper_threads: if s.threads == 0 { 4 } else { s.threads },
        whisper_model: s.model.clone().map(PathBuf::from).unwrap_or_default(),
        lexicon: s.lexicon.clone().map(PathBuf::from),
        api_url: s.api_url.clone().filter(|u| !u.is_empty()),
        api_key: s.api_key.clone().filter(|k| !k.is_empty()),
        ..VoiceSettings::default()
    };
    if !s.api_model.is_empty() {
        voice.api_model = s.api_model.clone();
    }
    if s.model.is_some() && voice.api_url.is_none() {
        // The speech model runs inside the program, from the samples the app decoded.
        if let Some(d) = decoder {
            let app = AppDecoder(d);
            voice.decoder = Some(Decoder(Arc::new(move |p: &Path| {
                app.0
                    .decode(p.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })));
        }
    }
    Settings {
        person: s.person.clone(),
        transcribe: s.transcribe,
        transcribe_spaces: s.transcribe_spaces.clone(),
        voice,
        voice_every: if s.transcribe_every_secs == 0 {
            60
        } else {
            s.transcribe_every_secs
        },
        watch: s
            .watch
            .iter()
            .map(|w| Watched {
                dir: PathBuf::from(&w.path),
                space: w.space.clone(),
            })
            .collect(),
        watch_every: if s.watch_every_secs == 0 {
            10
        } else {
            s.watch_every_secs
        },
        absorb: vec![],
        deliver: vec![],
        absorb_every: 15 * 60,
        state_dir: data.join("intake"),
        utc_offset_minutes: s.utc_offset_minutes,
    }
}

/// The intake of one node, on its data folder.
#[derive(uniffi::Object)]
pub struct NodeIntake {
    brain: Brain,
    settings: Settings,
    by_hand: Mutex<Intake>,
    running: Mutex<Option<Running>>,
    folders: Mutex<Option<memcp_server::folder_sync::FolderTask>>,
}

#[uniffi::export]
impl NodeIntake {
    /// Opens the intake of the node whose data folder is `data_dir`. `decoder` is needed when `settings.model` names a
    /// speech model that runs inside the program.
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        settings: NodeIntakeSettings,
        decoder: Option<Arc<dyn NodeAudioDecoder>>,
    ) -> Result<Arc<NodeIntake>, NodeIntakeError> {
        let data = PathBuf::from(&data_dir);
        let brain = Brain::open(&data).map_err(|e| NodeIntakeError::Invalid {
            message: format!("{data_dir}: {e}"),
        })?;
        text::set_language(brain.language().map_err(failed)?);
        let settings = settings_of(&data, &settings, decoder);
        let by_hand = Intake::new(&brain, settings.clone()).map_err(failed)?;
        Ok(Arc::new(NodeIntake {
            brain,
            settings,
            by_hand: Mutex::new(by_hand),
            running: Mutex::new(None),
            folders: Mutex::new(None),
        }))
    }

    /// Starts the background tasks: the recordings are read, the folders are watched, until `stop()`. Idempotent.
    pub fn start(&self) -> Result<(), NodeIntakeError> {
        let mut running = self.running.lock().map_err(failed)?;
        if running.is_none() {
            *running = intake::start(&self.brain, self.settings.clone()).map_err(failed)?;
        }
        // The folders of the spaces on this node (see docs/FOLDERS.md): kept in step with the memory.
        let mut folders = self.folders.lock().map_err(failed)?;
        if folders.is_none() {
            *folders = memcp_server::folder_sync::start(&self.brain);
        }
        Ok(())
    }

    /// Stops the background tasks and waits for the pass in progress (idempotent).
    pub fn stop(&self) {
        if let Ok(mut running) = self.running.lock()
            && let Some(r) = running.take()
        {
            r.stop();
        }
        if let Ok(mut folders) = self.folders.lock()
            && let Some(f) = folders.take()
        {
            f.stop();
        }
    }

    /// One pass over the recordings waiting in the inboxes, now. **Blocks** (a recording is read in real time or faster):
    /// call it from a background task.
    pub fn transcribe_now(&self) -> Result<NodeVoiceReport, NodeIntakeError> {
        let r = self
            .by_hand
            .lock()
            .map_err(failed)?
            .transcribe()
            .map_err(failed)?;
        Ok(NodeVoiceReport {
            transcribed: r.transcribed,
            nothing_audible: r.nothing_audible,
            erased: r.erased,
            problems: r.problems,
        })
    }

    /// One look at every watched folder, now.
    pub fn watch_now(&self) -> Result<Vec<NodeWatchReport>, NodeIntakeError> {
        let scans = self.by_hand.lock().map_err(failed)?.watch();
        Ok(scans
            .into_iter()
            .map(|s| {
                let path = s.dir.to_string_lossy().into_owned();
                match s.report {
                    Ok(r) => NodeWatchReport {
                        path,
                        space: s.space,
                        notes: r.notes,
                        files: r.files,
                        waiting: r.waiting,
                        problems: r.problems,
                    },
                    Err(e) => NodeWatchReport {
                        path,
                        space: s.space,
                        notes: 0,
                        files: 0,
                        waiting: 0,
                        problems: vec![e.to_string()],
                    },
                }
            })
            .collect())
    }

    /// The recordings that wait for a text, per space.
    pub fn recordings_waiting(&self) -> Result<Vec<NodeRecordingsWaiting>, NodeIntakeError> {
        let waiting = self
            .by_hand
            .lock()
            .map_err(failed)?
            .waiting()
            .map_err(failed)?;
        Ok(waiting
            .into_iter()
            .map(|(space, count, oldest_age_secs)| NodeRecordingsWaiting {
                space,
                count,
                oldest_age_secs,
            })
            .collect())
    }
}
