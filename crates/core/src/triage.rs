//! « À trier »: the one box every capturing device of a person writes to. It belongs to no vault and is no vault: a
//! device never chooses where a capture goes. The janitor sends each note to the inbox of a vault, then files it there;
//! what it is unsure of, and the files, wait here until the person decides on their page.
//!
//! It is a space of the kind [`TRIAGE`], made the first time a device is paired, named after the person
//! (`alice-a-trier`). A device's key writes only its inbox and reads nothing back.

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use crate::catalogue::{self, TRIAGE};
use crate::facts::{self, NewFact, RecallOptions, Remembered};
use crate::{Brain, Error, Result, Space, files};

/// The person's « À trier », if it was made.
pub fn of(brain: &Brain, user_id: &str) -> Result<Option<Space>> {
    let name: Option<String> = brain
        .open_control()?
        .query_row(
            "SELECT s.name FROM spaces s JOIN space_members m ON m.space = s.name AND m.owner = 1
             WHERE m.user_id = ?1 AND s.kind = ?2 ORDER BY s.name LIMIT 1",
            [user_id, TRIAGE],
            |r| r.get(0),
        )
        .optional()?;
    name.map(|n| Space::new(&n)).transpose()
}

/// The person's « À trier », made now if they have none.
pub fn ensure(brain: &Brain, user_id: &str, person: &str) -> Result<Space> {
    if let Some(s) = of(brain, user_id)? {
        return Ok(s);
    }
    let space = catalogue::vault_name(person, TRIAGE)?;
    brain.add_space_for(&space, user_id)?;
    brain.open_control()?.execute(
        "UPDATE spaces SET kind = ?2 WHERE name = ?1",
        [space.as_str(), TRIAGE],
    )?;
    Ok(space)
}

/// Whether `space` is an « À trier ».
pub fn is_triage(brain: &Brain, space: &Space) -> bool {
    brain.space_kind(space).is_ok_and(|k| k == TRIAGE)
}

/// The tag of the note in which the janitor says why it could not place what waits (`{"version":1,"reasons":{id: why}}`).
pub const REASONS_TAG: &str = "janitor/reasons";

/// The reasons the janitor gave, by id of the note or file that waits.
fn reasons(conn: &rusqlite::Connection) -> BTreeMap<String, String> {
    let text: Option<String> = conn
        .query_row(
            "SELECT f.text FROM facts f WHERE f.status = 'active' AND f.id IN
             (SELECT fact_id FROM fact_tags WHERE tag = ?1) ORDER BY f.created_at DESC LIMIT 1",
            [REASONS_TAG],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    text.and_then(|t| serde_json::from_str::<Value>(t.trim()).ok())
        .and_then(|d| d.get("reasons").and_then(Value::as_object).cloned())
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// What waits in an « À trier » **for a decision**: its notes (a recording's own description apart: the node reads the
/// recording and puts its text here) and its files, newest first, each with its `age_seconds` and the `reason` the janitor
/// left (why it could not place it). Not counted as waiting: a recording whose text is already here (`transcribed`), and one
/// whose transcription failed three times and carries the tag [`files::FAILED_TRANSCRIPTION`] (`failed_transcription`, a
/// list apart).
pub fn waiting(brain: &Brain, space: &Space, limit: usize) -> Result<Value> {
    let conn = brain.open_space(space)?;
    let now = crate::util::now();
    let why = reasons(&conn);
    let reason_of = |id: &str| {
        why.get(id)
            .cloned()
            .unwrap_or_else(|| "not looked at by the janitor yet".to_string())
    };
    let opts = RecallOptions {
        limit,
        tags: vec!["inbox".into()],
        ..RecallOptions::default()
    };
    let mut notes: Vec<Value> = facts::recall(&conn, "", &opts, now)?
        .into_iter()
        .map(|h| h.fact)
        .filter(|f| f.typ.as_deref() != Some("file"))
        .map(|f| {
            json!({ "id": f.id, "text": f.text, "tags": f.tags,
                "created_at": crate::util::iso(f.created_at),
                "age_seconds": (now - f.created_at).max(0), "reason": reason_of(&f.id) })
        })
        .collect();
    notes.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    // A recording whose text is already here is not waiting for anybody: it stays until its week is over and the node
    // erases it. A recording the node gave up on carries a tag and is counted apart. Only what nothing has read yet, and
    // is for the person to decide, is waiting.
    let mut transcribed = 0usize;
    let mut kept: Vec<Value> = vec![];
    let mut failed: Vec<Value> = vec![];
    for f in files::list(brain, space, None, limit.max(1000))? {
        let mut json = f.to_json();
        json["age_seconds"] = json!((now - f.created_at).max(0));
        let tags: Vec<String> = match &f.fact_id {
            Some(fid) => facts::read_fact(&conn, fid)?
                .map(|x| x.tags)
                .unwrap_or_default(),
            None => vec![],
        };
        if f.is_audio() {
            if tags.iter().any(|t| t == files::FAILED_TRANSCRIPTION) {
                json["tags"] = json!(tags);
                json["reason"] = json!(
                    "the transcription failed three times; take the tag failed/transcription off to retry"
                );
                failed.push(json);
                continue;
            }
            let named = conn
                .query_row(
                    "SELECT 1 FROM facts WHERE status = 'active' AND type IS NOT 'file' AND instr(text, ?1) > 0 LIMIT 1",
                    [&f.name],
                    |_| Ok(()),
                )
                .is_ok();
            if named {
                transcribed += 1;
                continue;
            }
        }
        json["tags"] = json!(tags);
        json["reason"] = json!(reason_of(&f.id));
        kept.push(json);
    }
    kept.truncate(limit);
    failed.truncate(limit);
    let count = notes.len() + kept.len();
    Ok(json!({
        "space": space.as_str(), "notes": notes, "files": kept, "transcribed": transcribed,
        "failed_transcription": failed, "count": count,
    }))
}

/// Sends one note or one file of `from` (an « À trier ») to the inbox of the vault `to`, as it is; then it leaves
/// `from`, once the write is certain. The id of what holds it now.
pub fn send(brain: &Brain, from: &Space, id: &str, to: &Space, author: &str) -> Result<String> {
    if !is_triage(brain, from) {
        return Err(Error::Invalid(format!("{from} is not an « À trier »")));
    }
    if is_triage(brain, to) || !brain.has_space(to)? {
        return Err(Error::Invalid(format!("{to} is not a vault")));
    }
    let reason = |new: &str| format!("sent from À trier to the inbox of {to} ({new}) by {author}");
    if let Some(f) = files::info(brain, from, id)?.filter(|f| f.status == "active") {
        let (_, bytes) =
            files::get(brain, from, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))?;
        let tags = match &f.fact_id {
            Some(fid) => facts::read_fact(&brain.open_space(from)?, fid)?
                .map(|x| x.tags)
                .unwrap_or_default(),
            None => vec!["inbox".into()],
        };
        let new = files::put_tagged(brain, to, author, &f.name, &f.note, &bytes, &tags)?;
        files::retire(brain, from, author, id, &reason(&new.id))?;
        return Ok(new.id);
    }
    let src = brain.open_space(from)?;
    let fact = facts::read_fact(&src, id)?
        .filter(|f| f.status == "active" && f.typ.as_deref() != Some("file"))
        .ok_or_else(|| Error::Invalid("nothing waits under this id".into()))?;
    let mut new = NewFact::agent(&fact.text, author);
    new.kind = fact.kind.clone();
    new.valid_from = Some(fact.valid_from);
    new.valid_to = fact.valid_to;
    new.tags = if fact
        .tags
        .iter()
        .any(|t| t == "inbox" || t.starts_with("inbox/"))
    {
        fact.tags.clone()
    } else {
        let mut t = fact.tags.clone();
        t.push("inbox".into());
        t
    };
    let dst = brain.open_space(to)?;
    let new_id = match facts::remember(&dst, to, author, new, &[])? {
        Remembered::Created(f) | Remembered::Duplicate(f) => f.id,
        // An inbox note is never held back by the ones it resembles.
        Remembered::NeedsDecision(_) => {
            return Err(Error::Invalid("the vault did not take the note".into()));
        }
    };
    facts::retire(&src, author, id, &reason(&new_id))?;
    Ok(new_id)
}
