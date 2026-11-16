//! Living next to a synchronised folder (Syncthing, iCloud, Drive): write
//! only over what we wrote, never over a hand-made change; leave alone the
//! copies a sync tool makes and the folders that are not notes.

/// FNV-1a 64 of a content, as 16 hex characters: change detection only.
pub fn content_hash(content: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteDecision {
    Write,
    /// Already there as we would write it.
    Skip,
    /// Someone changed the file since we wrote it: write ours beside it.
    ConflictCopy,
}

/// What to do with a note's file, from the hash of what is on disk (none:
/// no file), of what we last wrote there, and of what we would write now.
pub fn write_decision(
    on_disk: Option<&str>,
    last_written: Option<&str>,
    new: &str,
) -> WriteDecision {
    match (on_disk, last_written) {
        (None, _) => WriteDecision::Write,
        (Some(d), _) if d == new => WriteDecision::Skip,
        (Some(d), Some(w)) if d == w => WriteDecision::Write,
        _ => WriteDecision::ConflictCopy,
    }
}

/// A copy made by a sync tool: « nom (1).md », « .sync-conflict- »,
/// « conflicted copy », « nom-<machine>.md » when `machines` names it.
pub fn is_conflict_copy(file_name: &str, machines: &[String]) -> bool {
    let lower = file_name.to_lowercase();
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let numbered = stem.rsplit_once(" (").is_some_and(|(_, n)| {
        n.strip_suffix(')')
            .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
    });
    numbered
        || lower.contains(".sync-conflict-")
        || lower.contains("conflicted copy")
        || lower.contains("conflit de")
        || machines
            .iter()
            .any(|m| !m.is_empty() && stem.ends_with(&format!("-{}", m.to_lowercase())))
}

/// A file or folder the app never reads nor writes: hidden ones, `.git`,
/// `node_modules`, `target`, `.DS_Store`.
pub fn is_ignored(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "Icon\r")
}

/// The conflict copy's name: « nom (kalios <date>).md ».
pub fn conflict_name(file_name: &str, stamp: &str) -> String {
    match file_name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem} (kalios {stamp}).{ext}"),
        None => format!("{file_name} (kalios {stamp})"),
    }
}
