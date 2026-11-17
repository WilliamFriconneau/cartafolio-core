//! What the intake is told in `memcp.toml` (or by the add-on's options): the voice notes, the folders watched, the
//! folders absorbed. Read once, checked, and handed to [`crate::intake::Intake`].

use std::path::{Path, PathBuf};

use crate::config::Settings as File;

use super::absorb::{Delivery, Source};
use super::whisper::VoiceSettings;

/// Seconds between two looks at the watched folders, and between two passes over the recordings.
pub const WATCH_EVERY: u64 = 10;
pub const VOICE_EVERY: u64 = 60;

/// A folder watched: what is added to it is put into a space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watched {
    pub dir: PathBuf,
    /// The space that gets what comes in; `None`: the person's Inbox.
    pub space: Option<String>,
}

/// Everything the intake does, as set.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    /// The person the intake works for (a login name); `None`: the first administrator.
    pub person: Option<String>,
    pub transcribe: bool,
    /// The spaces whose recordings are read (`all`, or names); empty: the person's Inbox.
    pub transcribe_spaces: Vec<String>,
    pub voice: VoiceSettings,
    /// Seconds between two passes over the recordings.
    pub voice_every: u64,
    pub watch: Vec<Watched>,
    pub watch_every: u64,
    pub absorb: Vec<Source>,
    pub deliver: Vec<Delivery>,
    pub absorb_every: u64,
    /// Where the intake keeps what it remembers between passes.
    pub state_dir: PathBuf,
    /// Minutes east of UTC, for the dates of the notes.
    pub utc_offset_minutes: i32,
}

/// `~/x` is the home folder's.
pub fn home_path(v: &str) -> PathBuf {
    match v.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(v),
    }
}

/// `FOLDER -> SPACE` (the space may be left out for a watched folder).
fn arrow(line: &str) -> (&str, Option<&str>) {
    match line.split_once("->") {
        Some((a, b)) => (a.trim(), Some(b.trim()).filter(|s| !s.is_empty())),
        None => (line.trim(), None),
    }
}

/// The offset of the local time from UTC, in minutes, as the system says it (`date +%z`); 0 when it cannot be read.
pub fn local_offset_minutes() -> i32 {
    #[cfg(unix)]
    {
        if let Ok(o) = std::process::Command::new("date").arg("+%z").output() {
            let z = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if z.len() == 5
                && let (Some(sign), Ok(h), Ok(m)) = (
                    z.chars().next(),
                    z[1..3].parse::<i32>(),
                    z[3..5].parse::<i32>(),
                )
            {
                let total = h * 60 + m;
                return if sign == '-' { -total } else { total };
            }
        }
    }
    0
}

impl Settings {
    /// Reads the options of the file; `state_dir` is where the intake keeps its passes. An option that cannot work is an
    /// error naming it.
    pub fn from_file(file: &File, state_dir: &Path) -> Result<Settings, String> {
        let text = |k: &str| file.text(k).map(String::from);
        let person = text("intake_person")
            .or_else(|| text("transcribe_person"))
            .or_else(|| text("janitor_person"));
        let mut voice = VoiceSettings::default();
        if let Some(c) = text("transcribe_cmd") {
            voice.whisper_cmd = c;
        }
        if let Some(c) = text("transcribe_ffmpeg") {
            voice.ffmpeg_cmd = c;
        }
        if let Some(m) = text("transcribe_model") {
            voice.whisper_model = home_path(&m);
        }
        if let Some(n) = file.int("transcribe_threads") {
            voice.whisper_threads = u32::try_from(n).unwrap_or(8);
        }
        if let Some(l) = text("transcribe_lexicon") {
            voice.lexicon = Some(home_path(&l));
        }
        if let Some(l) = text("transcribe_language") {
            voice.whisper_lang = l;
        }
        voice.api_url = text("transcribe_api_url");
        voice.api_model = file.text_or_default("transcribe_api_model");
        if let Some(k) = text("transcribe_api_key_file") {
            let key = std::fs::read_to_string(&k)
                .map_err(|e| format!("transcribe_api_key_file {k}: {e}"))?;
            voice.api_key = Some(key.trim().to_string()).filter(|k| !k.is_empty());
        }
        let transcribe = file.flag("transcribe");
        if transcribe && voice.api_url.is_none() && voice.whisper_model.as_os_str().is_empty() {
            return Err(
                "transcribe is on but there is no speech service (transcribe_api_url) and no speech model (transcribe_model)"
                    .into(),
            );
        }
        let spaces: Vec<String> = file
            .text("transcribe_spaces")
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let mut watch = vec![];
        for line in file.list("watch") {
            let (dir, space) = arrow(&line);
            if dir.is_empty() {
                return Err(format!("watch: {line:?} names no folder (FOLDER -> SPACE)"));
            }
            watch.push(Watched {
                dir: home_path(dir),
                space: space.map(String::from),
            });
        }

        let exclude = file.list("absorb_exclude");
        let protect = file.list("absorb_protect");
        let roles = match file.text("absorb_roles") {
            Some(f) => {
                let path = home_path(f);
                super::absorb::read_roles(
                    &std::fs::read_to_string(&path)
                        .map_err(|e| format!("absorb_roles {}: {e}", path.display()))?,
                )
            }
            None => vec![],
        };
        let mut absorb = vec![];
        for line in file.list("absorb") {
            let (dir, space) = arrow(&line);
            let Some(space) = space else {
                return Err(format!("absorb: {line:?} names no space (FOLDER -> SPACE)"));
            };
            absorb.push(Source {
                dir: home_path(dir),
                space: space.to_string(),
                exclude: exclude.clone(),
                protect: protect.clone(),
                roles: roles.clone(),
            });
        }
        let mut deliver = vec![];
        for line in file.list("deliver") {
            // `SPACE TAG -> FOLDER`
            let (left, dir) = arrow(&line);
            let mut words = left.split_whitespace();
            let (Some(space), Some(tag), Some(dir)) = (words.next(), words.next(), dir) else {
                return Err(format!("deliver: {line:?} is not `SPACE TAG -> FOLDER`"));
            };
            deliver.push(Delivery {
                space: space.to_string(),
                tag: tag.to_string(),
                dir: home_path(dir),
            });
        }
        // The folder proposals are put down in is never absorbed back.
        for src in &mut absorb {
            for d in &deliver {
                if let Ok(rel) = d.dir.strip_prefix(&src.dir) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if !rel.is_empty() {
                        src.exclude.push(rel);
                    }
                }
            }
        }
        Ok(Settings {
            person,
            transcribe,
            transcribe_spaces: spaces,
            voice,
            voice_every: file
                .int("transcribe_every")
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(VOICE_EVERY),
            watch,
            watch_every: file
                .int("watch_every")
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(WATCH_EVERY),
            absorb,
            deliver,
            absorb_every: file
                .int("absorb_every")
                .and_then(|n| u64::try_from(n).ok())
                .map_or(15 * 60, |m| m * 60),
            state_dir: state_dir.to_path_buf(),
            utc_offset_minutes: local_offset_minutes(),
        })
    }

    /// Whether the node has anything to run in the background.
    pub fn is_idle(&self) -> bool {
        !self.transcribe
            && self.watch.is_empty()
            && self.absorb.is_empty()
            && self.deliver.is_empty()
    }
}
