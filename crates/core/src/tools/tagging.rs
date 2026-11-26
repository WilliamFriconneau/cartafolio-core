//! Automatic tagging at the node: the tags a note gets when it is written (`on_write`), the tool that previews them
//! (`tags_suggest`), the tool that corrects them (`tags_set`), and what the node learns from a correction.

use super::*;
use crate::autotag;

/// The suggestion for a text in a space (see [`autotag::for_note`]).
fn suggestion(
    brain: &Brain,
    caller: &Caller,
    space: &Space,
    conn: &Connection,
    text: &str,
) -> autotag::Suggestion {
    autotag::for_note(brain, &caller.user_id, space, conn, text)
}

/// Adds to `tags` what the node finds in `text` for `space` (see [`autotag::add_to`]). Nothing is added for a connection
/// limited to paths: it writes only under them.
pub(super) fn on_write(
    brain: &Brain,
    caller: &Caller,
    space: &Space,
    conn: &Connection,
    text: &str,
    tags: &mut Vec<String>,
) {
    if !caller.paths.contains_key(space) {
        autotag::add_to(brain, &caller.user_id, space, conn, text, tags);
    }
}

/// What the person's change of tags teaches, for the note `text` of `space`.
pub(super) fn learn_change(
    brain: &Brain,
    agent: &str,
    space: &Space,
    text: &str,
    old: &[String],
    new: &[String],
) -> bool {
    autotag::learn_change(brain, agent, space, text, old, new)
}

pub(super) fn tags_suggest(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let text = str_arg(args, "text")?;
    check_len(caller, text)?;
    let mut out = json!({ "notice": NOTICE, "space": Value::Null, "tags": [], "fields": {}, "field_tags": [], "all_tags": [] });
    if let Some(name) = args.get("space").and_then(Value::as_str) {
        let space = Space::new(name)?;
        if !caller.readable().contains(&space) || caller.read_paths(&space).is_some() {
            return Err(ToolError::Denied(format!("no read right on {space}")));
        }
        let conn = brain.open_space(&space)?;
        let s = suggestion(brain, caller, &space, &conn, text);
        out = json!({
            "space": name,
            "tags": s.tags,
            "fields": s.fields,
            "field_tags": s.field_tags(),
            "all_tags": s.all_tags(),
        });
    }
    // Where the person has sent notes like this one, as far as the node learned it.
    let readable = caller.readable();
    let route: Vec<Value> = autotag::Store::open(brain.dir())
        .route(text)
        .into_iter()
        .filter(|(s, _)| Space::new(s).is_ok_and(|s| readable.contains(&s)))
        .map(|(s, score)| json!({ "space": s, "score": score }))
        .collect();
    // The people of the note are a strong hint, ahead of its words: where earlier notes with them went.
    let people = crate::call::people_of(&tags_arg_lenient(args));
    let by_people: Vec<Value> = autotag::Store::open(brain.dir())
        .route_people(&people)
        .into_iter()
        .filter(|(s, _)| Space::new(s).is_ok_and(|s| readable.contains(&s)))
        .map(|(s, score)| json!({ "space": s, "score": score, "by": "people" }))
        .collect();
    out["route"] = json!(if by_people.is_empty() {
        route
    } else {
        by_people
    });
    Ok(out)
}

pub(super) fn tags_set(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let id = str_arg(args, "id")?;
    let Some((space, conn, old)) = locate(brain, &caller.writable(), id)? else {
        return Err(ToolError::Invalid("unknown fact".into()));
    };
    if caller.paths.contains_key(&space) || !caller.may_write_tags(&space, &old.tags) {
        return Err(ToolError::Invalid("unknown fact".into()));
    }
    if old.typ.as_deref() == Some("file") {
        return Err(ToolError::Invalid(
            "this note describes a kept file: its tags are set with the file".into(),
        ));
    }
    let list = |key: &str| -> Result<Option<Vec<String>>, ToolError> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => Ok(Some(tags_arg_of(args, key)?)),
        }
    };
    let mut tags = list("tags")?.unwrap_or_else(|| old.tags.clone());
    let add = list("add")?.unwrap_or_default();
    for t in &add {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }
    let remove = list("remove")?.unwrap_or_default();
    tags.retain(|t| !remove.contains(t));
    let learn = args.get("learn").and_then(Value::as_bool).unwrap_or(true);
    // A sign the person takes off a note is their word: the mark `signe-refuse/<sign>` keeps the tagger of the node and
    // the janitor's night sweep from putting it back on that note. Adding the sign again takes the mark off. What an
    // engine does (a pass that says `learn: false`) leaves no mark and clears none.
    if learn {
        // Every sign the note had and no longer has, whether it went through `remove` or through a whole `tags` list.
        for t in &old.tags {
            if !tags.contains(t)
                && let Some(mark) = pensine_health::refused_sign(t)
                && !tags.contains(&mark)
            {
                tags.push(mark);
            }
        }
        for t in &add {
            if let Some(mark) = pensine_health::refused_sign(t) {
                tags.retain(|x| *x != mark);
            }
        }
    }
    // Fields: a value sets it (the old value of that field goes), null or an empty text takes it off.
    if let Some(fields) = args.get("fields") {
        let fields = fields
            .as_object()
            .ok_or_else(|| ToolError::Invalid("fields: an object {name: value}".into()))?;
        for (name, value) in fields {
            let prefix = format!("{}{}/", autotag::FIELD_PREFIX, field_name(name));
            tags.retain(|t| !t.starts_with(&prefix));
            // The participants of a call are a list: the whole list is set (an empty one takes them all off).
            if let (Value::Array(list), true) = (
                value,
                crate::call::LIST_FIELDS.contains(&field_name(name).as_str()),
            ) {
                for item in list {
                    let raw = item.as_str().ok_or_else(|| {
                        ToolError::Invalid(format!("fields.{name}: a list of names or numbers"))
                    })?;
                    let p = crate::call::participant(raw)?;
                    tags.push(format!("{prefix}{}", p.slug()));
                }
                continue;
            }
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            if !text.trim().is_empty() {
                let tag = autotag::field_tag(name, &text).ok_or_else(|| {
                    ToolError::Invalid(format!("fields.{name}: not a value a tag can hold"))
                })?;
                tags.push(tag);
            }
        }
    }
    // The person tags follow the participants: a name that leaves both sides of a call leaves the note, a name that
    // comes in is a person of the note.
    sync_people(&old.tags, &mut tags);
    let tags = facts::normalize_tags(&tags)?;
    if tags.is_empty() {
        return Err(ToolError::Invalid(
            "a note keeps at least one tag; to take the note out, retire it".into(),
        ));
    }
    let mut before = old.tags.clone();
    before.sort();
    if tags == before {
        return Ok(json!({ "result": "unchanged", "fact": old.to_json() }));
    }
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .map_or_else(|| format!("tags set by {}", caller.agent), String::from);
    let mut new = NewFact::agent(&old.text, &format!("agent:{}", caller.agent));
    new.kind = Some(old.kind.clone());
    new.valid_from = Some(old.valid_from);
    new.valid_to = old.valid_to;
    new.tags = tags.clone();
    let f = facts::supersede(&conn, &space, &caller.agent, id, new, Some(&reason))?;
    // The same note with other tags: it keeps its date, its source and its origin.
    facts::carry_provenance(&conn, id, &f.id)?;
    let f = facts::read_fact(&conn, &f.id)?.unwrap_or(f);
    let learned =
        learn && learn_change(brain, &caller.agent, &space, &old.text, &old.tags, &f.tags);
    let added: Vec<&String> = f.tags.iter().filter(|t| !old.tags.contains(t)).collect();
    let removed: Vec<&String> = old.tags.iter().filter(|t| !f.tags.contains(t)).collect();
    Ok(json!({
        "result": "superseded",
        "fact": f.to_json(),
        "added": added,
        "removed": removed,
        "reason": reason,
        "learned": learned,
    }))
}

/// The `personnes/` tags of a note follow its `field/caller` and `field/callee` tags (names only, never numbers).
fn sync_people(old: &[String], tags: &mut Vec<String>) {
    let people = |list: &[String]| -> std::collections::BTreeSet<String> {
        list.iter()
            .filter_map(|t| {
                let rest = t.strip_prefix(autotag::FIELD_PREFIX)?;
                let value = rest
                    .strip_prefix("caller/")
                    .or_else(|| rest.strip_prefix("callee/"))?;
                (!value.chars().all(|c| c.is_ascii_digit())).then(|| value.to_string())
            })
            .collect()
    };
    let (before, after) = (people(old), people(tags));
    for gone in before.difference(&after) {
        let tag = format!("{}{gone}", crate::call::PERSON_PREFIX);
        tags.retain(|t| *t != tag);
    }
    for new in after.difference(&before) {
        let tag = format!("{}{new}", crate::call::PERSON_PREFIX);
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
}

fn field_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The `tags` of a preview: what is not a tag is left out, nothing fails.
fn tags_arg_lenient(args: &Value) -> Vec<String> {
    args.get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|t| facts::normalize_tag(t).ok())
        .collect()
}

fn tags_arg_of(args: &Value, key: &str) -> Result<Vec<String>, ToolError> {
    let list: Vec<String> = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::Invalid(format!("{key:?} must be a list of text")))?
        .iter()
        .map(|t| {
            t.as_str()
                .map(String::from)
                .ok_or_else(|| ToolError::Invalid(format!("{key:?} must be a list of text")))
        })
        .collect::<Result<_, _>>()?;
    Ok(facts::normalize_tags(&list)?)
}
