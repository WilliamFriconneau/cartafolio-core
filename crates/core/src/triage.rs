//! Inbox: the one box every capturing device of a person writes to. It belongs to no vault and is no vault: a
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

/// The person's Inbox, if it was made.
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

/// The person's Inbox, made now if they have none.
pub fn ensure(brain: &Brain, user_id: &str, person: &str) -> Result<Space> {
    if let Some(s) = of(brain, user_id)? {
        return Ok(s);
    }
    let space = catalogue::vault_name(person, TRIAGE)?;
    // A box an earlier version left `custom` (a crash between its creation and its kind) is put right.
    if brain.has_space(&space)?
        && crate::users::is_space_owner(&brain.open_control()?, &space, user_id)?
        && brain.space_kind(&space)? == catalogue::CUSTOM
    {
        brain.open_control()?.execute(
            "UPDATE spaces SET kind = ?2 WHERE name = ?1",
            [space.as_str(), TRIAGE],
        )?;
        return Ok(space);
    }
    // Made a box in the one transaction that makes it: never a `custom` space left by a crash.
    let mut new = crate::brain::NewSpace::owned_by(user_id);
    new.kind = TRIAGE.to_string();
    brain.make_space(&space, &new, &|_| Ok(()))?;
    Ok(space)
}

/// Whether `space` is an Inbox.
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

/// What waits in an Inbox **for a decision**: its notes (a recording's own description apart: the node reads the
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

/// Sends one note or one file of `from` (an Inbox) to the inbox of the vault `to`, as it is; then it leaves
/// `from`, once the write is certain. The id of what holds it now.
/// `user_id` is the person the space belongs to: the note gets the tags of `to` and what its text says there, as if it
/// had been written in it (the same as `remember`), with what that person taught the node.
pub fn send(
    brain: &Brain,
    from: &Space,
    id: &str,
    to: &Space,
    author: &str,
    user_id: &str,
) -> Result<String> {
    if !is_triage(brain, from) {
        return Err(Error::Invalid(format!("{from} is not the person's Inbox")));
    }
    if is_triage(brain, to) || !brain.has_space(to)? {
        return Err(Error::Invalid(format!("{to} is not a space")));
    }
    let reason = |new: &str| format!("sent from the Inbox to {to} ({new}) by {author}");
    if let Some(f) = files::info(brain, from, id)?.filter(|f| f.status == "active") {
        // Read from where it is kept, a block at a time: a big recording is never held whole.
        let (_, path) = files::path_of(brain, from, id)?
            .ok_or_else(|| Error::Invalid("unknown file".into()))?;
        let mut tags = match &f.fact_id {
            Some(fid) => facts::read_fact(&brain.open_space(from)?, fid)?
                .map(|x| x.tags)
                .unwrap_or_default(),
            None => vec!["inbox".into()],
        };
        crate::autotag::add_to(
            brain,
            user_id,
            to,
            &brain.open_space(to)?,
            &f.note,
            &mut tags,
        );
        let new = files::put_path(brain, to, author, &f.name, &f.note, &path, &tags)?;
        files::retire(brain, from, author, id, &reason(&new.id))?;
        learn_route(
            brain,
            author,
            &format!("{} {}", f.note, f.name.replace(['-', '_', '.'], " ")),
            &tags,
            to,
        );
        return Ok(new.id);
    }
    let src = brain.open_space(from)?;
    let fact = facts::read_fact(&src, id)?
        .filter(|f| f.status == "active" && f.typ.as_deref() != Some("file"))
        .ok_or_else(|| Error::Invalid("nothing waits under this id".into()))?;
    let mut new = NewFact::agent(&fact.text, author);
    new.kind = Some(fact.kind.clone());
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
    crate::autotag::add_to(brain, user_id, to, &dst, &fact.text, &mut new.tags);
    // The same note sent to the same space has the same id on every node: two janitors of the swarm that file it
    // before they hear of each other make one note, not two (the second copy is the same row when it arrives).
    let new_id = match facts::remember_as(&dst, to, author, new, &filed_id(id, to))? {
        Remembered::Created(f) | Remembered::Duplicate(f) => f.id,
        // An inbox note is never held back by the ones it resembles.
        Remembered::NeedsDecision(_) => {
            return Err(Error::Invalid("the space did not take the note".into()));
        }
    };
    facts::retire(&src, author, id, &reason(&new_id))?;
    learn_route(brain, author, &fact.text, &fact.tags, to);
    Ok(new_id)
}

/// The id of note `id` of Inbox once filed in `to`: derived from both, so it is the same on every node.
pub fn filed_id(id: &str, to: &Space) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{id}\u{0}{}", to.as_str()).as_bytes());
    format!("f_t{}", &crate::text::hex(&digest)[..20])
}

/// The person chose where a note of Inbox goes: the node learns it (the words of the note lead to that space).
/// What the janitor sends is its own choice, never learned from.
/// The people of the note (its `personnes/` tags) are learned with it: the next note about them goes there too.
fn learn_route(brain: &Brain, author: &str, text: &str, tags: &[String], to: &Space) {
    if !author.to_lowercase().contains("janitor") {
        let store = crate::autotag::Store::open(brain.dir());
        store.learn_route(text, to.as_str());
        store.learn_people_route(&crate::call::people_of(tags), to.as_str());
    }
}
