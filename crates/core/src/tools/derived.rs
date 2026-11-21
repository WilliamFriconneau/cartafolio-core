use crate::access::Caller;
use crate::facts::{self, Fact, NewFact};
use crate::origin::{self, Layer, Origin};
use crate::{Brain, Space, sync, util};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::{Value, json};
use std::collections::HashSet;

use super::{ToolError, scope_tags, tags_arg, write_space};

const AUTHORITY: &str = "derived-authority";

pub(super) fn deposit(brain: &Brain, caller: &Caller, args: &Value) -> Result<Value, ToolError> {
    let space = write_space(brain, caller, args)?;
    let records = args["records"]
        .as_array()
        .filter(|rows| (1..=200).contains(&rows.len()))
        .ok_or_else(|| invalid("expected 1 to 200 derived records"))?;
    let window = sync::window_status(brain)?;
    if !brain.dir().join(AUTHORITY).is_file()
        || window.days.is_some()
        || window.last_prune.is_some()
    {
        return Ok(json!({"result": "authority_unavailable"}));
    }
    let conn = brain.open_space(&space)?;
    let results: Vec<_> = records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let mut result = match prepare(caller, &space, record) {
                Ok((origin, new, legacy_tag)) => {
                    write(&conn, &space, caller, &origin, new, &legacy_tag)
                        .unwrap_or_else(|_| json!({"result": "retry"}))
                }
                Err(_) => json!({"result": "invalid"}),
            };
            result["index"] = json!(index);
            result
        })
        .collect();
    Ok(json!({"results": results}))
}

fn invalid(message: &str) -> ToolError {
    ToolError::Invalid(message.into())
}

fn prepare(
    caller: &Caller,
    space: &Space,
    record: &Value,
) -> Result<(Origin, NewFact, String), ToolError> {
    let source = &record["origin"];
    let app = source["app"]
        .as_str()
        .ok_or_else(|| invalid("missing origin app"))?;
    let id = source["id"]
        .as_str()
        .ok_or_else(|| invalid("missing origin key"))?;
    if source["layer"] != "dossier" {
        return Err(invalid("derived records use the dossier layer"));
    }
    let origin = Origin::parse(app, id, source["at"].as_str())?.in_layer(Layer::Dossier);
    let text = record["text"]
        .as_str()
        .ok_or_else(|| invalid("missing record text"))?;
    let text = facts::check_text(text)?;
    facts::check_size(&text)?;
    if payload(&text).is_none() {
        return Err(invalid("expected a JSON object"));
    }
    let mut new = NewFact::agent(&text, &caller.agent);
    new.tags = tags_arg(record)?;
    let origin_tag = origin.scope_tag();
    scope_tags(caller, space, &mut new.tags, Some(&origin_tag))?;
    new.tags.push(origin_tag);
    new.tags = facts::normalize_tags(&new.tags)?;
    let legacy_tag = record["legacy_tag"]
        .as_str()
        .filter(|tag| !tag.is_empty() && new.tags.iter().any(|candidate| candidate == tag))
        .ok_or_else(|| invalid("legacy_tag must be one of the record’s tags"))?;
    Ok((origin, new, legacy_tag.to_owned()))
}

fn write(
    conn: &Connection,
    space: &Space,
    caller: &Caller,
    origin: &Origin,
    new: NewFact,
    legacy_tag: &str,
) -> crate::Result<Value> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let versions = origin.versions(&tx)?;
    let disputes = sync::dispute_marks(&tx)?;
    let mut head: Option<Fact> = None;
    for id in versions {
        let mut id = id;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(id.clone()) || disputes.contains_key(&id) {
                return Ok(json!({"result": "disputed"}));
            }
            let Some(fact) = facts::read_fact(&tx, &id)? else {
                return Ok(json!({"result": "identity_unverified"}));
            };
            if let Some(next) = fact.replaced_by {
                id = next;
                continue;
            }
            if head.as_ref().is_some_and(|known| known.id != fact.id) {
                return Ok(json!({"result": "disputed"}));
            }
            head = Some(fact);
            break;
        }
    }
    let Some((label, next)) = payload(&new.text) else {
        return Ok(json!({"result": "invalid"}));
    };
    // Older replicas did not replicate the origin index. A legacy match blocks creation,
    // and a competing active legacy row blocks updates. Neither may select a head.
    let other: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM facts f
          WHERE (?3 IS NULL OR (f.id <> ?3 AND f.status = 'active'))
            AND ((?2 <> '' AND substr(f.text, 1, length(?2)) = ?2)
                 OR EXISTS (SELECT 1 FROM fact_tags t WHERE t.fact_id = f.id AND t.tag = ?1)))",
        rusqlite::params![legacy_tag, label, head.as_ref().map(|fact| &fact.id)],
        |row| row.get(0),
    )?;
    if other {
        return Ok(json!({"result": "identity_unverified"}));
    }
    let (status, saved) = if let Some(head) = head {
        if head.status != "active" {
            return Ok(json!({"result": "retired"}));
        }
        if !caller.may_write_tags(space, &head.tags) {
            return Ok(json!({"result": "invalid"}));
        }
        let Some((_, current)) = payload(&head.text) else {
            return Ok(json!({"result": "invalid"}));
        };
        if same_values(&current, &next) {
            return Ok(json!({"result": "unchanged", "id": head.id}));
        }
        if facts::active_with_text(&tx, &new.text)?.is_some() {
            return Ok(json!({"result": "identity_unverified"}));
        }
        // Preserve tags added to the established fact.
        let mut new = new;
        new.tags.extend(head.tags);
        new.tags = facts::normalize_tags(&new.tags)?;
        let saved = facts::supersede_in(
            &tx,
            space,
            &caller.agent,
            &head.id,
            new,
            Some("derived value updated"),
            "supersede",
        )?;
        ("updated", saved)
    } else {
        if facts::active_with_text(&tx, &new.text)?.is_some() {
            return Ok(json!({"result": "identity_unverified"}));
        }
        (
            "created",
            facts::insert(&tx, space, &caller.agent, &new, &new.text, None)?,
        )
    };
    origin::record(&tx, &saved.id, origin, util::now())?;
    tx.commit()?;
    Ok(json!({"result": status, "id": saved.id}))
}

fn payload(text: &str) -> Option<(&str, Value)> {
    let start = text.find('{')?;
    let json: Value = serde_json::from_str(&text[start..]).ok()?;
    json.is_object().then_some((&text[..start], json))
}

fn same_values(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same_values(value, other)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_values(a, b))
        }
        (Value::Number(a), Value::Number(b)) => {
            a == b || (a.is_f64() != b.is_f64() && a.as_f64() == b.as_f64())
        }
        _ => a == b,
    }
}
