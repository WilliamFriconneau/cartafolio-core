//! The intake: what the node does with what comes into it, on its own.
//!
//! - **Voice notes.** A recording that reached an inbox with no text is read here, by a speech model on this machine
//!   (`whisper-cli` and `ffmpeg`) or by a speech service of the OpenAI kind; its text becomes a note of the same inbox.
//! - **Folders watched.** What is added to a folder becomes a note or a file of a space.
//! - **Folders absorbed.** A folder of notes kept up to date is read, and what is new in it goes to an inbox; the
//!   proposals of a space are put down as files in a folder.
//!
//! All of it is configured in `memcp.toml` (or the app's options) and runs as background tasks of `memcp serve` and
//! `memcp-manager serve`: there is no other program and no timer. The same passes are the commands `memcp transcribe`,
//! `memcp watch` and `memcp intake ...`, for a machine that wants one pass now. The node works on its own memory in
//! the same process, with the rights of the person it works for: no key, no address.

pub mod absorb;
pub mod browser;
pub mod cli;
pub mod discover;
#[cfg(feature = "local-whisper")]
pub mod local;
mod memory;
pub mod plan;
mod settings;
pub mod time;
pub mod voice;
pub mod watch;
pub mod whisper;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use memcp_core::Brain;

pub use memory::{AUTHOR, Failure, Memory};
pub use settings::{Settings, Watched, home_path};

/// What went wrong in the intake's own files.
#[derive(Debug)]
pub enum Error {
    /// A file of the intake could not be written.
    Io(String),
    /// What was given cannot be worked on.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(m) => write!(f, "a file of the intake cannot be written: {m}"),
            Error::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// The intake of one memory.
pub struct Intake {
    memory: Memory,
    pub settings: Settings,
}

/// A line of what a watched folder gave, for a person to read.
pub struct Scanned {
    pub dir: PathBuf,
    pub space: String,
    pub report: Result<watch::Report, Error>,
}

impl Intake {
    pub fn new(brain: &Brain, settings: Settings) -> Result<Intake, Failure> {
        let memory = Memory::open(brain, settings.person.as_deref())?;
        Ok(Intake { memory, settings })
    }

    pub fn memory(&mut self) -> &mut Memory {
        &mut self.memory
    }

    /// The spaces whose recordings are read: those named, else every space the person writes to.
    fn voice_spaces(&self) -> Vec<String> {
        if self.settings.transcribe_spaces.is_empty() {
            vec!["all".to_string()]
        } else {
            self.settings.transcribe_spaces.clone()
        }
    }

    /// One pass over the recordings waiting in the inboxes.
    pub fn transcribe(&mut self) -> Result<voice::VoiceReport, Error> {
        let spaces = self.voice_spaces();
        voice::transcribe(
            &mut self.memory,
            &spaces,
            &self.settings.voice,
            &self.settings.state_dir.join("voice"),
            self.settings.utc_offset_minutes,
        )
    }

    /// Recordings that wait for a text, per space: `(space, how many, age in seconds of the oldest)`.
    pub fn waiting(&mut self) -> Result<Vec<(String, u64, i64)>, Failure> {
        voice::waiting(&mut self.memory)
    }

    /// One look at every watched folder.
    pub fn watch(&mut self) -> Vec<Scanned> {
        let mut out = vec![];
        for w in self.settings.watch.clone() {
            let space = match &w.space {
                Some(s) => Ok(s.clone()),
                None => self.memory.triage_space(),
            };
            let report = match space {
                Ok(space) => {
                    let report = watch::scan(
                        &mut self.memory,
                        &w.dir,
                        &space,
                        &self.settings.state_dir.join("watch"),
                        self.settings.utc_offset_minutes,
                    );
                    Scanned {
                        dir: w.dir,
                        space,
                        report,
                    }
                }
                Err(e) => Scanned {
                    dir: w.dir,
                    space: w.space.unwrap_or_default(),
                    report: Err(Error::Invalid(e.to_string())),
                },
            };
            out.push(report);
        }
        out
    }

    /// One pass over the absorbed folders and the proposals to put down. `everything`: send what a folder holds on
    /// the first pass instead of only taking stock.
    pub fn absorb(&mut self, everything: bool) -> Vec<(String, Result<absorb::Absorbed, Error>)> {
        let state = self.settings.state_dir.join("absorb");
        let mut out = vec![];
        for src in self.settings.absorb.clone() {
            let label = format!("{} -> {}", src.dir.display(), src.space);
            out.push((
                label,
                absorb::absorb(&mut self.memory, &src, &state, everything),
            ));
        }
        for d in self.settings.deliver.clone() {
            let label = format!("{} {} -> {}", d.space, d.tag, d.dir.display());
            let mut report = absorb::Absorbed::default();
            absorb::propose_guide(&mut self.memory, &d, &state, &mut report);
            let result = absorb::deliver(&mut self.memory, &d, &state, &mut report)
                .map(|n| absorb::Absorbed { sent: n, ..report });
            out.push((label, result));
        }
        out
    }
}

/// The background tasks of a node, until [`Running::stop`] (or the end of the process).
pub struct Running {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    /// Asks the tasks to end and waits for the pass in progress.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Says a problem once, until it is gone: a folder that is not there is not told every ten seconds.
#[derive(Default)]
struct Said(BTreeSet<String>);

impl Said {
    fn tell(&mut self, source: &str, problems: &[String]) {
        let now: BTreeSet<String> = problems.iter().map(|p| format!("{source}: {p}")).collect();
        for line in now.difference(&self.0) {
            eprintln!("intake: {line}");
        }
        self.0.retain(|l| !l.starts_with(&format!("{source}: ")));
        self.0.extend(now);
    }
}

/// Starts the background tasks the settings ask for. `None` when there is nothing to run.
pub fn start(brain: &Brain, settings: Settings) -> Result<Option<Running>, Failure> {
    if settings.is_idle() {
        return Ok(None);
    }
    let mut intake = Intake::new(brain, settings)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("memcp-intake".into())
        .spawn(move || run(&mut intake, &flag))
        .map_err(|e| Failure(e.to_string()))?;
    Ok(Some(Running {
        stop,
        handle: Some(handle),
    }))
}

fn run(intake: &mut Intake, stop: &AtomicBool) {
    let started = Instant::now();
    let s = intake.settings.clone();
    // Due times: the folders at once, the recordings soon, the absorbed folders a little later.
    let mut next_watch = started;
    let mut next_voice = started + Duration::from_secs(5);
    let mut next_absorb = started + Duration::from_secs(30);
    let mut said = Said::default();
    if s.transcribe {
        eprintln!("intake: reading the voice notes");
    }
    for w in &s.watch {
        eprintln!(
            "intake: watching {} for {}",
            w.dir.display(),
            w.space.as_deref().unwrap_or("Inbox")
        );
    }
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        if !s.watch.is_empty() && now >= next_watch {
            for scanned in intake.watch() {
                let source = format!("watch {}", scanned.dir.display());
                match scanned.report {
                    Ok(r) => {
                        if r.notes + r.files > 0 {
                            eprintln!(
                                "intake: {} note(s) and {} file(s) from {} went to {}",
                                r.notes,
                                r.files,
                                scanned.dir.display(),
                                scanned.space
                            );
                        }
                        said.tell(&source, &r.problems);
                    }
                    Err(e) => said.tell(&source, &[e.to_string()]),
                }
            }
            next_watch = Instant::now() + Duration::from_secs(s.watch_every.max(1));
        }
        if s.transcribe && now >= next_voice {
            match intake.transcribe() {
                Ok(r) => {
                    if r.transcribed + r.nothing_audible + r.erased > 0 {
                        eprintln!(
                            "intake: voice notes read {}, silent {}, recordings erased {}",
                            r.transcribed, r.nothing_audible, r.erased
                        );
                    }
                    said.tell("voice", &r.problems);
                }
                Err(e) => said.tell("voice", &[e.to_string()]),
            }
            next_voice = Instant::now() + Duration::from_secs(s.voice_every.max(1));
        }
        if (!s.absorb.is_empty() || !s.deliver.is_empty()) && now >= next_absorb {
            for (label, result) in intake.absorb(false) {
                let source = format!("absorb {label}");
                match result {
                    Ok(r) => {
                        if r.sent > 0 {
                            eprintln!("intake: {} passage(s) absorbed ({label})", r.sent);
                        }
                        said.tell(&source, &r.problems);
                    }
                    Err(e) => said.tell(&source, &[e.to_string()]),
                }
            }
            next_absorb = Instant::now() + Duration::from_secs(s.absorb_every.max(60));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
