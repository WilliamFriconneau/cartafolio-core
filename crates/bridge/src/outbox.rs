//! The outbox: a folder of its own that holds only what waits to be sent, and the last few that were.
//! No database, no copy of the memory. A capture is written whole (a temporary file, then a rename)
//! before anything says it is kept, so a crash or a closed lid loses nothing.
//!
//! `queue/<id>.json` is an item waiting (or refused for good), `queue/<id>.bin` its bytes if it has some;
//! `sent/<id>.json` is the short record of one that went.

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::private::{PrivateDir as _, PrivateFile as _};
use serde_json::{Value, json};

use crate::{Error, Item, Kind, State};

/// How many records of what went are kept to show.
const SENT_KEPT: usize = 50;
/// How many recordings that went are kept here to be listened to again (some 11 MB for thirty minutes each).
const AUDIO_KEPT: usize = 20;

/// One item, as stored.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub created_at: i64,
    pub bytes: u64,
    pub attempts: u32,
    pub state: State,
    pub detail: String,
    /// When the memory took it (Unix time), 0 while it waits.
    pub sent_at: i64,
    /// What to send: the texts, or the name, note and tags of a file, or the tool and its arguments.
    pub payload: Value,
}

impl Entry {
    pub fn item(&self) -> Item {
        Item {
            id: self.id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            created_at: self.created_at,
            bytes: self.bytes,
            state: self.state,
            detail: self.detail.clone(),
            sent_at: self.sent_at,
            heard: self.payload["heard"].as_str().unwrap_or("").to_string(),
            audio: String::new(),
            summary: self.payload["summary"].as_str().unwrap_or("").to_string(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id, "kind": self.kind.word(), "title": self.title,
            "created_at": self.created_at, "bytes": self.bytes, "attempts": self.attempts,
            "state": self.state.word(), "detail": self.detail, "sent_at": self.sent_at,
            "payload": self.payload,
        })
    }

    fn from_json(v: &Value) -> Option<Entry> {
        Some(Entry {
            id: v["id"].as_str()?.to_string(),
            kind: Kind::from_word(v["kind"].as_str()?)?,
            title: v["title"].as_str()?.to_string(),
            created_at: v["created_at"].as_i64()?,
            bytes: v["bytes"].as_u64()?,
            attempts: u32::try_from(v["attempts"].as_u64().unwrap_or(0)).unwrap_or(0),
            state: State::from_word(v["state"].as_str()?)?,
            detail: v["detail"].as_str().unwrap_or("").to_string(),
            sent_at: v["sent_at"].as_i64().unwrap_or(0),
            payload: v["payload"].clone(),
        })
    }
}

pub struct Outbox {
    root: PathBuf,
}

fn io(e: &std::io::Error) -> Error {
    Error::Io(e.to_string())
}

impl Outbox {
    /// Opens the folder, making it (private to the user) if it is not there.
    pub fn open(root: &Path) -> Result<Outbox, Error> {
        for sub in ["queue", "sent"] {
            fs::DirBuilder::new()
                .recursive(true)
                .private_dir()
                .create(root.join(sub))
                .map_err(|e| io(&e))?;
        }
        let o = Outbox {
            root: root.to_path_buf(),
        };
        o.clean();
        Ok(o)
    }

    /// A sortable, unique name: the time first, so the oldest goes first.
    pub fn new_id(now: i64) -> String {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed) % 10_000;
        let mut h = RandomState::new().build_hasher();
        h.write_i64(now);
        h.write_u32(n);
        format!("{now:011}-{n:04}-{:08x}", h.finish() & 0xffff_ffff)
    }

    /// The lock of this outbox, held while items are sent, refused or forgotten: two processes on one outbox (the app
    /// and the command line, two gateways) never work on the same item at once. `wait`: block until it is free.
    pub fn lock(&self, wait: bool) -> Result<Option<fs::File>, Error> {
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .private_file()
            .open(self.root.join(".lock"))
            .map_err(|e| io(&e))?;
        if wait {
            f.lock().map_err(|e| io(&e))?;
            return Ok(Some(f));
        }
        match f.try_lock() {
            Ok(()) => Ok(Some(f)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(e)) => Err(io(&e)),
        }
    }

    /// Whether `id` is one this outbox made (digits, letters a-f and dashes): never a path.
    pub fn valid_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 40
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
    }

    /// Writes `bytes` to `path` whole: a temporary file of its own in the same folder, then a rename, then the folder
    /// itself made durable (otherwise a power cut can lose a rename already reported as done).
    fn write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
        let mut h = RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        let tmp = path.with_file_name(format!(
            ".{name}.{}.{:08x}.tmp",
            std::process::id(),
            h.finish() & 0xffff_ffff
        ));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .private_file()
            .open(&tmp)
            .map_err(|e| io(&e))?;
        f.write_all(bytes).map_err(|e| io(&e))?;
        f.sync_all().map_err(|e| io(&e))?;
        fs::rename(&tmp, path).map_err(|e| io(&e))?;
        if let Some(dir) = path.parent() {
            let _ = fs::File::open(dir).and_then(|d| d.sync_all());
        }
        Ok(())
    }

    /// Puts an item in the queue. Its bytes go first: an item never exists without them.
    pub fn put(&self, entry: &Entry, blob: Option<&[u8]>) -> Result<(), Error> {
        let q = self.root.join("queue");
        if let Some(b) = blob {
            Self::write(&q.join(format!("{}.bin", entry.id)), b)?;
        }
        Self::write(
            &q.join(format!("{}.json", entry.id)),
            entry.to_json().to_string().as_bytes(),
        )
    }

    /// Rewrites an item still in the queue (an attempt counted, a refusal recorded). One that left meanwhile (sent,
    /// forgotten) is not brought back.
    pub fn update(&self, entry: &Entry) -> Result<(), Error> {
        let path = self.root.join("queue").join(format!("{}.json", entry.id));
        if !path.exists() {
            return Ok(());
        }
        Self::write(&path, entry.to_json().to_string().as_bytes())
    }

    /// Leftovers of a crash, more than an hour old: temporary files, and bytes whose item never got written.
    pub fn clean(&self) {
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for dir in ["queue", "sent"] {
            for e in fs::read_dir(self.root.join(dir))
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = e.path();
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t < hour_ago);
                let name = e.file_name().to_string_lossy().into_owned();
                let orphan = path.extension().is_some_and(|x| x == "bin")
                    && !path.with_extension("json").exists();
                if old && (name.ends_with(".tmp") || orphan) {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    fn read_dir(dir: &Path) -> Vec<Entry> {
        let mut out: Vec<Entry> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
            .filter_map(|v| Entry::from_json(&v))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// What waits or was refused, the oldest first.
    pub fn queue(&self) -> Vec<Entry> {
        Self::read_dir(&self.root.join("queue"))
    }

    /// What went, the newest first, at most [`SENT_KEPT`].
    pub fn sent(&self) -> Vec<Entry> {
        let mut v = Self::read_dir(&self.root.join("sent"));
        v.reverse();
        v.truncate(SENT_KEPT);
        v
    }

    pub fn blob(&self, id: &str) -> Option<Vec<u8>> {
        if !Self::valid_id(id) {
            return None;
        }
        fs::read(self.root.join("queue").join(format!("{id}.bin"))).ok()
    }

    /// An item that went: its record moves to `sent`, its bytes are dropped, the oldest records go.
    pub fn archive(&self, entry: &Entry) -> Result<(), Error> {
        let mut done = entry.clone();
        done.state = State::Sent;
        done.sent_at = crate::now();
        // Small on purpose: of what was sent, only the text heard on this machine is kept, to be read again.
        done.payload = json!({
            "heard": entry.payload["heard"].as_str().unwrap_or(""),
            "summary": entry.payload["summary"].as_str().unwrap_or(""),
        });
        let sent = self.root.join("sent");
        // A recording is kept to be listened to again, under its own extension (a player needs it).
        if entry.kind == Kind::Audio {
            let ext = Path::new(entry.payload["name"].as_str().unwrap_or(""))
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("m4a");
            let _ = fs::rename(
                self.root.join("queue").join(format!("{}.bin", entry.id)),
                sent.join(format!("{}.{ext}", entry.id)),
            );
        }
        Self::write(
            &sent.join(format!("{}.json", done.id)),
            done.to_json().to_string().as_bytes(),
        )?;
        for ext in ["json", "bin"] {
            let _ = fs::remove_file(self.root.join("queue").join(format!("{}.{ext}", entry.id)));
        }
        let all = Self::read_dir(&sent);
        if all.len() > SENT_KEPT {
            for old in &all[..all.len() - SENT_KEPT] {
                self.remove_sent(&old.id);
            }
        }
        let with_audio: Vec<String> = Self::read_dir(&sent)
            .into_iter()
            .filter(|e| self.audio(&e.id).is_some())
            .map(|e| e.id)
            .collect();
        if with_audio.len() > AUDIO_KEPT {
            for id in &with_audio[..with_audio.len() - AUDIO_KEPT] {
                if let Some(a) = self.audio(id) {
                    let _ = fs::remove_file(a);
                }
            }
        }
        Ok(())
    }

    /// The recording of an item that went, kept here to be listened to again.
    pub fn audio(&self, id: &str) -> Option<PathBuf> {
        if !Self::valid_id(id) {
            return None;
        }
        fs::read_dir(self.root.join("sent"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_stem().and_then(|s| s.to_str()) == Some(id)
                    && p.extension().is_some_and(|x| x != "json" && x != "tmp")
            })
    }

    fn remove_sent(&self, id: &str) {
        if let Some(a) = self.audio(id) {
            let _ = fs::remove_file(a);
        }
        let _ = fs::remove_file(self.root.join("sent").join(format!("{id}.json")));
    }

    /// Takes an item out of the queue, or a record out of what went.
    pub fn remove(&self, id: &str) {
        if !Self::valid_id(id) {
            return;
        }
        for ext in ["json", "bin"] {
            let _ = fs::remove_file(self.root.join("queue").join(format!("{id}.{ext}")));
        }
        self.remove_sent(id);
    }
}
