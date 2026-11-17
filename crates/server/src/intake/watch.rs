//! A folder watched: what is added to it becomes a note or a file of a space.
//!
//! The folder is a drop box. A Markdown or text file becomes a note (cut at 3500 characters), an audio file becomes a
//! recording the node reads (see `voice`), any other file is kept as it is. All of it lands in the **inbox** of the
//! chosen space (the person's « À trier » when none is named), where the janitor files it like any other note.
//!
//! - The file stays where it is: nothing in a watched folder is moved, changed or deleted.
//! - A file is taken once its last change is a few seconds old (a copy in progress is waited for); a name that says it
//!   is not finished (`.part`, `.crdownload`, `.tmp`) is never taken, nor a hidden file or folder.
//! - What was taken is remembered (size and time of the file), so a restart never sends a file twice. A file edited
//!   later is taken again as it now reads: the memory ignores a text or a name it already holds.
//! - The first pass takes what is already in the folder: it is a drop box, not a library. For a folder that holds notes
//!   to keep up to date, see `absorb`.
//! - A file the memory refuses is tried three times, then left alone, said once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use super::absorb::fingerprint;
use super::{Error, Memory};

/// A file younger than this (seconds since its last change) is still being written.
pub const SETTLE_SECS: i64 = 3;
/// The most a note of this kind holds (the memory takes 4000 characters).
const NOTE_MAX: usize = 3500;
/// The biggest file taken (read whole into memory).
const FILE_MAX: u64 = 100 * 1024 * 1024;
/// Tries before a file the memory refuses is left alone.
const MAX_FAILS: u64 = 3;
/// How deep the folder is read.
const DEPTH: usize = 6;

const AUDIO: [&str; 9] = [
    "m4a", "mp3", "wav", "ogg", "opus", "flac", "caf", "amr", "aac",
];
const TEXT: [&str; 3] = ["md", "markdown", "txt"];
const UNFINISHED: [&str; 6] = ["part", "crdownload", "tmp", "download", "partial", "swp"];

/// How a scan went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Notes kept (one per piece of a long file).
    pub notes: u32,
    /// Files and recordings kept.
    pub files: u32,
    /// Still being written: next time.
    pub waiting: u32,
    pub failed: u32,
    pub problems: Vec<String>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// The files of `dir` as paths relative to it: no hidden file or folder, no link, no unfinished download.
fn files_of(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, base: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.ends_with('~') {
                continue;
            }
            // `file_type` does not follow a link: a link leaves the folder, it is not read.
            let Ok(kind) = e.file_type() else { continue };
            let path = e.path();
            if kind.is_dir() {
                if depth < DEPTH {
                    walk(&path, base, depth + 1, out);
                }
            } else if kind.is_file() {
                let ext = extension(&path);
                if UNFINISHED.contains(&ext.as_str()) {
                    continue;
                }
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push((rel.to_string_lossy().replace('\\', "/"), path.clone()));
                }
            }
        }
    }
    let mut out = vec![];
    walk(dir, dir, 0, &mut out);
    out
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// `text` in pieces of at most `size` characters, at a line, else at a word, else anywhere.
fn pieces(text: &str, size: usize) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    for line in text.split_inclusive('\n') {
        if cur.chars().count() + line.chars().count() > size && !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
            cur.clear();
        }
        cur.push_str(line);
        while cur.chars().count() > size {
            let cut = cur
                .char_indices()
                .take(size)
                .filter(|(i, c)| *i > 0 && c.is_whitespace())
                .last()
                .map_or_else(
                    || cur.char_indices().nth(size).map_or(cur.len(), |(i, _)| i),
                    |(i, _)| i,
                );
            let (head, tail) = cur.split_at(cut);
            out.push(head.trim().to_string());
            cur = tail.to_string();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.retain(|p| !p.is_empty());
    out
}

/// A name the memory takes for a recording and reads the date from: `2026-11-02-13h34-title.ext`.
fn recording_name(rel: &str, mtime: i64, offset: i32) -> String {
    let (y, mo, d, h, mi) = super::time::civil(mtime, offset);
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "m4a"));
    let stem: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!(
        "{y:04}-{mo:02}-{d:02}-{h:02}h{mi:02}-{stem}.{}",
        ext.to_lowercase()
    )
}

/// One scan of `dir`: what is new in it goes to the inbox of `space`.
pub fn scan(
    memory: &mut Memory,
    dir: &Path,
    space: &str,
    state_dir: &Path,
    offset_minutes: i32,
) -> Result<Report, Error> {
    let mut report = Report::default();
    if !dir.is_dir() {
        return Err(Error::Invalid(format!(
            "the watched folder {} is not there",
            dir.display()
        )));
    }
    let state_file = state_dir.join(format!(
        "watch-{}.json",
        fingerprint(&format!("{}\0{space}", dir.display()))
    ));
    let mut seen: BTreeMap<String, Value> = std::fs::read_to_string(&state_file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["files"].as_object().cloned())
        .map(|o| o.into_iter().collect())
        .unwrap_or_default();
    let mut changed = false;
    for (rel, path) in files_of(dir) {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let known = seen.get(&rel);
        let same = known.is_some_and(|e| {
            e["size"].as_u64() == Some(meta.len()) && e["mtime"].as_i64() == Some(mtime)
        });
        if same
            && known
                .is_some_and(|e| e["done"] == true || e["fails"].as_u64().unwrap_or(0) >= MAX_FAILS)
        {
            continue;
        }
        if now() - mtime < SETTLE_SECS {
            report.waiting += 1;
            continue;
        }
        if meta.len() > FILE_MAX {
            if !same {
                report.problems.push(format!(
                    "{rel}: over {} MB, too big to take",
                    FILE_MAX / 1024 / 1024
                ));
                seen.insert(
                    rel,
                    json!({ "size": meta.len(), "mtime": mtime, "done": false, "fails": MAX_FAILS }),
                );
                changed = true;
            }
            continue;
        }
        let fails = if same {
            known.and_then(|e| e["fails"].as_u64()).unwrap_or(0)
        } else {
            0
        };
        match take(
            memory,
            &rel,
            &path,
            space,
            mtime,
            offset_minutes,
            &mut report,
        ) {
            Ok(()) => {
                seen.insert(
                    rel,
                    json!({ "size": meta.len(), "mtime": mtime, "done": true, "fails": 0 }),
                );
            }
            Err(why) => {
                report.failed += 1;
                let n = fails + 1;
                let last = if n >= MAX_FAILS {
                    " (left alone from now on)"
                } else {
                    ""
                };
                report.problems.push(format!("{rel}: {why}{last}"));
                seen.insert(
                    rel,
                    json!({ "size": meta.len(), "mtime": mtime, "done": false, "fails": n }),
                );
            }
        }
        changed = true;
    }
    if changed {
        std::fs::create_dir_all(state_dir).map_err(|e| Error::Io(e.to_string()))?;
        let tmp = state_file.with_extension("tmp");
        std::fs::write(&tmp, json!({ "files": seen }).to_string())
            .map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &state_file).map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(report)
}

/// Puts one file into the inbox of `space` the way a watched folder would (`memcp intake send`): a note, a recording or a
/// file by its kind. How many notes and files it made.
pub fn put_file(
    memory: &mut Memory,
    path: &Path,
    space: &str,
    offset_minutes: i32,
) -> Result<Report, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if meta.len() > FILE_MAX {
        return Err(format!("{}: too big to take", path.display()));
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or_else(now, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let rel = path
        .file_name()
        .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().into_owned());
    let mut report = Report::default();
    take(
        memory,
        &rel,
        path,
        space,
        mtime,
        offset_minutes,
        &mut report,
    )?;
    Ok(report)
}

/// Keeps one file in the memory, as a note, a recording or a file.
fn take(
    memory: &mut Memory,
    rel: &str,
    path: &Path,
    space: &str,
    mtime: i64,
    offset: i32,
    report: &mut Report,
) -> Result<(), String> {
    let ext = extension(path);
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (y, mo, d, _, _) = super::time::civil(mtime, offset);
    if TEXT.contains(&ext.as_str()) {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if text.trim().is_empty() {
            return Ok(());
        }
        let head = format!("Fichier {rel} déposé le {y:04}-{mo:02}-{d:02}");
        let parts = pieces(&text, NOTE_MAX);
        for (i, part) in parts.iter().enumerate() {
            let of = if parts.len() > 1 {
                format!(" (partie {}/{})", i + 1, parts.len())
            } else {
                String::new()
            };
            memory
                .call(
                    "remember",
                    json!({ "space": space, "kind": "context", "tags": ["inbox"], "text": format!("{head}{of} : {part}") }),
                )
                .map_err(|e| e.to_string())?;
            report.notes += 1;
        }
        return Ok(());
    }
    if AUDIO.contains(&ext.as_str()) {
        let name = recording_name(rel, mtime, offset);
        memory
            .file_put(
                space,
                &name,
                &format!("Enregistrement déposé dans un dossier surveillé ({rel})"),
                &["inbox/audio"],
                &bytes,
            )
            .map_err(|e| e.to_string())?;
    } else {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        memory
            .file_put(
                space,
                name,
                &format!("Fichier déposé dans un dossier surveillé ({rel})"),
                &["inbox"],
                &bytes,
            )
            .map_err(|e| e.to_string())?;
    }
    report.files += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_text_is_cut_at_lines_and_nothing_is_lost() {
        let text = (0..400)
            .map(|i| format!("ligne numéro {i}\n"))
            .collect::<String>();
        let parts = pieces(&text, 500);
        assert!(parts.len() > 5 && parts.iter().all(|p| p.chars().count() <= 500));
        let squeeze = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(squeeze(&parts.concat()), squeeze(&text));
        let one_line = "mot ".repeat(400);
        assert!(
            pieces(&one_line, 100)
                .iter()
                .all(|p| p.chars().count() <= 100)
        );
    }

    #[test]
    fn a_recording_is_named_with_the_date_the_node_reads() {
        // 2026-11-02 13:34 UTC, in Paris winter time.
        let at = super::super::time::unix(2026, 11, 2, 12, 34);
        assert_eq!(
            recording_name("dictées/Réunion 1.M4A", at, 60),
            "2026-11-02-13h34-Réunion-1.m4a"
        );
    }
}
