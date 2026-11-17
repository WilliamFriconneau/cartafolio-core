//! Absorbing a folder of notes: the bridge reads the Markdown files of a folder the person keeps up to date (synced to
//! this machine) and sends **what is new in them** to the inbox of a space, tagged `inbox/absorb`. The server's janitor
//! then draws short, dated facts out of each piece, in the note's own words: what is memorised is the information, not
//! a copy of the files.
//!
//! - Each note is cut into passages at its headings (a long section into parts). A passage already sent (the same
//!   text) is never sent again; an edited one is sent as it now reads.
//! - Only files that changed since the last pass are read again (size and time, kept in a small state file). The first
//!   pass only takes stock, unless asked for everything: a folder that was imported before is not sent twice.
//! - Hidden files and folders are never read; folders named in `exclude` are skipped; a file whose name holds a word of
//!   `protect` is never opened.
//!
//! The folder itself is the queue: a memory that cannot be reached is tried again at the next pass.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use memcp_client::{Client, Error as ClientError};
use serde_json::{Value, json};

use crate::Error;

/// The longest text of one passage (the memory takes 4000 characters).
const PASSAGE_MAX: usize = 3500;

/// One folder and where it goes.
#[derive(Clone, Debug)]
pub struct Source {
    pub dir: PathBuf,
    pub space: String,
    /// Folders (relative to `dir`, any case) never read.
    pub exclude: Vec<String>,
    /// Words that keep a file closed when its name holds one (any case).
    pub protect: Vec<String>,
    /// People's names and the post each holds: a name is never sent, its post is (`PDG`, `directrice des opérations`).
    pub roles: Vec<(String, String)>,
}

/// The names → posts of a Markdown table (`| Name | Post | ... |`, a header and a separator line are skipped) or of
/// `Name — Post` / `Name: Post` lines.
pub fn read_roles(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in text.lines() {
        let l = line.trim();
        let (name, post) = if l.starts_with('|') {
            let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
            match cells.as_slice() {
                [n, p, ..]
                    if !n.is_empty()
                        && !p.is_empty()
                        && !n.chars().all(|c| c == '-' || c == ':') =>
                {
                    (*n, *p)
                }
                _ => continue,
            }
        } else if let Some((n, p)) = l.split_once(" — ").or_else(|| l.split_once(": ")) {
            (n.trim_start_matches(['-', '*', ' ']).trim(), p.trim())
        } else {
            continue;
        };
        let lower = fold(name);
        if matches!(lower.as_str(), "nom" | "name" | "personne" | "qui") || name.chars().count() < 3
        {
            continue;
        }
        out.push((name.to_string(), post.to_string()));
    }
    // Longest names first: « Marie Dupont » before « Marie ».
    out.sort_by_key(|r| std::cmp::Reverse(r.0.chars().count()));
    out
}

/// `text` with each known name, as a whole word and whatever its case, replaced by the post.
pub fn without_names(text: &str, roles: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (name, post) in roles {
        out = replace_word(&out, name, post);
    }
    out
}

fn replace_word(text: &str, from: &str, to: &str) -> String {
    let (low, from_low) = (text.to_lowercase(), from.to_lowercase());
    // Only where lowercasing keeps every letter's length (offsets stay true).
    if from_low.is_empty()
        || text
            .chars()
            .any(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() != c.len_utf8())
    {
        return text.to_string();
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::new();
    let mut at = 0;
    while let Some(i) = low[at..].find(&from_low) {
        let (start, end) = (at + i, at + i + from_low.len());
        out.push_str(&text[at..start]);
        if word(text[..start].chars().next_back()) || word(text[end..].chars().next()) {
            out.push_str(&text[start..end]);
        } else {
            out.push_str(to);
        }
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// How a pass went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Absorbed {
    pub files_read: u32,
    /// Pieces sent to the inbox.
    pub sent: u32,
    /// Files only taken stock of (first pass).
    pub noted: u32,
    pub failed: u32,
    pub problems: Vec<String>,
    /// The memory could not be reached: what is left goes at the next pass.
    pub unreachable: bool,
    pub key_refused: bool,
}

fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// The Markdown files of `src`, as paths relative to it, skipping hidden, excluded and protected ones.
fn files(src: &Source) -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    let mut stack = vec![src.dir.clone()];
    let excluded: Vec<String> = src
        .exclude
        .iter()
        .map(|e| fold(e.trim_matches('/')))
        .collect();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            let rel = path
                .strip_prefix(&src.dir)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if ft.is_dir() {
                let r = fold(&rel);
                if !excluded
                    .iter()
                    .any(|x| r == *x || r.starts_with(&format!("{x}/")))
                {
                    stack.push(path);
                }
            } else if ft.is_file() && fold(&name).ends_with(".md") {
                let n = fold(&name);
                if src
                    .protect
                    .iter()
                    .any(|w| !w.is_empty() && n.contains(&fold(w)))
                {
                    continue;
                }
                out.push((rel, path));
            }
        }
    }
    out.sort();
    out
}

/// A note cut into passages: `(id, text)`, ids stable while the headings stay.
pub fn passages(rel: &str, text: &str) -> Vec<(String, String)> {
    let mut body = text;
    // A front matter block is metadata, not the note.
    if let Some(rest) = body.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        body = rest[end + 4..].trim_start_matches(['\r', '\n']);
    }
    let title = Path::new(rel)
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let mut sections: Vec<(String, String)> = vec![(String::new(), String::new())];
    let mut in_code = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        let heading = (!in_code)
            .then(|| line.strip_prefix('#'))
            .flatten()
            .map(|h| h.trim_start_matches('#'))
            .filter(|h| h.starts_with(' '));
        match heading {
            Some(h) => sections.push((h.trim().to_string(), String::new())),
            None => {
                if let Some(last) = sections.last_mut() {
                    last.1.push_str(line);
                    last.1.push('\n');
                }
            }
        }
    }
    let mut out = vec![];
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for (heading, content) in sections {
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        let head = if heading.is_empty() {
            title.clone()
        } else {
            format!("{title} — {heading}")
        };
        let n = seen.entry(heading.clone()).or_insert(0);
        *n += 1;
        let base = format!("{rel}#{heading}/{n}");
        let room = PASSAGE_MAX
            .saturating_sub(head.chars().count() + 3)
            .max(500);
        for (i, part) in cut(content, room).into_iter().enumerate() {
            let id = if i == 0 {
                base.clone()
            } else {
                format!("{base}.{}", i + 1)
            };
            out.push((id, format!("{head} : {part}")));
        }
    }
    out
}

/// `text` in pieces of at most `size` characters, at a line, else a word, else anywhere.
fn cut(text: &str, size: usize) -> Vec<String> {
    let mut parts = vec![];
    let mut cur = String::new();
    for piece in text.split_inclusive(['\n', ' ']) {
        if cur.chars().count() + piece.chars().count() > size && !cur.trim().is_empty() {
            parts.push(cur.trim().to_string());
            cur.clear();
        }
        cur.push_str(piece);
        while cur.chars().count() > size {
            let head: String = cur.chars().take(size).collect();
            cur = cur.chars().skip(size).collect();
            parts.push(head.trim().to_string());
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// A short fingerprint of a passage's text (FNV-1a), to know it was sent.
fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// What the last passes saw: `{files: {rel: {size, mtime, sent: [hashes]}}}`.
struct State {
    file: PathBuf,
    data: Value,
    fresh: bool,
}

impl State {
    fn open(file: PathBuf) -> State {
        let read = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["files"].is_object());
        let fresh = read.is_none();
        State {
            file,
            data: read.unwrap_or_else(|| json!({ "files": {} })),
            fresh,
        }
    }

    fn save(&self) -> Result<(), Error> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
        }
        let tmp = self.file.with_extension("tmp");
        std::fs::write(&tmp, self.data.to_string()).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.file).map_err(|e| Error::Io(e.to_string()))
    }
}

/// One pass over `src`. `everything`: on a first pass, send what the folder holds instead of only taking stock.
pub fn absorb(
    client: &mut Client,
    src: &Source,
    state_dir: &Path,
    everything: bool,
) -> Result<Absorbed, Error> {
    let mut report = Absorbed::default();
    if !src.dir.is_dir() {
        return Err(Error::Invalid("the folder to absorb is not there".into()));
    }
    let name = hash(&format!("{}\0{}", src.dir.display(), src.space));
    let mut state = State::open(state_dir.join(format!("absorb-{name}.json")));
    let stock_only = state.fresh && !everything;
    for (rel, path) in files(src) {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let seen = &state.data["files"][&rel];
        if seen["size"].as_u64() == Some(meta.len()) && seen["mtime"].as_u64() == Some(mtime) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            report.problems.push(format!("{rel}: not readable as text"));
            continue;
        };
        let cut = passages(&rel, &text);
        let mut sent: Vec<String> = seen["sent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
        if stock_only {
            sent = cut.iter().map(|(_, t)| hash(t)).collect();
            report.noted += 1;
        } else {
            report.files_read += 1;
            let (y, m, d, _, _) = crate::time::civil(i64::try_from(mtime).unwrap_or(0), 0);
            for (_, t) in &cut {
                let h = hash(t);
                if sent.contains(&h) {
                    continue;
                }
                let note = without_names(
                    &format!("D'après la note {rel}, modifiée le {y:04}-{m:02}-{d:02} : {t}"),
                    &src.roles,
                );
                match client.call(
                    "remember",
                    json!({ "space": src.space, "kind": "context", "tags": ["inbox/absorb"], "text": note }),
                ) {
                    Ok(_) => {
                        sent.push(h);
                        report.sent += 1;
                    }
                    Err(ClientError::Tool(m)) => {
                        report.failed += 1;
                        report.problems.push(format!("{rel}: {m}"));
                    }
                    Err(e) => {
                        note_error(&e, &mut report);
                        // What was sent of this note is kept; it is read again next pass.
                        state.data["files"][&rel] = json!({ "sent": sent });
                        state.save()?;
                        return Ok(report);
                    }
                }
            }
        }
        // Only what the note holds now is remembered as sent: an old text that comes back is sent again.
        let now: Vec<String> = cut
            .iter()
            .map(|(_, t)| hash(t))
            .filter(|h| sent.contains(h))
            .collect();
        state.data["files"][&rel] = json!({ "size": meta.len(), "mtime": mtime, "sent": now });
    }
    state.save()?;
    Ok(report)
}

/// Where proposals of a space are put down as files: the facts under `tag` (`proposals/pro`) of `space`, one file each, in
/// `dir` (a folder an assistant that only reads files sees: the work assistant, say).
#[derive(Clone, Debug)]
pub struct Delivery {
    pub space: String,
    pub tag: String,
    pub dir: PathBuf,
}

/// Puts down the proposals not put down yet. How many were written.
pub fn deliver(
    client: &mut Client,
    d: &Delivery,
    state_dir: &Path,
    report: &mut Absorbed,
) -> Result<u32, Error> {
    let name = hash(&format!(
        "deliver\0{}\0{}\0{}",
        d.space,
        d.tag,
        d.dir.display()
    ));
    let state_file = state_dir.join(format!("deliver-{name}.json"));
    let mut done: Vec<String> = std::fs::read_to_string(&state_file)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default();
    let listed = match client.call(
        "recall",
        json!({ "space": d.space, "query": "", "tags": [d.tag], "limit": 50 }),
    ) {
        Ok(l) => l,
        Err(e) => {
            note_error(&e, report);
            return Ok(0);
        }
    };
    std::fs::create_dir_all(&d.dir).map_err(|e| Error::Io(e.to_string()))?;
    let mut n = 0;
    for f in listed["results"].as_array().into_iter().flatten() {
        let (Some(id), Some(text)) = (f["id"].as_str(), f["text"].as_str()) else {
            continue;
        };
        if done.iter().any(|x| x == id) || f["status"] != "active" {
            continue;
        }
        let created = f["created_at"].as_i64().unwrap_or(0);
        let (y, m, day, _, _) = crate::time::civil(created, 0);
        let body = text.trim();
        let words: String = body
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| {
                !matches!(
                    c,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '[' | ']'
                )
            })
            .take(60)
            .collect();
        let file = d
            .dir
            .join(format!("{y:04}-{m:02}-{day:02} {}.md", words.trim()));
        // Never over a file already there (the person may have edited it): the next free name.
        let mut path = file.clone();
        let mut k = 2;
        while path.exists() {
            path = file.with_file_name(format!("{y:04}-{m:02}-{day:02} {} ({k}).md", words.trim()));
            k += 1;
        }
        let content = format!(
            "---\ndate: {y:04}-{m:02}-{day:02}\nsource: mémoire\nstatut: nouvelle\nid: {id}\n---\n\n{body}\n"
        );
        std::fs::write(&path, content).map_err(|e| Error::Io(e.to_string()))?;
        done.push(id.to_string());
        n += 1;
    }
    let tmp = state_file.with_extension("tmp");
    std::fs::write(&tmp, json!(done).to_string()).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::rename(&tmp, &state_file).map_err(|e| Error::Io(e.to_string()))?;
    Ok(n)
}

/// Proposes the app guide that tells the assistants of `d.space` what a proposal for this folder is (once per text: the
/// person approves it on the memory's page, then every assistant of the space is told). Only for `proposals/<name>`.
pub fn propose_guide(client: &mut Client, d: &Delivery, state_dir: &Path, report: &mut Absorbed) {
    if !d.tag.starts_with("proposals/") {
        return;
    }
    let folder = d.dir.file_name().map_or_else(
        || "the folder".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let text = format!(
        "What is written in this space with the tag `{tag}` is put down as a file in the person's work folder, under « {folder} », once, for the assistants that read that folder. \
Write there an idea or a note meant for that folder, when the person asks for it (« idée pour le pro », « note-le pour le pro »): one fact each, dated, in the person's words, no person's name (the post instead). \
It is not filed elsewhere and not changed afterwards; to take one back, retire it.",
        tag = d.tag
    );
    let mark = state_dir.join(format!(
        "guide-{}.txt",
        hash(&format!("{}\0{text}", d.space))
    ));
    if mark.exists() {
        return;
    }
    match client.call("guide_propose", json!({ "space": d.space, "text": text })) {
        Ok(_) => {
            let _ = std::fs::create_dir_all(state_dir);
            let _ = std::fs::write(&mark, b"proposed");
        }
        Err(e) => note_error(&e, report),
    }
}

fn note_error(e: &ClientError, report: &mut Absorbed) {
    match e {
        ClientError::Refused => report.key_refused = true,
        ClientError::Unreachable(_) | ClientError::RateLimited(_) => report.unreachable = true,
        other => report.problems.push(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::passages;

    #[test]
    fn a_note_is_cut_at_its_headings_with_stable_ids() {
        let note = "---\ntags: x\n---\nIntro line.\n\n# Décisions\nOn garde A.\n```\n# not a heading\n```\n## Décisions\nEt B.\n# Vide\n";
        let p = passages("Projets/Alpha.md", note);
        let ids: Vec<&str> = p.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(
            ids,
            [
                "Projets/Alpha.md#/1",
                "Projets/Alpha.md#Décisions/1",
                "Projets/Alpha.md#Décisions/2"
            ]
        );
        assert_eq!(p[0].1, "Alpha : Intro line.");
        assert!(
            p[1].1.starts_with("Alpha — Décisions : On garde A.")
                && p[1].1.contains("# not a heading")
        );
        let roles = super::read_roles(
            "| Nom | Poste |\n|---|---|\n| Marie Dupont | directrice des opérations |\n| Marie | assistante |\nPaul Martin — PDG\n",
        );
        assert_eq!(roles[0].0, "Marie Dupont");
        assert_eq!(
            super::without_names(
                "Réunion avec marie dupont et Paul Martin, puis Marie. Mariette reste.",
                &roles
            ),
            "Réunion avec directrice des opérations et PDG, puis assistante. Mariette reste."
        );
        let long = format!("# Long\n{}", "mot ".repeat(2000));
        let parts = passages("L.md", &long);
        assert!(parts.len() >= 3 && parts.iter().all(|(_, t)| t.chars().count() <= 3500));
        assert_eq!(parts[1].0, "L.md#Long/1.2");
    }
}
