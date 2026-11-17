//! The captures also written as Markdown into a folder the person chose, for whoever keeps notes in Obsidian or any
//! folder of Markdown. A note is one `.md`; a document or a recording is copied beside its `.md`,
//! which embeds it (`![[name]]`). Nothing is read back: the folder is the person's.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::time::civil;

/// A folder of Markdown, and where the person is (for the dates).
pub struct Markdown {
    pub folder: PathBuf,
    pub utc_offset_minutes: i32,
}

/// One side of a live text: who spoke, and what Apple's recognition heard.
pub struct LiveLine {
    pub me: bool,
    pub text: String,
}

impl Markdown {
    /// A note: its words, dated. Returns the file written.
    pub fn note(&self, text: &str, at: i64) -> Result<PathBuf, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("an empty note".into()));
        }
        let body = format!("{}{text}\n", self.front("note", at));
        self.write(&format!("{} {}", self.stamp(at), title_of(text)), &body)
    }

    /// A document or a voice recording, copied into the folder, with a note beside it that embeds it.
    pub fn file(&self, source: &Path, note: &str, at: i64) -> Result<PathBuf, Error> {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Invalid("a file without a name".into()))?;
        let copy = self.copy(source, name)?;
        let embedded = copy.file_name().and_then(|n| n.to_str()).unwrap_or(name);
        let mut body = self.front("document", at);
        if !note.trim().is_empty() {
            body.push_str(note.trim());
            body.push_str("\n\n");
        }
        body.push_str(&format!("![[{embedded}]]\n"));
        let title = safe(stem(name));
        self.write(
            &format!(
                "{} {}",
                self.stamp(at),
                if title.is_empty() { "Document" } else { &title }
            ),
            &body,
        )
    }

    /// A meeting: its parts copied and embedded, and the live text of each side, which is Apple's quick recognition
    /// (the text that goes to the memory is made later): said as such.
    pub fn meeting(
        &self,
        title: &str,
        started_at: i64,
        parts: &[(PathBuf, u32)],
        live: &[LiveLine],
    ) -> Result<PathBuf, Error> {
        self.meeting_heard(title, started_at, parts, live, "", "")
    }

    /// [`Markdown::meeting`] with its transcript (Whisper on this machine) and its summary, when they were made.
    /// `parts` may be empty: the sound is not kept in this folder.
    pub fn meeting_heard(
        &self,
        title: &str,
        started_at: i64,
        parts: &[(PathBuf, u32)],
        live: &[LiveLine],
        heard: &str,
        summary: &str,
    ) -> Result<PathBuf, Error> {
        let mut body = self.front("meeting", started_at);
        let minutes: u32 = parts.iter().map(|(_, s)| s).sum::<u32>().div_ceil(60);
        body.push_str(&format!("# {}\n\n{minutes} min\n\n", readable(title)));
        for (path, _) in parts {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("part.m4a");
            let copy = self.copy(path, &format!("{} {name}", self.stamp(started_at)))?;
            let embedded = copy.file_name().and_then(|n| n.to_str()).unwrap_or(name);
            body.push_str(&format!("![[{embedded}]]\n"));
        }
        if !summary.trim().is_empty() {
            body.push_str(&format!(
                "\n## Summary (Apple Intelligence)\n\n{}\n",
                summary.trim()
            ));
        }
        if !heard.trim().is_empty() {
            body.push_str(&format!("\n## Transcript (Whisper)\n\n{}\n", heard.trim()));
        }
        if !live.is_empty() && heard.trim().is_empty() {
            body.push_str("\n## Live text (Apple, approximate)\n\n");
            for line in live {
                body.push_str(&format!(
                    "- **{}** {}\n",
                    if line.me { "Me" } else { "Others" },
                    line.text.trim()
                ));
            }
        }
        self.write(
            &format!("{} {}", self.stamp(started_at), readable(title)),
            &body,
        )
    }

    fn front(&self, kind: &str, at: i64) -> String {
        let (y, mo, d, h, mi) = civil(at, self.utc_offset_minutes);
        let off = self.utc_offset_minutes;
        let sign = if off < 0 { '-' } else { '+' };
        format!(
            "---\ndate: {y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}{sign}{:02}:{:02}\ntype: {kind}\nsource: memcp-capture\ntags: [memcp]\n---\n\n",
            off.abs() / 60,
            off.abs() % 60
        )
    }

    fn stamp(&self, at: i64) -> String {
        let (y, mo, d, h, mi) = civil(at, self.utc_offset_minutes);
        format!("{y:04}-{mo:02}-{d:02} {h:02}h{mi:02}")
    }

    /// Writes `name.md`, never over another file: « name 2.md » then.
    fn write(&self, name: &str, body: &str) -> Result<PathBuf, Error> {
        let (path, mut file) = create(&self.folder, name, "md")?;
        file.write_all(body.as_bytes())
            .map_err(|e| Error::Folder(e.to_string()))?;
        Ok(path)
    }

    fn copy(&self, source: &Path, name: &str) -> Result<PathBuf, Error> {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let base = safe(stem(name));
        let (path, mut file) = create(
            &self.folder,
            if base.is_empty() { "file" } else { &base },
            ext,
        )?;
        let mut from = fs::File::open(source).map_err(|e| Error::Folder(e.to_string()))?;
        std::io::copy(&mut from, &mut file).map_err(|e| Error::Folder(e.to_string()))?;
        Ok(path)
    }
}

/// A file created under a name not taken yet in `folder`: the creation itself takes the name (`create_new`), so two
/// captures written at once never get the same file (found by the stress test).
fn create(folder: &Path, name: &str, ext: &str) -> Result<(PathBuf, fs::File), Error> {
    let with = |n: &str| {
        if ext.is_empty() {
            folder.join(n)
        } else {
            folder.join(format!("{n}.{ext}"))
        }
    };
    for i in 1..10_000 {
        let path = if i == 1 {
            with(name)
        } else {
            with(&format!("{name} {i}"))
        };
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Error::Folder(e.to_string())),
        }
    }
    Err(Error::Folder(format!("no free name for {name}")))
}

fn stem(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
}

/// The first words of a note, six at most, as a file name.
fn title_of(text: &str) -> String {
    let first = text.lines().next().unwrap_or(text);
    let cleaned = safe(first);
    let title = cleaned
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "Note".into()
    } else {
        title
    }
}

/// « reunion-teams » as « Reunion teams ».
fn readable(title: &str) -> String {
    let spaced = safe(&title.replace(['-', '_'], " "));
    let mut chars = spaced.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_else(|| "Meeting".into())
}

/// A file name Obsidian and every system accept: no / \ : * ? " < > | # ^ [ ], no leading dot, at most 80 chars.
fn safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|#^[]".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned
        .trim_start_matches('.')
        .chars()
        .take(80)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bridge-md-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("fixture")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    // 2026-11-05 12:30 UTC, 13 h 30 in Paris (UTC+1).
    const AT: i64 = 1_793_881_800;

    #[test]
    fn a_note_is_one_dated_file_named_after_its_first_words() {
        let md = Markdown {
            folder: folder(),
            utc_offset_minutes: 60,
        };
        let path = md
            .note(
                "Appeler le fournisseur: devis / relance #urgent\nensuite",
                AT,
            )
            .expect("fixture");
        let name = path
            .file_name()
            .expect("fixture")
            .to_str()
            .expect("fixture")
            .to_string();
        assert_eq!(
            name,
            "2026-11-05 13h30 Appeler le fournisseur devis relance urgent.md"
        );
        let body = fs::read_to_string(&path).expect("fixture");
        assert!(body.starts_with("---\ndate: 2026-11-05T13:30+01:00\ntype: note\n"));
        assert!(body.ends_with("Appeler le fournisseur: devis / relance #urgent\nensuite\n"));
        // The same again: never over the first one.
        let again = md
            .note(
                "Appeler le fournisseur: devis / relance #urgent\nensuite",
                AT,
            )
            .expect("fixture");
        assert_ne!(again, path);
    }

    #[test]
    fn a_document_is_copied_and_embedded() {
        let src = folder().join("Devis fictif.pdf");
        fs::write(&src, b"%PDF fictif").expect("fixture");
        let md = Markdown {
            folder: folder(),
            utc_offset_minutes: 60,
        };
        let path = md.file(&src, "à relire", AT).expect("fixture");
        let body = fs::read_to_string(&path).expect("fixture");
        assert!(
            body.contains("à relire\n\n![[Devis fictif.pdf]]\n"),
            "{body}"
        );
        assert!(md.folder.join("Devis fictif.pdf").exists());
    }

    #[test]
    fn a_meeting_embeds_its_parts_and_says_its_live_text_is_approximate() {
        let src = folder().join("part-1.m4a");
        fs::write(&src, b"audio fictif").expect("fixture");
        let md = Markdown {
            folder: folder(),
            utc_offset_minutes: 60,
        };
        let live = [
            LiveLine {
                me: true,
                text: "Bonjour".into(),
            },
            LiveLine {
                me: false,
                text: "Bonjour à tous".into(),
            },
        ];
        let path = md
            .meeting("reunion-teams", AT, &[(src, 125)], &live)
            .expect("fixture");
        assert!(path.ends_with("2026-11-05 13h30 Reunion teams.md"));
        let body = fs::read_to_string(&path).expect("fixture");
        assert!(body.contains("type: meeting"));
        assert!(body.contains("# Reunion teams\n\n3 min\n"));
        assert!(body.contains("![[2026-11-05 13h30 part-1.m4a]]"));
        assert!(body.contains("## Live text (Apple, approximate)"));
        assert!(body.contains("- **Others** Bonjour à tous"));
    }

    #[test]
    fn an_empty_note_is_refused() {
        let md = Markdown {
            folder: folder(),
            utc_offset_minutes: 0,
        };
        assert!(md.note("   ", AT).is_err());
    }
}
