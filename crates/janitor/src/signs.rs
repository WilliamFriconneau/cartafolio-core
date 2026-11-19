//! The signs sweep: the signs and symptoms, the measures and the red flags of the health referential, put as tags on the
//! notes of a health space that were written without them, wherever the note came from (a device, an assistant, an
//! import, a sync from another node, a mounted folder's journal). The write-time tagger only sees what is written
//! through it; this pass makes the memory whole.
//!
//! - It reads the notes of each health space newest first, a few hundred per pass, and goes on where it stopped. When it
//!   has been through them all it only looks at what is newer than its last full turn.
//! - It tags with the referential alone (`health_rule::detected_tags`), no model: what it finds is the vocabulary the
//!   person's own tags already follow. The text of a note is never changed (`tags_set` keeps the old version).
//! - It leaves alone what is not the person's words: the raw layer of an app (`sources/<app>`), a reference document
//!   (`.../Sources/...`), a mounted library, the janitor's own notes, the living boards, the rules.
//! - A short note (a sentence or two) also teaches the node: its words lead to the tags the referential found, so the next
//!   note that says it the same way is tagged when it is written. A long one only gets its tags, as its words say too much.

use memcp_client::Error as ClientError;
use serde_json::{Value, json};

use crate::{Janitor, Report, health_rule, is_library, now, tags_of};

/// Bump it to have every note looked at again (the referential grew, a rule changed).
const VERSION: u64 = 1;
/// Notes tagged in one pass at most, so that a first turn over a large memory spreads over the nights.
const PER_PASS: u32 = 600;
/// The size of a page read.
const PAGE: u64 = 200;
/// Up to this many characters a note is also a lesson for the node.
const SHORT_NOTE: usize = 600;

/// Tags that say a note is not the person's own words, or is not the janitor's to touch.
fn is_plumbing(tag: &str) -> bool {
    tag == "inbox"
        || tag == "dismissed"
        || tag == "library"
        || [
            "janitor", "living", "rules", "index", "sources/", "library/",
        ]
        .iter()
        .any(|p| tag == p.trim_end_matches('/') || tag.starts_with(p))
}

/// Whether a note of the chronology is left as it is: already tagged with the referential, not the person's words, or not
/// the janitor's to touch.
pub(crate) fn leave_alone(item: &Value) -> bool {
    let tags = tags_of(item);
    if tags.iter().any(|t| {
        t.starts_with("signe/")
            || t.starts_with("mesure/")
            || t.starts_with("alerte/")
            || is_plumbing(t)
    }) || is_library(item)
    {
        return true;
    }
    let path = item["provenance"]["source"]["path"].as_str().unwrap_or("");
    path.contains("/Sources/") || path.starts_with("Sources/")
}

impl Janitor {
    /// One bounded turn of the sweep over every health space.
    pub(crate) fn sweep_signs(&mut self, vaults: &[String], report: &mut Report) {
        let mut budget = PER_PASS;
        for vault in vaults {
            if budget == 0 {
                break;
            }
            match self.client.call(
                "space_view",
                json!({ "space": vault, "view": "capabilities" }),
            ) {
                Ok(cap) if cap["kind"] == "health" => {}
                Ok(_) => continue,
                Err(ClientError::Refused) => {
                    report.key_refused = true;
                    return;
                }
                Err(_) => continue,
            }
            if self.sweep_space(vault, &mut budget, report).is_err() {
                return;
            }
        }
    }

    /// Returns `Err` when the pass must stop (the key was refused).
    fn sweep_space(
        &mut self,
        vault: &str,
        budget: &mut u32,
        report: &mut Report,
    ) -> Result<(), ()> {
        let started = now().max(0).unsigned_abs();
        let (mut cursor, full_at) = self.state.signs_progress(vault, VERSION);
        let incremental = full_at > 0;
        loop {
            let mut args =
                json!({ "space": vault, "view": "chronology", "type": "note", "limit": PAGE });
            if let Some(c) = &cursor {
                args["cursor"] = json!(c);
            }
            let page = match self.client.call("space_view", args) {
                Ok(p) => p,
                Err(ClientError::Refused) => {
                    report.key_refused = true;
                    return Err(());
                }
                Err(e) => {
                    report.problems.push(format!(
                        "signs sweep, {vault}: the notes cannot be read: {e}"
                    ));
                    return Ok(());
                }
            };
            let mut reached_old = false;
            for item in page["items"].as_array().into_iter().flatten() {
                if incremental && item["created_at"].as_u64().is_some_and(|c| c < full_at) {
                    reached_old = true;
                    break;
                }
                if leave_alone(item) {
                    continue;
                }
                let (Some(id), Some(text)) = (item["id"].as_str(), item["text"].as_str()) else {
                    continue;
                };
                let found = health_rule::detected_tags(text);
                if found.is_empty() {
                    continue;
                }
                let done = self.client.call(
                    "tags_set",
                    json!({
                        "id": id,
                        "add": found,
                        "reason": "janitor: signs and measures of the health referential",
                        "learn": text.chars().count() <= SHORT_NOTE,
                    }),
                );
                match done {
                    Ok(_) => {
                        report.signs_tagged += 1;
                        *budget = budget.saturating_sub(1);
                    }
                    Err(ClientError::Refused) => {
                        report.key_refused = true;
                        return Err(());
                    }
                    Err(e) => report
                        .problems
                        .push(format!("signs sweep, {vault}: {id}: {e}")),
                }
                if *budget == 0 {
                    // Where this page started: what was tagged here is skipped when the next pass comes back to it.
                    self.state
                        .set_signs_progress(vault, VERSION, cursor.as_deref(), full_at);
                    return Ok(());
                }
            }
            let next = page["page"]["next_cursor"].as_str().map(String::from);
            if reached_old || next.is_none() || !page["page"]["has_more"].as_bool().unwrap_or(false)
            {
                // A full turn is done: from now on only what is newer than this turn's start is looked at.
                self.state.set_signs_progress(vault, VERSION, None, started);
                return Ok(());
            }
            cursor = next;
            self.state
                .set_signs_progress(vault, VERSION, cursor.as_deref(), full_at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(tags: &[&str], path: &str) -> Value {
        json!({ "tags": tags, "provenance": { "source": { "path": path } } })
    }

    #[test]
    fn the_persons_notes_are_swept_and_the_rest_is_left_alone() {
        assert!(!leave_alone(&note(&["health", "journal/2026-11-19"], "")));
        assert!(!leave_alone(&note(
            &["health"],
            "Santé/Journal/2026-10-03.md"
        )));
        assert!(leave_alone(&note(&["signe/fatigue"], "")));
        assert!(leave_alone(&note(&["sources/kalios"], "")));
        assert!(leave_alone(&note(&["janitor/copy"], "")));
        assert!(leave_alone(&note(&["living/targets/open"], "")));
        assert!(leave_alone(&note(
            &["health"],
            "Santé/Sources/Psychiatrie.md"
        )));
        assert!(leave_alone(&note(&["library/x"], "")));
    }
}
