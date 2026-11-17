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
    out["route"] = json!(route);
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
    for t in list("add")?.unwrap_or_default() {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    let remove = list("remove")?.unwrap_or_default();
    tags.retain(|t| !remove.contains(t));
    // Fields: a value sets it (the old value of that field goes), null or an empty text takes it off.
    if let Some(fields) = args.get("fields") {
        let fields = fields
            .as_object()
            .ok_or_else(|| ToolError::Invalid("fields: an object {name: value}".into()))?;
        for (name, value) in fields {
            let prefix = format!("{}{}/", autotag::FIELD_PREFIX, field_name(name));
            tags.retain(|t| !t.starts_with(&prefix));
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
    new.kind = old.kind.clone();
    new.valid_from = Some(old.valid_from);
    new.valid_to = old.valid_to;
    new.tags = tags.clone();
    let f = facts::supersede(&conn, &space, &caller.agent, id, new, Some(&reason))?;
    let learn = args.get("learn").and_then(Value::as_bool).unwrap_or(true);
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

fn field_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
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
