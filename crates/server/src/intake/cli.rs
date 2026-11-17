//! The intake as commands: `memcp transcribe`, `memcp watch`, `memcp intake ...`. One pass now, by hand or from a script,
//! on the same settings the node's own background tasks use (`memcp.toml`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use memcp_core::Brain;
use memcp_core::text;
use serde_json::json;

use super::{Intake, Settings, watch};
use crate::cli::{Failure, bad, fail};
use crate::config::Settings as File;

/// A late recording: waiting longer than this (seconds).
const LATE: i64 = 30 * 60;

/// The intake of the memory in `data`, on the settings of the file. `user` names the person, over the file's.
pub fn open(data: &Path, file: &File, user: Option<&str>) -> Result<Intake, Failure> {
    let brain = Brain::open(data).map_err(fail)?;
    text::set_language(brain.language().map_err(fail)?);
    let mut settings = Settings::from_file(file, &data.join("intake")).map_err(|e| (78, e))?;
    if let Some(u) = user {
        settings.person = Some(u.to_string());
    }
    Intake::new(&brain, settings).map_err(|e| (1, e.to_string()))
}

fn speech_is_set(i: &Intake) -> Result<(), Failure> {
    let v = &i.settings.voice;
    if v.api_url.is_none() && v.whisper_model.as_os_str().is_empty() {
        return Err((
            78,
            "no speech service and no speech model: set transcribe_api_url or transcribe_model in memcp.toml".into(),
        ));
    }
    Ok(())
}

/// `memcp transcribe`: one pass over the recordings waiting in the inboxes.
pub fn transcribe(data: &Path, file: &File, user: Option<&str>) -> Result<(), Failure> {
    let mut intake = open(data, file, user)?;
    speech_is_set(&intake)?;
    let r = intake.transcribe().map_err(|e| (1, e.to_string()))?;
    sayln!(
        "recordings read: {}, nothing audible: {}, erased: {}",
        r.transcribed, r.nothing_audible, r.erased
    );
    for p in &r.problems {
        esayln!("  {p}");
    }
    if r.problems.is_empty() {
        Ok(())
    } else {
        Err((3, format!("{} problem(s)", r.problems.len())))
    }
}

fn print_scans(scans: &[super::Scanned]) -> u32 {
    let mut problems = 0;
    for s in scans {
        match &s.report {
            Ok(r) => {
                sayln!(
                    "{} -> {}: {} note(s), {} file(s), {} still being written",
                    s.dir.display(),
                    s.space,
                    r.notes,
                    r.files,
                    r.waiting
                );
                for p in &r.problems {
                    problems += 1;
                    esayln!("  {p}");
                }
            }
            Err(e) => {
                problems += 1;
                esayln!("{}: {e}", s.dir.display());
            }
        }
    }
    problems
}

/// `memcp watch [--once]`: looks at the watched folders (every `watch_every` seconds until stopped, or once).
pub fn watch_folders(
    data: &Path,
    file: &File,
    user: Option<&str>,
    once: bool,
) -> Result<(), Failure> {
    let mut intake = open(data, file, user)?;
    if intake.settings.watch.is_empty() {
        return Err((
            78,
            "no folder is watched: add `watch = [\"FOLDER -> SPACE\"]` to memcp.toml".into(),
        ));
    }
    loop {
        let scans = intake.watch();
        let problems = print_scans(&scans);
        if once {
            return if problems == 0 {
                Ok(())
            } else {
                Err((3, format!("{problems} problem(s)")))
            };
        }
        std::thread::sleep(Duration::from_secs(intake.settings.watch_every.max(1)));
    }
}

/// `memcp intake send|text|absorb|status|discover`.
pub fn intake(
    data: &Path,
    file: &File,
    user: Option<&str>,
    pos: &[String],
    space: Option<&str>,
    seconds: Option<&str>,
) -> Result<(), Failure> {
    let sub = pos.get(1).map(String::as_str).unwrap_or_default();
    match sub {
        "send" => {
            let files: Vec<PathBuf> = pos[2..].iter().map(PathBuf::from).collect();
            if files.is_empty() {
                return Err(bad("intake send FILE... [--space SPACE]"));
            }
            let mut intake = open(data, file, user)?;
            let space = target(&mut intake, space)?;
            let offset = intake.settings.utc_offset_minutes;
            let mut refused = 0;
            for f in files {
                match watch::put_file(intake.memory(), &f, &space, offset) {
                    Ok(r) => sayln!(
                        "{}: kept in {space} ({} note(s), {} file(s))",
                        f.display(),
                        r.notes,
                        r.files
                    ),
                    Err(e) => {
                        refused += 1;
                        esayln!("{}: {e}", f.display());
                    }
                }
            }
            if refused == 0 {
                Ok(())
            } else {
                Err((3, format!("{refused} file(s) refused")))
            }
        }
        "text" => {
            let text = match pos.get(2).map(String::as_str) {
                Some("-") | None => {
                    let mut t = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).map_err(fail)?;
                    t
                }
                Some(_) => pos[2..].join(" "),
            };
            if text.trim().is_empty() {
                return Err(bad("intake text TEXT | - [--space SPACE]"));
            }
            let mut intake = open(data, file, user)?;
            let space = target(&mut intake, space)?;
            intake
                .memory()
                .call(
                    "remember",
                    json!({ "space": space, "tags": ["inbox"], "text": text.trim() }),
                )
                .map_err(|e| (3, e.to_string()))?;
            sayln!("kept in {space}");
            Ok(())
        }
        "absorb" => {
            let mut intake = open(data, file, user)?;
            if intake.settings.absorb.is_empty() && intake.settings.deliver.is_empty() {
                return Err((
                    78,
                    "no folder is absorbed: add `absorb = [\"FOLDER -> SPACE\"]` to memcp.toml"
                        .into(),
                ));
            }
            let everything = pos.get(2).is_some_and(|w| w == "all");
            let mut problems = 0;
            for (label, result) in intake.absorb(everything) {
                match result {
                    Ok(r) => {
                        sayln!(
                            "{label}: {} passage(s) sent, {} file(s) read, {} only noted",
                            r.sent, r.files_read, r.noted
                        );
                        for p in &r.problems {
                            problems += 1;
                            esayln!("  {p}");
                        }
                    }
                    Err(e) => {
                        problems += 1;
                        esayln!("{label}: {e}");
                    }
                }
            }
            if problems == 0 {
                Ok(())
            } else {
                Err((3, format!("{problems} problem(s)")))
            }
        }
        "status" => {
            let mut intake = open(data, file, user)?;
            let waiting = intake.waiting().map_err(|e| (1, e.to_string()))?;
            if waiting.is_empty() {
                sayln!("no recording waits for a text");
            }
            for (space, n, age) in waiting {
                sayln!(
                    "{space}: {n} recording(s) waiting, the oldest for {} min{}",
                    age / 60,
                    if age > LATE { "  LATE" } else { "" }
                );
            }
            for w in &intake.settings.watch {
                sayln!(
                    "watching {} for {}",
                    w.dir.display(),
                    w.space.as_deref().unwrap_or("Inbox")
                );
            }
            Ok(())
        }
        "discover" => {
            let seconds = seconds.and_then(|s| s.parse::<u64>().ok()).unwrap_or(3);
            for f in super::discover::discover(Duration::from_secs(seconds)) {
                sayln!("{}\t{}\t{}\t{}", f.name, f.address, f.version, f.via);
            }
            Ok(())
        }
        _ => Err(bad("intake send|text|absorb|status|discover")),
    }
}

fn target(intake: &mut Intake, space: Option<&str>) -> Result<String, Failure> {
    match space {
        Some(s) => Ok(s.to_string()),
        None => intake
            .memory()
            .triage_space()
            .map_err(|e| (1, e.to_string())),
    }
}
