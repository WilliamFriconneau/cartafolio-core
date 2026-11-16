//! A note as a Markdown file: the beginner's case, a folder of notes.

use crate::normalize::normalize;

/// A file to write in the space's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile {
    pub file_name: String,
    pub content: String,
}

fn yaml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `AAAA-MM-JJ-HHMM-<first words>.md`, stable for a note: its local time
/// `created_at` (RFC 3339, read as written) and its first five words, without
/// accents; `note` when there are none.
pub fn file_name(created_at: &str, text: &str) -> String {
    let date = created_at.get(0..10).unwrap_or("0000-00-00");
    let hour = created_at.get(11..13).unwrap_or("00");
    let minute = created_at.get(14..16).unwrap_or("00");
    let words: Vec<String> = normalize(text).into_iter().take(5).collect();
    let mut slug = words.join("-");
    slug.truncate(60);
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "note" } else { slug };
    format!("{date}-{hour}{minute}-{slug}.md")
}

/// The file of a note: a YAML header (date, spaces, source: kalios), then the
/// text, kept as written.
pub fn note_file(created_at: &str, space_names: &[String], text: &str) -> MarkdownFile {
    let spaces = space_names
        .iter()
        .map(|n| yaml_string(n))
        .collect::<Vec<_>>()
        .join(", ");
    let content = format!(
        "---\ndate: {}\nspaces: [{}]\nsource: kalios\n---\n\n{}\n",
        yaml_string(created_at),
        spaces,
        text.trim_end()
    );
    MarkdownFile {
        file_name: file_name(created_at, text),
        content,
    }
}
