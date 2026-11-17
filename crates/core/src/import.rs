//! Import of a Markdown folder (an Obsidian vault, for instance): deterministic,
//! no model, incremental.
//!
//! Each note is cut into passages (`kind = 'passage'`) stored in the space its folder maps
//! to ([`ImportRules`]). Nothing is erased: a passage whose text changes is replaced, a
//! vanished passage is retired (tombstone), a moved note keeps its passages (the pointer
//! follows). Protected files are never read (only their name is looked at).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::facts::{self, Fact, NewFact, Source};
use crate::space::Space;
use crate::text;
use crate::util;

const MAX_BLOCK: usize = 1500;
const MIN_BLOCK: usize = 40;
const AUTHOR: &str = "import";
const DRY_RUN_VALID_SECS: i64 = 24 * 3600;

/// Folders never imported, whatever the rules say (compared folded).
const ALWAYS_EXCLUDED_DIRS: [&str; 3] = [".obsidian", ".trash", ".git"];

/// Date of a note dated by its name (`2026-10-30.md`, `2026-10-30 Meeting.md`), in Unix
/// seconds at UTC midnight: a journal or daily note holds from that day on.
pub fn date_from_name(rel: &str) -> Option<i64> {
    let name = rel.rsplit('/').next()?;
    let b = name.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| name.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1900..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days since 1970 (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400)
}

/// Whole words in a file name that make it protected (English and French).
const PROTECTED_WORDS: [&str; 8] = ["codes", "iban", "rib", "pin", "puk", "pass", "mdp", "2fa"];
/// Fragments in a file name that make it protected (English and French).
const PROTECTED_FRAGMENTS: [&str; 10] = [
    "secret",
    "password",
    "passwd",
    "credential",
    "token",
    "recovery",
    "private key",
    "mot de passe",
    "motdepasse",
    "jeton",
];
/// File names that are instructions for agents, not memory.
const INSTRUCTION_FILES: [&str; 4] = ["claude.md", "agents.md", "gemini.md", "skill.md"];

/// Where notes go and what is left out. Parsed from a small rules file:
///
/// ```text
/// # comment
/// map Health = health          # notes under "Health/" go to the space "health"
/// map Journal/Work = work      # a deeper prefix wins over a shorter one
/// default = personal           # every other note
/// exclude = Archives, Attachments
/// protect = bank card, door codes
/// instructions = House rules   # file-name fragments that are agent instructions
/// ```
///
/// Folder prefixes and fragments are compared folded (no accents, lowercase).
#[derive(Clone, Debug, PartialEq)]
pub struct ImportRules {
    /// (folded folder prefix, space), longest prefix first.
    pub routes: Vec<(String, Space)>,
    /// The space of every note no route matches.
    pub default_space: Space,
    /// Folders (folded names) never read.
    pub excluded_dirs: Vec<String>,
    /// Name fragments (folded) of notes never read at all (card numbers, door codes).
    pub protected: Vec<String>,
    /// Name fragments of notes that are instructions to an agent, not memory: counted, never imported.
    pub instructions: Vec<String>,
}

impl ImportRules {
    /// Reads the rules text (`map Folder = space`, `default = space`, `exclude`, `protect`, `instructions`; `#` starts a comment).
    ///
    /// # Errors
    /// [`Error::Invalid`] for an unknown key, a bad space name, or a missing `default`.
    pub fn parse(src: &str) -> Result<ImportRules> {
        let mut routes = vec![];
        let mut default_space = None;
        let mut excluded_dirs = vec![];
        let mut protected = vec![];
        let mut instructions = vec![];
        let list = |v: &str| -> Vec<String> {
            v.split(',')
                .map(|x| text::fold(x.trim()).trim_matches('/').to_string())
                .filter(|x| !x.is_empty())
                .collect()
        };
        for (n, raw) in src.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = || Error::Invalid(format!("import rules, line {}: {raw:?}", n + 1));
            let (key, value) = line.split_once('=').ok_or_else(bad)?;
            let (key, value) = (key.trim(), value.trim());
            if let Some(prefix) = key.strip_prefix("map ") {
                let prefix = text::fold(prefix.trim()).trim_matches('/').to_string();
                if prefix.is_empty() {
                    return Err(bad());
                }
                routes.push((prefix, Space::new(value)?));
            } else {
                match key {
                    "default" => default_space = Some(Space::new(value)?),
                    "exclude" => excluded_dirs.extend(list(value)),
                    "protect" => protected.extend(list(value)),
                    "instructions" => instructions.extend(list(value)),
                    _ => return Err(bad()),
                }
            }
        }
        routes.sort_by_key(|r: &(String, Space)| std::cmp::Reverse(r.0.len()));
        Ok(ImportRules {
            routes,
            default_space: default_space.ok_or_else(|| {
                Error::Invalid("import rules: `default = <space>` is required".into())
            })?,
            excluded_dirs,
            protected,
            instructions,
        })
    }

    /// Every space the rules may write to.
    pub fn spaces(&self) -> Vec<Space> {
        let mut out: Vec<Space> = self.routes.iter().map(|(_, s)| s.clone()).collect();
        out.push(self.default_space.clone());
        out.sort();
        out.dedup();
        out
    }

    /// The space of a note, from its folder.
    pub fn space_for(&self, rel: &str) -> Space {
        let folded = text::fold(rel);
        self.routes
            .iter()
            .find(|(p, _)| folded.starts_with(&format!("{p}/")))
            .map_or_else(|| self.default_space.clone(), |(_, s)| s.clone())
    }
}

/// How to run an import.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// Only process this folder (relative path).
    pub only: Option<String>,
    /// Count and report only; nothing is written (and a later real run is then allowed).
    pub dry_run: bool,
    /// Allows a first real run without a recent `--dry-run`.
    pub yes: bool,
    /// Extra protection patterns (name fragments), on top of the rules.
    pub extra_protected: Vec<String>,
}

/// What an import did (or, in a dry run, would do), in notes and passages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Nothing was written.
    pub dry_run: bool,
    /// Notes read.
    pub read: usize,
    /// Notes whose passages did not change.
    pub unchanged: usize,
    /// Passages created.
    pub created: usize,
    /// Passages replaced by a new text (the old ones stay in history).
    pub replaced: usize,
    /// Passages retired because they left the note.
    pub retired: usize,
    /// Passages whose pointer moved with a renamed note.
    pub repointed: usize,
    /// Passages skipped because the same text is already active.
    pub duplicates: usize,
    /// Notes never read (protected).
    pub protected: usize,
    /// Notes in excluded folders.
    pub excluded: usize,
    /// Notes that are instructions to an agent.
    pub instructions: usize,
    /// Notes too big to read.
    pub too_big: usize,
    /// Notes that were imported before and are gone from the folder.
    pub missing_notes: usize,
    /// Passages left out because they look like a secret (token, password, key).
    pub secrets_masked: usize,
    /// Where (note#heading), never what: so the owner can go and look.
    pub masked_refs: Vec<String>,
    /// Passages created, per space.
    pub created_by_space: BTreeMap<String, usize>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{}",
            if self.dry_run {
                "Dry run (nothing was written):"
            } else {
                "Import done:"
            }
        )?;
        writeln!(f, "  notes read ............... {}", self.read)?;
        writeln!(f, "  unchanged ................ {}", self.unchanged)?;
        writeln!(f, "  passages created ......... {}", self.created)?;
        writeln!(f, "  passages replaced ........ {}", self.replaced)?;
        writeln!(f, "  passages retired ......... {}", self.retired)?;
        writeln!(f, "  pointers moved ........... {}", self.repointed)?;
        writeln!(f, "  duplicates skipped ....... {}", self.duplicates)?;
        writeln!(f, "  vanished notes ........... {}", self.missing_notes)?;
        writeln!(f, "  protected (never read) ... {}", self.protected)?;
        writeln!(f, "  passages left out (secret) {}", self.secrets_masked)?;
        writeln!(f, "  excluded ................. {}", self.excluded)?;
        writeln!(f, "  instructions (not memory)  {}", self.instructions)?;
        writeln!(f, "  too big .................. {}", self.too_big)?;
        for (s, n) in &self.created_by_space {
            writeln!(f, "  created in {s:<14} {n}")?;
        }
        if !self.masked_refs.is_empty() {
            writeln!(
                f,
                "\nNotes where a passage was left out as a secret (to review):"
            )?;
            for r in self.masked_refs.iter().take(40) {
                writeln!(f, "  {r}")?;
            }
        }
        Ok(())
    }
}

/// A passage ready to be stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Heading path of the passage inside the note.
    pub reference: String,
    /// The passage text.
    pub text: String,
    /// Where it sits in the note.
    pub range: String,
}

fn sha_hex(bytes: &[u8]) -> String {
    text::hex(&Sha256::digest(bytes))
}

fn is_protected(name: &str, extra: &[String]) -> bool {
    let n = text::fold(name);
    if PROTECTED_FRAGMENTS.iter().any(|f| n.contains(f))
        || extra.iter().any(|e| n.contains(e.as_str()))
    {
        return true;
    }
    n.split(|c: char| !c.is_alphanumeric())
        .any(|w| PROTECTED_WORDS.contains(&w))
}

fn is_instruction(rel: &str, extra: &[String]) -> bool {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let f = text::fold(file);
    INSTRUCTION_FILES.contains(&f.as_str()) || extra.iter().any(|e| f.contains(e.as_str()))
}

/// Readable text of a line: wiki links, images and comments removed.
fn clean_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // ![[embed]] and ![alt](url): removed.
        if chars[i] == '!' && i + 1 < chars.len() {
            if chars[i + 1] == '[' && i + 2 < chars.len() && chars[i + 2] == '[' {
                if let Some(end) = find(&chars, i + 3, "]]") {
                    i = end + 2;
                    continue;
                }
            } else if chars[i + 1] == '['
                && let Some(end) = find(&chars, i + 2, "](")
                && let Some(close) = find(&chars, end + 2, ")")
            {
                i = close + 1;
                continue;
            }
        }
        // [[target|text]] or [[target#heading]]: keep the text.
        if chars[i] == '['
            && i + 1 < chars.len()
            && chars[i + 1] == '['
            && let Some(end) = find(&chars, i + 2, "]]")
        {
            let inner: String = chars[i + 2..end].iter().collect();
            let shown = inner.rsplit('|').next().unwrap_or(&inner);
            out.push_str(shown.split('#').next().unwrap_or(shown));
            i = end + 2;
            continue;
        }
        // [text](url): keep the text.
        if chars[i] == '['
            && let Some(mid) = find(&chars, i + 1, "](")
            && let Some(close) = find(&chars, mid + 2, ")")
        {
            out.extend(&chars[i + 1..mid]);
            i = close + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    (from..chars.len().saturating_sub(n.len() - 1)).find(|&i| chars[i..i + n.len()] == n[..])
}

/// Removes `<!-- -->` and `%% %%` comments (over the whole text, multi-line included).
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    'outer: loop {
        for (open, close) in [("<!--", "-->"), ("%%", "%%")] {
            if let Some(a) = rest.find(open)
                && let Some(b) = rest[a + open.len()..].find(close)
            {
                out.push_str(&rest[..a]);
                rest = &rest[a + open.len() + b + close.len()..];
                continue 'outer;
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

/// Cuts a note into passages.
pub fn chunk(rel: &str, content: &str) -> Vec<Block> {
    let content = strip_comments(content);
    let all: Vec<&str> = content.lines().collect();
    // YAML frontmatter: removed from the text.
    let mut start = 0;
    if all.first().is_some_and(|l| l.trim() == "---")
        && let Some(end) = all.iter().skip(1).take(200).position(|l| l.trim() == "---")
    {
        start = end + 2;
    }
    let stem = rel
        .rsplit('/')
        .next()
        .unwrap_or(rel)
        .trim_end_matches(".md")
        .to_string();

    struct Section {
        title: String,
        first_line: usize,
        last_line: usize,
        lines: Vec<String>,
    }
    let mut sections: Vec<Section> = vec![Section {
        title: stem.clone(),
        first_line: start + 1,
        last_line: start + 1,
        lines: vec![],
    }];
    let mut fence = false;
    for (i, raw) in all.iter().enumerate().skip(start) {
        let line_no = i + 1;
        let t = raw.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        let level = raw.chars().take_while(|c| *c == '#').count();
        let is_heading = !fence
            && (1..=4).contains(&level)
            && raw.chars().nth(level).is_some_and(char::is_whitespace);
        if is_heading {
            let title = clean_line(raw[level..].trim()).trim().to_string();
            sections.push(Section {
                title: if title.is_empty() {
                    stem.clone()
                } else {
                    title
                },
                first_line: line_no,
                last_line: line_no,
                lines: vec![],
            });
        } else if let Some(s) = sections.last_mut() {
            s.lines.push(clean_line(raw));
            s.last_line = line_no;
        }
    }

    let mut seen_titles: HashMap<String, usize> = HashMap::new();
    let mut blocks: Vec<(String, String, usize, usize)> = vec![]; // (ref, text, first, last)
    for sec in sections {
        let body = sec.lines.join("\n");
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        let count = seen_titles.entry(sec.title.clone()).or_insert(0);
        *count += 1;
        let title_key = if *count > 1 {
            format!("{}~{}", sec.title, count)
        } else {
            sec.title.clone()
        };
        // Paragraphs, grouped up to MAX_BLOCK characters.
        let mut pieces: Vec<String> = vec![];
        let mut cur = String::new();
        for para in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            let para_len = para.chars().count();
            if para_len > MAX_BLOCK {
                if !cur.is_empty() {
                    pieces.push(std::mem::take(&mut cur));
                }
                let mut buf = String::new();
                for word in para.split_whitespace() {
                    if buf.chars().count() + word.chars().count() + 1 > MAX_BLOCK && !buf.is_empty()
                    {
                        pieces.push(std::mem::take(&mut buf));
                    }
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
                    buf.push_str(word);
                }
                if !buf.is_empty() {
                    pieces.push(buf);
                }
            } else if cur.chars().count() + para_len + 2 > MAX_BLOCK && !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur));
                cur = para.to_string();
            } else {
                if !cur.is_empty() {
                    cur.push_str("\n\n");
                }
                cur.push_str(para);
            }
        }
        if !cur.is_empty() {
            pieces.push(cur);
        }
        for (n, piece) in pieces.into_iter().enumerate() {
            let cleaned: String = piece
                .chars()
                .filter(|c| !c.is_control() || c.is_whitespace())
                .collect();
            let text = format!("{} — {}", sec.title, text::normalize(&cleaned));
            blocks.push((
                format!("{rel}#{title_key}/{}", n + 1),
                text,
                sec.first_line,
                sec.last_line,
            ));
        }
    }

    // Blocks that are too short join the next one (or the previous one if there is no next).
    let mut merged: Vec<(String, String, usize, usize)> = vec![];
    let mut carry: Option<(String, String, usize, usize)> = None;
    for b in blocks {
        let b = match carry.take() {
            Some((r, t, first, _)) => (r, format!("{t} {}", b.1), first, b.3),
            None => b,
        };
        if b.1.chars().count() < MIN_BLOCK {
            carry = Some(b);
        } else {
            merged.push(b);
        }
    }
    if let Some(last) = carry {
        match merged.last_mut() {
            Some(prev) => {
                prev.1 = format!("{} {}", prev.1, last.1);
                prev.3 = last.3;
            }
            None => merged.push(last),
        }
    }
    merged
        .into_iter()
        .filter(|(_, t, _, _)| !t.trim().is_empty())
        .map(|(reference, text, a, b)| Block {
            reference,
            text,
            range: format!("{a}-{b}"),
        })
        .collect()
}

/// True if the text looks like a secret: private key, `password: value`, `Bearer` token,
/// long alphanumeric run mixing letters and digits. Cautious: better to leave out one passage
/// too many than to keep a secret in a memory that agents query.
pub fn looks_secret(t: &str) -> bool {
    if t.contains("-----BEGIN") {
        return true;
    }
    let folded = text::fold(t);
    let words: Vec<&str> = t.split_whitespace().collect();
    // A keyword followed by ":" or "=" then a value that looks like a secret.
    const KEYS: [&str; 10] = [
        "password",
        "passwd",
        "mot de passe",
        "mdp",
        "token",
        "jeton",
        "secret",
        "api key",
        "apikey",
        "api_key",
    ];
    for key in KEYS {
        let mut from = 0;
        while let Some(pos) = folded[from..].find(key) {
            let after = &folded[from + pos + key.len()..];
            // The separator may follow one or two words ("wifi password: ..."), not a sentence.
            let window: String = after.chars().take(24).collect();
            if let Some(sep) = window.find([':', '='])
                && !window[..sep].contains(['.', '?', '!', ';', '\n'])
            {
                let rest = &after[sep + 1..];
                let value: String = rest
                    .trim_start_matches(|c: char| {
                        c == ' ' || c == '`' || c == '"' || c == '\\' || c == '*'
                    })
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != '`' && *c != '"')
                    .collect();
                if value.chars().count() >= 8 && value.chars().any(|c| c.is_ascii_digit()) {
                    return true;
                }
            }
            from += pos + key.len();
        }
    }
    for (i, w) in words.iter().enumerate() {
        let lw = w.to_lowercase();
        if lw == "bearer"
            && let Some(next) = words.get(i + 1)
            && next.chars().filter(|c| c.is_ascii_alphanumeric()).count() >= 16
        {
            return true;
        }
        let core = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if core.len() >= 24
            && !lw.starts_with("http")
            && !core.contains('/')
            && core
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+' || c == '=')
            && core.chars().any(|c| c.is_ascii_digit())
            && core.chars().any(|c| c.is_ascii_alphabetic())
        {
            return true;
        }
    }
    false
}

struct FileEntry {
    rel: String,
    path: PathBuf,
}

enum Class {
    Importable,
    Protected,
    Excluded,
    Instruction,
    Ignored,
}

fn classify(rel: &str, only: Option<&str>, rules: &ImportRules, extra: &[String]) -> Class {
    if !rel.to_lowercase().ends_with(".md") {
        return Class::Ignored;
    }
    if let Some(prefix) = only {
        let p = prefix.trim_matches('/');
        if !(rel == p || rel.starts_with(&format!("{p}/"))) {
            return Class::Ignored;
        }
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if is_protected(name, extra) {
        return Class::Protected;
    }
    let segments: Vec<String> = rel.split('/').map(text::fold).collect();
    let dirs = &segments[..segments.len().saturating_sub(1)];
    let folded_dir = dirs.join("/");
    let excluded_by_rules = rules
        .excluded_dirs
        .iter()
        .any(|e| folded_dir == *e || folded_dir.starts_with(&format!("{e}/")) || dirs.contains(e));
    if dirs
        .iter()
        .any(|d| ALWAYS_EXCLUDED_DIRS.contains(&d.as_str()))
        || excluded_by_rules
    {
        return Class::Excluded;
    }
    if is_instruction(rel, &rules.instructions) || dirs.iter().any(|d| d == "skills") {
        return Class::Instruction;
    }
    Class::Importable
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<FileEntry>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        // No symlinks: never leave the imported folder.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, out);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel: String = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().nfc().collect::<String>())
                .collect::<Vec<_>>()
                .join("/");
            out.push(FileEntry { rel, path });
        }
    }
}

struct Open<'a> {
    brain: &'a Brain,
    conns: HashMap<Space, Connection>,
}

impl Open<'_> {
    fn get(&mut self, s: &Space) -> Result<&Connection> {
        if !self.conns.contains_key(s) {
            let c = self.brain.open_space(s)?;
            c.execute_batch("PRAGMA synchronous = NORMAL;")?;
            self.conns.insert(s.clone(), c);
        }
        self.conns
            .get(s)
            .ok_or_else(|| Error::Schema("missing connection".into()))
    }
}

/// The safeguard: a first real run requires a recent `--dry-run` on the same scope.
fn dry_run_key(vault: &Path, only: &Option<String>) -> String {
    let id = format!("{}|{}", vault.display(), only.as_deref().unwrap_or(""));
    format!("import_dryrun:{}", &sha_hex(id.as_bytes())[..12])
}

/// Reads a folder of notes into passages (facts of kind `passage`, source `note`), routed to spaces by `rules`.
/// Idempotent: a passage whose text is unchanged is left alone, a changed one is `supersede`d, one that left the
/// note is `retire`d, a renamed note moves its pointers. Passages that look like a secret are left out and only
/// their place is reported. A first real run needs a recent dry run on the same scope unless `opts.yes`.
///
/// # Errors
/// [`Error::Invalid`] if `vault` is not a folder, a space in the rules does not exist, or a real first run has no recent dry run.
///
/// # Effects
/// Facts and history events in the target spaces (nothing in the folder: it is only read).
pub fn import(
    brain: &Brain,
    vault: &Path,
    rules: &ImportRules,
    opts: &ImportOptions,
) -> Result<Report> {
    if !vault.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a directory",
            vault.display()
        )));
    }
    for s in rules.spaces() {
        if !brain.has_space(&s)? {
            return Err(Error::Invalid(format!(
                "the import rules use space {s}, which does not exist (create it with `space add {s}`)"
            )));
        }
    }
    let ctl = brain.open_control()?;
    let key = dry_run_key(vault, &opts.only);
    let now = util::now();
    if !opts.dry_run && !opts.yes {
        let last: Option<String> = ctl
            .query_row("SELECT v FROM meta WHERE k = ?1", [&key], |r| r.get(0))
            .optional()?;
        let recent = last
            .and_then(|v| v.parse::<i64>().ok())
            .is_some_and(|t| now - t <= DRY_RUN_VALID_SECS);
        if !recent {
            return Err(Error::Invalid(
                "first run on this scope: run `import --dry-run` first (or add --yes)".into(),
            ));
        }
    }

    let mut files = vec![];
    walk(vault, vault, &mut files);
    let mut report = Report {
        dry_run: opts.dry_run,
        ..Default::default()
    };
    let extra: Vec<String> = rules
        .protected
        .iter()
        .chain(&opts.extra_protected)
        .map(|e| text::fold(e))
        .collect();
    let mut importable: Vec<FileEntry> = vec![];
    for f in files {
        match classify(&f.rel, opts.only.as_deref(), rules, &extra) {
            Class::Importable => importable.push(f),
            Class::Protected => report.protected += 1,
            Class::Excluded => report.excluded += 1,
            Class::Instruction => report.instructions += 1,
            Class::Ignored => {}
        }
    }
    let present: HashSet<String> = importable.iter().map(|f| f.rel.clone()).collect();

    let mut open = Open {
        brain,
        conns: HashMap::new(),
    };
    for f in &importable {
        let meta = fs::metadata(&f.path)?;
        let bytes = fs::read(&f.path)?;
        let content = String::from_utf8_lossy(&bytes).into_owned();
        report.read += 1;
        let space = rules.space_for(&f.rel);
        let hash = sha_hex(&bytes);
        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let conn = open.get(&space)?;
        import_note(
            conn,
            &space,
            &f.rel,
            &content,
            &hash,
            mtime,
            &present,
            opts.dry_run,
            &mut report,
        )?;
    }

    // Known notes no longer in the folder (deleted, moved, now protected or excluded).
    for space in rules.spaces() {
        let conn = open.get(&space)?;
        let known: Vec<String> = {
            let mut st = conn.prepare("SELECT path FROM notes WHERE state = 'imported'")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for path in known {
            let in_scope = opts.only.as_deref().is_none_or(|p| {
                let p = p.trim_matches('/');
                path == p || path.starts_with(&format!("{p}/"))
            });
            if !in_scope || present.contains(&path) {
                continue;
            }
            report.missing_notes += 1;
            for fact in facts::active_passages_of(conn, &path)? {
                report.retired += 1;
                if !opts.dry_run {
                    facts::retire_as(conn, AUTHOR, &fact.id, "source_removed", "import_retire")?;
                }
            }
            if !opts.dry_run {
                conn.execute(
                    "UPDATE notes SET state = 'missing' WHERE path = ?1",
                    [&path],
                )?;
            }
        }
    }

    if opts.dry_run {
        ctl.execute(
            "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = ?2",
            params![key, now.to_string()],
        )?;
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn import_note(
    conn: &Connection,
    space: &Space,
    rel: &str,
    content: &str,
    hash: &str,
    mtime: i64,
    present: &HashSet<String>,
    dry: bool,
    report: &mut Report,
) -> Result<()> {
    let known: Option<(String, String)> = conn
        .query_row(
            "SELECT hash, state FROM notes WHERE path = ?1",
            [rel],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((h, state)) = &known
        && h == hash
        && state == "imported"
    {
        report.unchanged += 1;
        return Ok(());
    }
    let all_blocks = chunk(rel, content);
    let before = all_blocks.len();
    let blocks: Vec<Block> = all_blocks
        .into_iter()
        .filter(|b| !looks_secret(&b.text))
        .collect();
    report.secrets_masked += before - blocks.len();
    if before != blocks.len() && report.masked_refs.len() < 200 {
        report.masked_refs.push(rel.to_string());
    }
    let existing = facts::active_passages_of(conn, rel)?;
    let mut by_ref: HashMap<String, Fact> = existing
        .into_iter()
        .filter_map(|f| f.source_ref.clone().map(|r| (r, f)))
        .collect();

    for b in &blocks {
        let source = Source::Note {
            path: rel.to_string(),
            hash: hash.to_string(),
            range: Some(b.range.clone()),
            reference: Some(b.reference.clone()),
        };
        if let Some(old) = by_ref.remove(&b.reference) {
            if text::text_hash(&old.text) == text::text_hash(&b.text) {
                continue; // same passage, nothing to do
            }
            report.replaced += 1;
            if !dry {
                let new = NewFact {
                    kind: "passage".into(),
                    text: b.text.clone(),
                    source,
                    valid_from: date_from_name(rel),
                    valid_to: None,
                    proposed: false,
                    tags: vec![],
                };
                facts::supersede_as(
                    conn,
                    space,
                    AUTHOR,
                    &old.id,
                    new,
                    Some("note changed"),
                    "import_replace",
                )?;
            }
            continue;
        }
        // Does the same text already exist, active, elsewhere?
        match facts::active_with_text(conn, &b.text)? {
            Some(other) if other.source_path.as_deref() == Some(rel) => {
                // Same note, heading or number changed: the pointer follows.
                by_ref.retain(|_, f| f.id != other.id);
                report.repointed += 1;
                if !dry {
                    facts::repoint(conn, AUTHOR, &other.id, &source)?;
                }
            }
            Some(other) => {
                let other_gone = other
                    .source_path
                    .as_ref()
                    .is_none_or(|p| !present.contains(p))
                    || other.source_kind != "note";
                if other_gone {
                    report.repointed += 1;
                    if !dry {
                        facts::repoint(conn, AUTHOR, &other.id, &source)?;
                    }
                } else {
                    report.duplicates += 1;
                }
            }
            None => {
                report.created += 1;
                *report
                    .created_by_space
                    .entry(space.as_str().to_string())
                    .or_default() += 1;
                if !dry {
                    facts::create_passage(
                        conn,
                        space,
                        AUTHOR,
                        &b.text,
                        source,
                        date_from_name(rel),
                    )?;
                }
            }
        }
    }
    // What remained of this note and no longer exists: retired, never erased.
    for (_, gone) in by_ref {
        report.retired += 1;
        if !dry {
            facts::retire_as(conn, AUTHOR, &gone.id, "source_removed", "import_retire")?;
        }
    }
    if !dry {
        conn.execute(
            "INSERT INTO notes (path, hash, mtime, imported_at, blocks, state) VALUES (?1, ?2, ?3, ?4, ?5, 'imported')
             ON CONFLICT(path) DO UPDATE SET hash = ?2, mtime = ?3, imported_at = ?4, blocks = ?5, state = 'imported'",
            params![rel, hash, mtime, util::now(), i64::try_from(blocks.len()).unwrap_or(0)],
        )?;
    }
    Ok(())
}
