//! The space view contract (`memcp.view/1`): what a space offers to read, as data.
//!
//! A client discovers a space's **dashboard**, its **targets** (the living board `targets`) and its
//! **timestamped register** (the notes, newest first, with a cursor) through one tool, `space_view`.
//! Every view returns the same envelope: the contract name and version, the space, the view, the
//! items with their stable id, type, time and provenance, and the page. The tools that existed keep
//! answering as before; this module only reads.
//!
//! The contract is versioned by [`CONTRACT`]. A change that removes or renames a field is a new
//! contract; adding a field keeps the version. Clients of an older version keep reading the fields
//! they know.

use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};

use crate::access::Caller;
use crate::brain::Brain;
use crate::facts::{self, Fact};
use crate::space::Space;
use crate::tools::ToolError;
use crate::util;

/// The name and version of this contract, as a client reads it.
pub const CONTRACT: &str = "memcp.view/1";

/// The whole number of [`CONTRACT`], for a client that compares numbers.
pub const CONTRACT_VERSION: u32 = 1;

const DAY: i64 = 86_400;
const WINDOW_DAYS: i64 = 30;
const MAX_SECTIONS: usize = 20;

/// The notes of the person, without the page's own furniture (rules, dashboard, living boards).
const FURNITURE: &str = "id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules','index','guide') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')";

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Invalid(format!("{key} is required")))
}

fn opt(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// `space_view {space, view?, from?, to?, type?, tag?, limit?, cursor?}`.
pub fn space_view(brain: &Brain, caller: &Caller, args: &Value) -> Result<Value, ToolError> {
    let space_name = arg(args, "space")?;
    let space = Space::new(space_name)?;
    if !caller.readable().contains(&space) || caller.read_paths(&space).is_some() {
        return Err(ToolError::Denied(format!("no access to {space_name}")));
    }
    let kind = brain.space_kind(&space).unwrap_or_else(|_| "custom".into());
    let label = brain
        .layout(&caller.user_id)
        .ok()
        .and_then(|places| places.into_iter().find(|p| p.space == space))
        .map(|p| p.label);
    let view = opt(args, "view").unwrap_or_else(|| "capabilities".into());
    let now = util::now();
    match view.as_str() {
        "capabilities" => {
            let mut v = capabilities(caller, &space, &kind, label, now);
            v["profession"] =
                crate::profession::json_for(Some(brain.dir()), &kind, space.as_str(), None);
            Ok(v)
        }
        "dashboard" => {
            let conn = brain.open_space(&space)?;
            let dashboard = dashboard(&conn, &kind, now);
            Ok(json!({
                "contract": CONTRACT,
                "contract_version": CONTRACT_VERSION,
                "space": space.as_str(),
                "kind": kind,
                "label": label,
                "view": "dashboard",
                "generated_at": util::iso(now),
                "dashboard": dashboard,
                "provenance": { "kind": "memory", "space": space.as_str(), "source": "dashboard" },
                "page": Value::Null,
                "notice": crate::tools::NOTICE,
            }))
        }
        "targets" => targets(brain, caller, &space, &kind, label, now),
        "chronology" => {
            let conn = brain.open_space(&space)?;
            register(&conn, &space, &kind, label, args, now, "chronology")
        }
        "register" => {
            let conn = brain.open_space(&space)?;
            register(&conn, &space, &kind, label, args, now, "register")
        }
        "stats" => {
            let conn = brain.open_space(&space)?;
            stats_view(&conn, &space, &kind, label, args, now)
        }
        "files" => {
            let conn = brain.open_space(&space)?;
            files_view(&conn, &space, &kind, label, args, now)
        }
        "tasks" => tasks(brain, caller, &space, &kind, label, now),
        other => Err(ToolError::Invalid(format!(
            "view: capabilities, targets, chronology, stats, files, tasks (or the legacy dashboard/register) (got {other:?})"
        ))),
    }
}

fn capabilities(
    caller: &Caller,
    space: &Space,
    kind: &str,
    label: Option<String>,
    now: i64,
) -> Value {
    let sensitive = kind == "health" || kind == "money";
    let writable = caller.can_write(space);
    let has_tasks = kind == "home" || kind == "hobbies";
    let mut item_types = serde_json::Map::new();
    item_types.insert(
        "note".into(),
        json!({ "id": "the fact id", "description": "A note written by the person or an assistant." }),
    );
    item_types.insert(
        "file".into(),
        json!({ "id": "the fact id", "description": "The note that describes a stored file; `file` gives the file's own id, name, type and size." }),
    );
    item_types.insert(
        "target".into(),
        json!({ "id": "the fact id", "fields": ["column", "signs", "metrics"], "description": "A card of the living board `targets`; `metrics` carries a deadline, a target amount, a realized amount and the progress." }),
    );
    item_types.insert(
        "dashboard".into(),
        json!({ "id": "the fact id", "description": "The document a space draws as its dashboard (legacy view)." }),
    );
    if has_tasks {
        item_types.insert(
            "task".into(),
            json!({ "id": "the fact id", "fields": ["column", "done"], "description": "A card of the living board `next`; `done` is true for the `done` column." }),
        );
    }
    if kind == "health" {
        item_types.insert(
            "sign".into(),
            json!({ "id": "the fact id", "fields": ["sign", "level", "label"], "description": "A note whose signs and symptoms are from the health referential." }),
        );
        item_types.insert(
            "measure".into(),
            json!({ "id": "the fact id", "fields": ["measure", "value", "unit", "label", "measures"], "description": "A note that states measures from the health referential; `measure`, `value`, `unit` and `label` are the first, `measures` lists all of them." }),
        );
    }
    if kind == "money" {
        item_types.insert(
            "movement".into(),
            json!({ "id": "mv_… (stable)", "fields": ["date", "amount", "account"], "description": "A bank movement of the money projection." }),
        );
    }
    let mut chronology_types = vec!["note", "file", "sign", "measure"];
    if kind == "money" {
        chronology_types.push("movement");
    }
    let mut views = vec![
        json!({
            "id": "targets",
            "title": "Cibles",
            "home": true,
            "description": "The space's home page: its targets, their figures (a deadline, a target amount, a realized amount) and their progress.",
            "tool": "space_view",
            "arguments": { "space": space.as_str(), "view": "targets" },
            "returns": ["target"],
            "filters": [],
            "pagination": Value::Null,
            "schema": { "arguments": { "space": "string" }, "returns": "target[]" },
        }),
        json!({
            "id": "chronology",
            "title": "Journal",
            "description": "One chronology for the space: notes, files, and structured entries (signs, measures, movements), newest first, with a temporal window, a type, a tag, words and a cursor. Provenance and time are kept on every entry.",
            "tool": "space_view",
            "arguments": { "space": space.as_str(), "view": "chronology" },
            "returns": chronology_types,
            "filters": ["from", "to", "type", "tag", "q"],
            "pagination": {
                "kind": "cursor",
                "limit": { "min": 1, "max": 200, "default": 50 },
                "cursor": "opaque: take `page.next_cursor` from the answer and pass it back as `cursor`",
            },
            "schema": {
                "arguments": { "space": "string", "from": "date?", "to": "date?", "type": "string?", "tag": "string?", "q": "string?", "limit": "integer?", "cursor": "string?" },
                "returns": "item[]",
            },
        }),
        json!({
            "id": "stats",
            "title": "Stats",
            "description": "The space's indicators over a window: totals by type, tags, days, apps and sources, targets and tasks.",
            "tool": "space_view",
            "arguments": { "space": space.as_str(), "view": "stats" },
            "returns": ["stats"],
            "filters": ["from", "to"],
            "pagination": Value::Null,
            "schema": {
                "arguments": { "space": "string", "from": "date?", "to": "date?" },
                "returns": { "total": "integer", "by_type": "object", "by_day": "array", "by_tag": "array", "sources": "array", "targets": "object", "tasks": "object", "first_at": "datetime|null", "last_at": "datetime|null" },
            },
        }),
        json!({
            "id": "files",
            "title": "Inventaire",
            "description": "The files still kept in the space, newest first: name, type, size, the note that describes them, their provenance and a cursor. The file bytes are reached with `file_get` when the rights allow it.",
            "tool": "space_view",
            "arguments": { "space": space.as_str(), "view": "files" },
            "returns": ["file"],
            "filters": ["from", "to", "q", "mime"],
            "pagination": {
                "kind": "cursor",
                "limit": { "min": 1, "max": 200, "default": 50 },
                "cursor": "opaque: take `page.next_cursor` from the answer and pass it back as `cursor`",
            },
            "schema": {
                "arguments": { "space": "string", "from": "date?", "to": "date?", "q": "string?", "mime": "string?", "limit": "integer?", "cursor": "string?" },
                "returns": "file[]",
            },
        }),
    ];
    if has_tasks {
        views.push(json!({
            "id": "tasks",
            "title": "Tâches",
            "description": "The checkable tasks of the space: the cards of the living board `next`, with `done`. Check one with `living_set` (move it to `done`).",
            "tool": "space_view",
            "arguments": { "space": space.as_str(), "view": "tasks" },
            "returns": ["task"],
            "filters": [],
            "pagination": Value::Null,
            "schema": { "arguments": { "space": "string" }, "returns": "task[]" },
        }));
    }
    let schemas = json!({
        "envelope": {
            "contract": "string", "contract_version": "integer", "space": "string", "kind": "string",
            "label": "string|null", "view": "string", "generated_at": "datetime", "notice": "string",
        },
        "pagination": { "limit": "integer", "has_more": "boolean", "next_cursor": "string|null" },
        "provenance": {
            "kind": "memory|app|statement|files", "app": "string|null", "layer": "sources|dossier|null",
            "author": "string|null", "source": "object",
        },
        "target_metrics": {
            "due": "date|null", "target_amount": "number|null", "realized_amount": "number|null",
            "progress_percent": "number|null", "currency": "string|null",
        },
        "items": {
            "note": { "id": "string", "type": "note", "at": "datetime", "created_at": "integer", "text": "string", "tags": "string[]", "fields": "object", "provenance": "provenance" },
            "file": { "id": "string", "type": "file", "at": "datetime", "created_at": "integer", "name": "string", "mime": "string", "size": "integer", "fact_id": "string|null", "provenance": "provenance" },
            "target": { "id": "string", "type": "target", "column": "string", "text": "string", "signs": "string[]", "metrics": "target_metrics", "provenance": "provenance" },
            "task": { "id": "string", "type": "task", "column": "string", "done": "boolean", "text": "string", "provenance": "provenance" },
            "sign": { "id": "string", "type": "sign", "sign": "string|null", "level": "string|null", "label": "string|null", "provenance": "provenance" },
            "measure": { "id": "string", "type": "measure", "measure": "string|null", "value": "string|number|null", "unit": "string|null", "label": "string|null", "provenance": "provenance" },
            "movement": { "id": "string", "type": "movement", "date": "date", "amount": "number", "account": "string", "provenance": "provenance" },
        },
        "actions": {
            "write_note": { "tool": "remember", "arguments": { "space": "string", "text": "string", "tags": "string[]?" } },
            "add_file": { "tool": "file_put", "arguments": { "space": "string", "name": "string", "note": "string?", "tags": "string[]?", "data_base64": "string" } },
            "move_target": { "tool": "living_set", "arguments": { "space": "string", "page": "targets", "move": [{ "id": "string", "column": "string" }] } },
            "check_task": { "tool": "living_set", "arguments": { "space": "string", "page": "next", "move": [{ "id": "string", "column": "done" }] } },
            "add_target": { "tool": "living_set", "arguments": { "space": "string", "page": "targets", "add": [{ "text": "string", "column": "open", "signs": "string[]?", "fields": "object?" }] } },
        }
    });
    json!({
        "contract": CONTRACT,
        "contract_version": CONTRACT_VERSION,
        "space": space.as_str(),
        "kind": kind,
        "label": label,
        "writable": writable,
        "sensitive": sensitive,
        "home": "targets",
        "generated_at": util::iso(now),
        "source": { "kind": "memory", "space": space.as_str() },
        "views": views,
        "item_types": item_types,
        "schemas": schemas,
        "tools": crate::tools::available_names(caller),
        "provenance": {
            "where": "each item carries `source` and `provenance`",
            "source": "who or what wrote it: an agent, a connection, an imported note or an app",
            "app": "the app that deposited it, from `sources/<app>` or `dossier/<app>`",
            "stable_id": "the fact id; a view item never invents an id",
        },
        "notice": crate::tools::NOTICE,
    })
}

/// The targets board, as contract items./// The targets board, as contract items. The board itself is read by `living_get`, so an assistant
/// and a client see the same cards and the same signs.
fn targets(
    brain: &Brain,
    caller: &Caller,
    space: &Space,
    kind: &str,
    label: Option<String>,
    now: i64,
) -> Result<Value, ToolError> {
    let board = crate::tools::living::living(
        brain,
        caller,
        "living_get",
        &json!({ "space": space.as_str(), "page": "targets" }),
    )?;
    let columns: Vec<String> = board["columns"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_else(|| vec!["open".into(), "reached".into()]);
    let mut items = vec![];
    for column in &columns {
        for card in board["board"][column].as_array().into_iter().flatten() {
            let id = card["card_id"]
                .as_str()
                .or_else(|| card["id"].as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            items.push(json!({
                "id": id,
                "fact_id": card["fact_id"],
                "type": "target",
                "space": space.as_str(),
                "column": column,
                "text": card["text"],
                "extract": extract(card["text"].as_str().unwrap_or(""), 160),
                "at": card["created"],
                "created_at": card["created_at"],
                "author": card["author"],
                "signs": card["signs"],
                "metrics": target_metrics(&card["fields"]),
                "fields": card["fields"],
                "tags": card["tags"],
                "provenance": {
                    "kind": "memory",
                    "author": card["author"],
                    "source": { "kind": "agent", "agent": card["author"] },
                },
            }));
        }
    }
    Ok(json!({
        "contract": CONTRACT,
        "contract_version": CONTRACT_VERSION,
        "space": space.as_str(),
        "kind": kind,
        "label": label,
        "view": "targets",
        "generated_at": util::iso(now),
        "columns": columns,
        "last_pass": board["last_pass"],
        "items": items,
        "provenance": { "kind": "memory", "space": space.as_str(), "source": "living/targets" },
        "page": { "limit": items.len(), "has_more": false, "next_cursor": Value::Null },
        "notice": crate::tools::NOTICE,
    }))
}

fn number_of(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| {
        v.as_str().and_then(|s| {
            s.trim()
                .replace(',', ".")
                .replace(' ', "")
                .parse::<f64>()
                .ok()
        })
    })
}

/// The figures of a target: a deadline, a target amount, a realized amount and the progress they
/// give, when the space (money, most often) put them in the card's fields. Anything absent is
/// `null`; the contract never invents a figure.
fn target_metrics(fields: &Value) -> Value {
    let get = |names: &[&str]| -> Option<Value> {
        names
            .iter()
            .find_map(|n| fields.get(*n).filter(|v| !v.is_null()).cloned())
    };
    let due = get(&["due", "echeance", "échéance", "deadline"]);
    let target = get(&["target_amount", "montant_cible", "target"]).and_then(|v| number_of(&v));
    let realized = get(&[
        "realized_amount",
        "montant_realise",
        "montant_réalisé",
        "realized",
    ])
    .and_then(|v| number_of(&v));
    let progress = match (target, realized) {
        (Some(t), Some(r)) if t > 0.0 => Some((r / t * 1000.0).round() / 10.0),
        _ => None,
    };
    // ISO 4217 codes are upper case; the fields travel as tags, which are lower-cased.
    let currency = get(&["currency", "devise"]).map(|v| match v {
        Value::String(s) => Value::String(s.to_uppercase()),
        other => other,
    });
    json!({
        "due": due,
        "target_amount": target,
        "realized_amount": realized,
        "progress_percent": progress,
        "currency": currency,
    })
}

/// The timestamped register: notes, files, targets or the dashboard, newest first.
fn register(
    conn: &Connection,
    space: &Space,
    kind: &str,
    label: Option<String>,
    args: &Value,
    now: i64,
    view_name: &str,
) -> Result<Value, ToolError> {
    let default_type = if view_name == "chronology" {
        "all"
    } else {
        "note"
    };
    let typ = opt(args, "type").unwrap_or_else(|| default_type.to_string());
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50)
        .clamp(1, 200);
    // The money projection lives in the server's finance ledger (the `finances_*` tools). A node
    // without it answers an empty page here, never an error: the field stays readable. Its cursor is
    // the ledger's (a movement id), not the fact cursor, so it is not parsed here.
    if typ == "movement" {
        return Ok(json!({
            "contract": CONTRACT,
            "contract_version": CONTRACT_VERSION,
            "space": space.as_str(),
            "kind": kind,
            "label": label,
            "view": view_name,
            "generated_at": util::iso(now),
            "items": [],
            "page": { "limit": limit, "has_more": false, "next_cursor": Value::Null },
            "note": "movements come from the money projection: `finances_movements`, or this view on a node holding the finance ledger",
            "notice": crate::tools::NOTICE,
        }));
    }
    let from = bound(args, "from", false, 0)?;
    let to = bound(args, "to", true, now)?;
    let cursor = opt(args, "cursor")
        .map(|c| {
            c.split_once(':')
                .and_then(|(at, id)| at.parse::<i64>().ok().map(|at| (at, id.to_string())))
                .ok_or_else(|| {
                    ToolError::Invalid("cursor: take `page.next_cursor` from the answer".into())
                })
        })
        .transpose()?;
    let tag = opt(args, "tag");
    let mut sql =
        "SELECT id FROM facts WHERE status = 'active' AND created_at >= ? AND created_at <= ?"
            .to_string();
    let mut vals: Vec<SqlValue> = vec![from.into(), to.into()];
    match typ.as_str() {
        "note" | "all" => sql.push_str(&format!(" AND {FURNITURE}")),
        "file" => sql.push_str(" AND type = 'file'"),
        "target" => sql.push_str(
            " AND id IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'living/targets/%')",
        ),
        "dashboard" => {
            sql.push_str(" AND id IN (SELECT fact_id FROM fact_tags WHERE tag = 'dashboard')")
        }
        "sign" => sql.push_str(&format!(
            " AND {FURNITURE} AND id IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'signe/%' OR tag LIKE 'sign/%' OR tag LIKE 'field/level/%')"
        )),
        "measure" => sql.push_str(&format!(
            " AND {FURNITURE} AND id IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'mesure/%')"
        )),
        other => {
            return Err(ToolError::Invalid(format!(
                "type: all, note, file, target, dashboard, sign, measure or movement (got {other:?})"
            )));
        }
    }
    let query = opt(args, "q");
    if let Some(q) = &query {
        sql.push_str(" AND text LIKE ? COLLATE NOCASE");
        vals.push(format!("%{q}%").into());
    }
    if let Some(tag) = &tag {
        sql.push_str(" AND id IN (SELECT fact_id FROM fact_tags WHERE tag = ? OR tag LIKE ?)");
        vals.push(tag.clone().into());
        vals.push(format!("{tag}/%").into());
    }
    if let Some((at, id)) = &cursor {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
        vals.push((*at).into());
        vals.push((*at).into());
        vals.push(id.clone().into());
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    vals.push((limit + 1).into());
    let mut st = conn
        .prepare(&sql)
        .map_err(|e| ToolError::Invalid(e.to_string()))?;
    let ids: Vec<String> = st
        .query_map(rusqlite::params_from_iter(vals.iter()), |r| r.get(0))
        .map_err(|e| ToolError::Invalid(e.to_string()))?
        .flatten()
        .collect();
    let more = ids.len() > limit as usize;
    let mut items = vec![];
    for id in ids.into_iter().take(limit as usize) {
        if let Some(fact) = facts::read_fact(conn, &id)? {
            items.push(item(conn, &fact, kind));
        }
    }
    let next = more.then(|| {
        items
            .last()
            .and_then(|i| {
                let at = i["created_at"].as_i64()?;
                let id = i["id"].as_str()?;
                Some(format!("{at}:{id}"))
            })
            .unwrap_or_default()
    });
    Ok(json!({
        "contract": CONTRACT,
        "contract_version": CONTRACT_VERSION,
        "space": space.as_str(),
        "kind": kind,
        "label": label,
        "view": view_name,
        "generated_at": util::iso(now),
        "filters": { "from": from, "to": to, "type": typ, "tag": tag, "q": query },
        "items": items,
        "provenance": { "kind": "memory", "space": space.as_str(), "source": "facts" },
        "page": { "limit": limit, "has_more": more, "next_cursor": next },
        "notice": crate::tools::NOTICE,
    }))
}

/// A temporal bound: a date (`YYYY-MM-DD`, the whole day when it is `to`), an ISO time or seconds.
fn bound(args: &Value, key: &str, end_of_day: bool, default: i64) -> Result<i64, ToolError> {
    let Some(v) = args.get(key) else {
        return Ok(default);
    };
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(default);
    };
    if s.contains('T') || s.chars().all(|c| c.is_ascii_digit()) {
        return crate::tools::parse_time(s)
            .ok_or_else(|| ToolError::Invalid(format!("{key}: YYYY-MM-DD or ISO 8601 expected")));
    }
    let day = crate::tools::parse_date(&Value::String(s.to_string()))?;
    Ok(day + if end_of_day { DAY - 1 } else { 0 })
}

/// One register item: the fact's own fields, its contract type, its time and its provenance.
fn item(conn: &Connection, fact: &Fact, space_kind: &str) -> Value {
    let mut v = fact.to_json();
    let fact_type = v.get("type").cloned().unwrap_or(Value::Null);
    let (typ, facets) = classify(fact, space_kind);
    v["fact_type"] = fact_type;
    v["type"] = json!(typ);
    v["at"] = json!(util::iso(fact.created_at));
    v["extract"] = json!(extract(&fact.text, 160));
    v["provenance"] = provenance(fact);
    if let Some(obj) = facets.as_object() {
        for (k, val) in obj {
            v[k] = val.clone();
        }
    }
    if fact.typ.as_deref() == Some("file")
        && let Ok(file) = conn.query_row(
            "SELECT id, name, mime, size FROM files WHERE fact_id = ?1 AND status = 'active' LIMIT 1",
            [&fact.id],
            |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "name": r.get::<_, String>(1)?,
                    "mime": r.get::<_, String>(2)?,
                    "size": r.get::<_, i64>(3)?,
                }))
            },
        )
    {
        v["file"] = file;
    }
    v
}

fn classify(fact: &Fact, space_kind: &str) -> (String, Value) {
    let fields = crate::autotag::fields_json(&fact.tags);
    if space_kind == "health" {
        // Every measure of the note, in the order of its tags: a day of readings (blood pressure, pulse, saturation) is one
        // note with several. The first stays in the item's own fields, for the clients that read one.
        let measures: Vec<Value> = fact
            .tags
            .iter()
            .filter_map(|t| t.strip_prefix("mesure/"))
            .map(|id| {
                json!({
                    "measure": id,
                    "value": fields.get(id).cloned().unwrap_or(Value::Null),
                    "unit": fields.get(format!("{id}_unit")).cloned().unwrap_or(Value::Null),
                    "label": measure_label(id),
                })
            })
            .collect();
        if let Some(first) = measures.first() {
            let mut item = first.clone();
            item["measures"] = json!(measures);
            return ("measure".into(), item);
        }
        let sign = fact
            .tags
            .iter()
            .find_map(|t| t.strip_prefix("signe/").or_else(|| t.strip_prefix("sign/")));
        let level = fields.get("level").cloned().unwrap_or(Value::Null);
        if sign.is_some() || !level.is_null() {
            let id = sign.map(String::from);
            return (
                "sign".into(),
                json!({
                    "sign": id,
                    "level": level,
                    "label": id.as_deref().and_then(sign_label),
                }),
            );
        }
    }
    if fact.typ.as_deref() == Some("file") {
        ("file".into(), Value::Null)
    } else {
        ("note".into(), Value::Null)
    }
}

fn sign_label(id: &str) -> Option<String> {
    let folded = memcp_health::text::fold(id);
    memcp_health::builtin()
        .manifestations()
        .iter()
        .find(|m| {
            memcp_health::text::fold(&m.id) == folded || memcp_health::text::fold(&m.tag) == folded
        })
        .and_then(|m| m.labels.get("fr").or_else(|| m.labels.get("en")).cloned())
}

/// The label of a measure, by its id or by its tag (`heart_rate` and `pouls`); a measure the referential does not know
/// is named by its key, readable (`hrv_sdnn_ms` gives `Hrv sdnn ms`).
fn measure_label(id: &str) -> Option<String> {
    let known = memcp_health::builtin()
        .measure_defs()
        .iter()
        .find(|m| m.id == id || m.tag == id)
        .and_then(|m| m.labels.get("fr").or_else(|| m.labels.get("en")).cloned());
    known.or_else(|| {
        let words = id.replace(['_', '-'], " ");
        let mut c = words.trim().chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
    })
}

fn provenance(fact: &Fact) -> Value {
    let layer = fact.tags.iter().find_map(|t| {
        if let Some(rest) = t.strip_prefix("sources/") {
            Some(("sources", rest.split('/').next().unwrap_or("")))
        } else if let Some(rest) = t.strip_prefix("dossier/") {
            Some(("dossier", rest.split('/').next().unwrap_or("")))
        } else {
            None
        }
    });
    let (layer, app) = match layer {
        Some((layer, app)) if !app.is_empty() => (json!(layer), json!(app)),
        _ => (Value::Null, Value::Null),
    };
    json!({
        "kind": if app.is_null() { "memory" } else { "app" },
        "app": app,
        "layer": layer,
        "author": fact.author,
        "source": {
            "kind": fact.source_kind,
            "agent": fact.source_agent,
            "path": fact.source_path,
            "ref": fact.source_ref,
        },
    })
}

/// The indicators of a space: totals, types, tags, days, sources, targets and tasks. No model.
fn stats_view(
    conn: &Connection,
    space: &Space,
    kind: &str,
    label: Option<String>,
    args: &Value,
    now: i64,
) -> Result<Value, ToolError> {
    let from = bound(args, "from", false, 0)?;
    let to = bound(args, "to", true, now)?;
    let base = format!(
        "status = 'active' AND created_at >= {from} AND created_at <= {to} AND {FURNITURE}"
    );
    let count = |extra: &str| -> i64 {
        conn.query_row(
            &format!("SELECT count(*) FROM facts WHERE {base} {extra}"),
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    let total = count("");
    let files = count("AND type = 'file'");
    let signs = count(
        "AND id IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'signe/%' OR tag LIKE 'sign/%' OR tag LIKE 'field/level/%')",
    );
    let measures = count("AND id IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'mesure/%')");
    let notes = total
        .saturating_sub(files)
        .saturating_sub(signs)
        .saturating_sub(measures);
    let by_type = json!({ "note": notes, "file": files, "sign": signs, "measure": measures });
    let mut by_day = vec![];
    if let Ok(mut st) = conn.prepare(&format!(
        "SELECT date(created_at, 'unixepoch'), count(*) FROM facts WHERE {base} GROUP BY 1 ORDER BY 1"
    )) && let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
    {
        for (day, n) in rows.flatten() {
            by_day.push(json!({ "day": day, "count": n }));
        }
    }
    let mut by_tag = vec![];
    if let Ok(mut st) = conn.prepare(
        "SELECT t.tag, count(DISTINCT t.fact_id) FROM fact_tags t JOIN facts f ON f.id = t.fact_id
         WHERE f.status = 'active' AND f.created_at >= ?1 AND f.created_at <= ?2
         AND f.id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules','index','guide') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
         AND substr(t.tag, 1, 1) <> '@'
         GROUP BY t.tag ORDER BY 2 DESC, 1 ASC LIMIT 20"
    ) && let Ok(rows) = st.query_map([from, to], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
    {
        for (tag, n) in rows.flatten() {
            by_tag.push(json!({ "tag": tag, "count": n }));
        }
    }
    let mut sources: std::collections::BTreeMap<(String, String), i64> = Default::default();
    if let Ok(mut st) = conn.prepare(&format!(
        "SELECT tag, count(*) FROM fact_tags WHERE fact_id IN (SELECT id FROM facts WHERE {base})
         AND (tag LIKE 'sources/%' OR tag LIKE 'dossier/%') GROUP BY tag"
    )) && let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
    {
        for (tag, n) in rows.flatten() {
            let (layer, app) = if let Some(rest) = tag.strip_prefix("sources/") {
                ("sources", rest.split('/').next().unwrap_or(""))
            } else if let Some(rest) = tag.strip_prefix("dossier/") {
                ("dossier", rest.split('/').next().unwrap_or(""))
            } else {
                continue;
            };
            if !app.is_empty() {
                *sources
                    .entry((layer.to_string(), app.to_string()))
                    .or_insert(0) += n;
            }
        }
    }
    let sources: Vec<Value> = sources
        .into_iter()
        .map(|((layer, app), count)| json!({ "layer": layer, "app": app, "count": count }))
        .collect();
    let count_tag = |tag: &str| -> i64 {
        conn.query_row(
            "SELECT count(DISTINCT fact_id) FROM fact_tags WHERE tag = ?1",
            [tag],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    let targets = json!({
        "open": count_tag("living/targets/open"),
        "reached": count_tag("living/targets/reached"),
    });
    let tasks = json!({
        "open": count_tag("living/next/now") + count_tag("living/next/next") + count_tag("living/next/later"),
        "done": count_tag("living/next/done"),
    });
    let (first_at, last_at): (Option<i64>, Option<i64>) = conn
        .query_row(
            &format!("SELECT min(created_at), max(created_at) FROM facts WHERE {base}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((None, None));
    let stats = json!({
        "total": total,
        "by_type": by_type,
        "by_day": by_day,
        "by_tag": by_tag,
        "sources": sources,
        "targets": targets,
        "tasks": tasks,
        "first_at": first_at.map(util::iso),
        "last_at": last_at.map(util::iso),
    });
    Ok(json!({
        "contract": CONTRACT,
        "contract_version": CONTRACT_VERSION,
        "space": space.as_str(),
        "kind": kind,
        "label": label,
        "view": "stats",
        "generated_at": util::iso(now),
        "filters": { "from": from, "to": to },
        "stats": stats,
        "items": [],
        "page": Value::Null,
        "provenance": { "kind": "memory", "space": space.as_str(), "source": "facts" },
        "notice": crate::tools::NOTICE,
    }))
}

/// The inventory of a space's files, newest first, with a cursor. Only files still active.
fn files_view(
    conn: &Connection,
    space: &Space,
    kind: &str,
    label: Option<String>,
    args: &Value,
    now: i64,
) -> Result<Value, ToolError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50)
        .clamp(1, 200);
    let from = bound(args, "from", false, 0)?;
    let to = bound(args, "to", true, now)?;
    let cursor = opt(args, "cursor")
        .map(|c| {
            c.split_once(':')
                .and_then(|(at, id)| at.parse::<i64>().ok().map(|at| (at, id.to_string())))
                .ok_or_else(|| {
                    ToolError::Invalid("cursor: take `page.next_cursor` from the answer".into())
                })
        })
        .transpose()?;
    let q = opt(args, "q");
    let mime = opt(args, "mime");
    let mut sql = "SELECT id, name, mime, size, note, fact_id, author, created_at FROM files
                   WHERE status = 'active' AND created_at >= ? AND created_at <= ?"
        .to_string();
    let mut vals: Vec<SqlValue> = vec![from.into(), to.into()];
    if let Some(q) = &q {
        sql.push_str(" AND (name LIKE ? COLLATE NOCASE OR note LIKE ? COLLATE NOCASE)");
        vals.push(format!("%{q}%").into());
        vals.push(format!("%{q}%").into());
    }
    if let Some(mime) = &mime {
        sql.push_str(" AND mime LIKE ?");
        vals.push(format!("{mime}%").into());
    }
    if let Some((at, id)) = &cursor {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
        vals.push((*at).into());
        vals.push((*at).into());
        vals.push(id.clone().into());
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    vals.push((limit + 1).into());
    let mut st = conn
        .prepare(&sql)
        .map_err(|e| ToolError::Invalid(e.to_string()))?;
    let rows = st
        .query_map(rusqlite::params_from_iter(vals.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })
        .map_err(|e| ToolError::Invalid(e.to_string()))?;
    type FileRow = (
        String,
        String,
        String,
        i64,
        String,
        Option<String>,
        String,
        i64,
    );
    let mut all: Vec<FileRow> = rows.flatten().collect();
    let more = all.len() > limit as usize;
    all.truncate(limit as usize);
    let mut items = vec![];
    for (id, name, mime, size, note, fact_id, author, created_at) in all {
        let fact = fact_id
            .as_deref()
            .and_then(|fid| facts::read_fact(conn, fid).ok().flatten());
        let (tags, fields, provenance) = fact
            .as_ref()
            .map(|f| {
                (
                    json!(f.tags),
                    crate::autotag::fields_json(&f.tags),
                    provenance(f),
                )
            })
            .unwrap_or_else(|| {
                (
                    json!([]),
                    json!({}),
                    json!({ "kind": "file", "author": author, "source": { "kind": "file" } }),
                )
            });
        items.push(json!({
            "id": id,
            "type": "file",
            "space": space.as_str(),
            "name": name,
            "mime": mime,
            "size": size,
            "note": note,
            "text": if note.is_empty() { name.clone() } else { note.clone() },
            "extract": extract(if note.is_empty() { &name } else { &note }, 160),
            "fact_id": fact_id,
            "author": author,
            "at": util::iso(created_at),
            "created_at": created_at,
            "tags": tags,
            "fields": fields,
            "provenance": provenance,
        }));
    }
    let next = more.then(|| {
        items
            .last()
            .and_then(|i| {
                let at = i["created_at"].as_i64()?;
                let id = i["id"].as_str()?;
                Some(format!("{at}:{id}"))
            })
            .unwrap_or_default()
    });
    Ok(json!({
        "contract": CONTRACT,
        "contract_version": CONTRACT_VERSION,
        "space": space.as_str(),
        "kind": kind,
        "label": label,
        "view": "files",
        "generated_at": util::iso(now),
        "filters": { "from": from, "to": to, "q": q, "mime": mime },
        "items": items,
        "page": { "limit": limit, "has_more": more, "next_cursor": next },
        "provenance": { "kind": "files", "space": space.as_str(), "source": "files" },
        "notice": crate::tools::NOTICE,
    }))
}

/// The tasks of a home or hobbies space: the cards of the living board `next`, with `done` true for
/// the column `done`. A client checks one with `living_set` (move it to `done`).
fn tasks(
    brain: &Brain,
    caller: &Caller,
    space: &Space,
    kind: &str,
    label: Option<String>,
    now: i64,
) -> Result<Value, ToolError> {
    let board = crate::tools::living::living(
        brain,
        caller,
        "living_get",
        &json!({ "space": space.as_str(), "page": "next" }),
    )?;
    let columns: Vec<String> = board["columns"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_else(|| vec!["now".into(), "next".into(), "later".into(), "done".into()]);
    let mut items = vec![];
    for column in &columns {
        for card in board["board"][column].as_array().into_iter().flatten() {
            let id = card["card_id"]
                .as_str()
                .or_else(|| card["id"].as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            items.push(json!({
                "id": id,
                "fact_id": card["fact_id"],
                "type": "task",
                "space": space.as_str(),
                "column": column,
                "done": column == "done",
                "text": card["text"],
                "extract": extract(card["text"].as_str().unwrap_or(""), 160),
                "at": card["created"],
                "created_at": card["created_at"],
                "author": card["author"],
                "tags": card["tags"],
                "fields": card["fields"],
                "provenance": {
                    "kind": "memory",
                    "author": card["author"],
                    "source": { "kind": "agent", "agent": card["author"] },
                },
            }));
        }
    }
    Ok(json!({
        "contract": CONTRACT,
        "contract_version": CONTRACT_VERSION,
        "space": space.as_str(),
        "kind": kind,
        "label": label,
        "view": "tasks",
        "generated_at": util::iso(now),
        "columns": columns,
        "last_pass": board["last_pass"],
        "items": items,
        "page": { "limit": items.len(), "has_more": false, "next_cursor": Value::Null },
        "provenance": { "kind": "memory", "space": space.as_str(), "source": "living/next" },
        "notice": crate::tools::NOTICE,
    }))
}

/// A short, single-line extract of a text.
fn extract(text: &str, n: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        let cut: String = one.chars().take(n).collect();
        format!("{cut}…")
    }
}

// --- The dashboard: a stored document, computed with no model (docs/DASHBOARDS.md) ---------------

/// The dashboard of a space, with no model: a JSON document (DASHBOARDS.md) kept in the space as an
/// active fact tagged `dashboard`, or, when there is none, the plain default (the notes day by day).
/// Each section is computed here from the facts of the space; a section the catalogue does not know
/// is shown as an error and nothing else, and a document is never run, only read.
pub fn dashboard(conn: &Connection, _kind: &str, now: i64) -> Value {
    let (doc, source) = match stored_dashboard(conn) {
        Some(d) => (d, "stored"),
        None => (
            json!({ "version": 1, "sections": [
                { "type": "daily", "title": "Notes day by day", "window_days": WINDOW_DAYS }
            ]}),
            "default",
        ),
    };
    let title = doc
        .get("title")
        .and_then(Value::as_str)
        .map(|t| extract(t, 80));
    let sections: Vec<Value> = doc
        .get("sections")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_SECTIONS)
                .enumerate()
                .map(|(i, s)| {
                    let mut out = section(conn, s, now);
                    out["id"] = json!(format!("section:{i}"));
                    out
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "source": source, "title": title, "sections": sections })
}

/// The newest active fact tagged `dashboard` that reads as a version 1 document.
fn stored_dashboard(conn: &Connection) -> Option<Value> {
    let mut st = conn
        .prepare(
            "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id IN
             (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%') ORDER BY f.created_at DESC LIMIT 5",
        )
        .ok()?;
    let ids: Vec<String> = st.query_map([], |r| r.get(0)).ok()?.flatten().collect();
    ids.iter().find_map(|id| {
        let f = facts::read_fact(conn, id).ok().flatten()?;
        let doc: Value = serde_json::from_str(f.text.trim()).ok()?;
        (doc.get("version").and_then(Value::as_i64) == Some(1)).then_some(doc)
    })
}

fn days_of(s: &Value, default: i64) -> i64 {
    s.get("window_days")
        .and_then(Value::as_i64)
        .unwrap_or(default)
        .clamp(1, 365)
}

fn tag_of(s: &Value, default: &str) -> String {
    let raw = s.get("tag").and_then(Value::as_str).unwrap_or(default);
    facts::normalize_tag(raw).unwrap_or_else(|_| default.to_string())
}

/// One section of a dashboard, with its data.
fn section(conn: &Connection, s: &Value, now: i64) -> Value {
    let kind = s.get("type").and_then(Value::as_str).unwrap_or("");
    let title = s
        .get("title")
        .and_then(Value::as_str)
        .map(|t| extract(t, 80));
    let mut out = json!({ "type": kind, "title": title });
    match kind {
        "daily" => {
            let days = days_of(s, WINDOW_DAYS);
            out["window_days"] = json!(days);
            out["days"] = per_day(conn, now, days);
        }
        "counts" => {
            let days = days_of(s, WINDOW_DAYS);
            let top = s
                .get("top")
                .and_then(Value::as_i64)
                .unwrap_or(12)
                .clamp(1, 50) as usize;
            match s.get("from").and_then(Value::as_str) {
                Some("journal-manifestations") => {
                    out["window_days"] = json!(days);
                    let c = signs(conn, now, days, top);
                    out["days_with_entries"] = c["days_with_entries"].clone();
                    out["top"] = c["top"].clone();
                }
                Some("keywords") => {
                    out["window_days"] = json!(days);
                    let terms: Vec<String> = s
                        .get("terms")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(|t| t.trim().to_lowercase())
                                .filter(|t| !t.is_empty() && t.chars().count() <= 40)
                                .take(30)
                                .collect()
                        })
                        .unwrap_or_default();
                    out["top"] = mentions(conn, now, days, &terms, top);
                }
                Some("tags") => {
                    out["window_days"] = json!(days);
                    out["top"] = child_tags(conn, now, days, &tag_of(s, ""), top);
                }
                _ => out["error"] = json!("unknown source for counts"),
            }
        }
        "targets" => out["items"] = by_tag(conn, &tag_of(s, "cibles"), 12),
        "list" => {
            let limit = s
                .get("limit")
                .and_then(Value::as_i64)
                .unwrap_or(8)
                .clamp(1, 50) as usize;
            out["items"] = by_tag(conn, &tag_of(s, "inbox"), limit);
        }
        "text" => {
            out["body"] = json!(extract(
                s.get("body").and_then(Value::as_str).unwrap_or(""),
                2000
            ));
        }
        _ => out["error"] = json!("unknown section type"),
    }
    out
}

/// How many notes of the window cite each term (a project, a person, a sign): the keyword model of a
/// space, used to count rather than to file. Case is ignored.
fn mentions(conn: &Connection, now: i64, days: i64, terms: &[String], top_n: usize) -> Value {
    let since = now - days * DAY;
    let mut counts: Vec<(String, i64)> = terms.iter().map(|t| (t.clone(), 0)).collect();
    if let Ok(mut st) = conn.prepare(
        "SELECT text FROM facts WHERE status = 'active' AND created_at >= ?1
         AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
         ORDER BY created_at DESC LIMIT 5000",
    ) && let Ok(rows) = st.query_map([since], |r| r.get::<_, String>(0))
    {
        for text in rows.flatten() {
            let low = text.to_lowercase();
            for (t, n) in counts.iter_mut() {
                if low.contains(t.as_str()) {
                    *n += 1;
                }
            }
        }
    }
    counts.retain(|(_, n)| *n > 0);
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts.truncate(top_n);
    Value::Array(
        counts
            .into_iter()
            .map(|(c, n)| json!({ "code": c, "count": n }))
            .collect(),
    )
}

/// How many notes of the window sit under each direct child of a tag (the projects under `work/projects`).
fn child_tags(conn: &Connection, now: i64, days: i64, base: &str, top_n: usize) -> Value {
    let since = now - days * DAY;
    let prefix = if base.is_empty() {
        String::new()
    } else {
        format!("{base}/")
    };
    let mut counts: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();
    if let Ok(mut st) = conn.prepare(
        "SELECT t.tag, t.fact_id FROM fact_tags t JOIN facts f ON f.id = t.fact_id
         WHERE f.status = 'active' AND f.created_at >= ?1 AND substr(t.tag, 1, 1) <> '@'
         AND t.fact_id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')",
    ) && let Ok(rows) = st.query_map([since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    }) {
        for (tag, id) in rows.flatten() {
            let Some(rest) = tag.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let child = rest.split('/').next().unwrap_or("");
            if !child.is_empty() {
                counts.entry(child.to_string()).or_default().insert(id);
            }
        }
    }
    let mut top: Vec<(String, i64)> = counts
        .into_iter()
        .map(|(c, ids)| (c, ids.len() as i64))
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(top_n);
    Value::Array(
        top.into_iter()
            .map(|(c, n)| json!({ "code": c, "count": n }))
            .collect(),
    )
}

/// Notes written per day over a window, as `[{day, count}]`.
fn per_day(conn: &Connection, now: i64, days: i64) -> Value {
    let since = now - days * DAY;
    let mut out: Vec<Value> = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT date(created_at, 'unixepoch'), count(*) FROM facts
         WHERE status = 'active' AND created_at >= ?1
         AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
         GROUP BY 1 ORDER BY 1",
    ) && let Ok(rows) = st.query_map([since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    }) {
        for (d, n) in rows.flatten() {
            out.push(json!({ "day": d, "count": n }));
        }
    }
    Value::Array(out)
}

/// Counts the manifestations (signs and symptoms) of the journal days over a window. The health app
/// deposits one fact per day, `[app] day YYYY-MM-DD: {json}`, under a `dossier/…` path.
fn signs(conn: &Connection, now: i64, days_window: i64, top_n: usize) -> Value {
    let from_day = util::iso(now - days_window * DAY);
    let from_day = from_day.get(..10).unwrap_or("").to_string();
    let mut counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut days = 0i64;
    let Ok(mut st) = conn.prepare(
        "SELECT f.text FROM facts f
         WHERE f.status = 'active' AND f.id IN
           (SELECT fact_id FROM fact_tags WHERE tag = 'dossier' OR tag LIKE 'dossier/%')
         ORDER BY f.created_at DESC LIMIT 2000",
    ) else {
        return json!({ "days_with_entries": 0, "top": [] });
    };
    let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) else {
        return json!({ "days_with_entries": 0, "top": [] });
    };
    for text in rows.flatten() {
        let Some(at) = text.find("] day ") else {
            continue;
        };
        let rest = &text[at + 6..];
        let (Some(date), Some(json_at)) = (rest.get(..10), rest.find(": ")) else {
            continue;
        };
        if date < from_day.as_str() {
            continue;
        }
        let Ok(day) = serde_json::from_str::<Value>(&rest[json_at + 2..]) else {
            continue;
        };
        days += 1;
        for note in day
            .get("notes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for m in note
                .get("manifestations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(code) = m.get("code").and_then(Value::as_str) {
                    *counts.entry(code.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    let mut top: Vec<(String, i64)> = counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(top_n);
    json!({
        "days_with_entries": days,
        "top": top.into_iter().map(|(c, n)| json!({ "code": c, "count": n })).collect::<Vec<_>>(),
    })
}

/// The active facts filed under a tag path, newest first.
fn by_tag(conn: &Connection, tag: &str, limit: usize) -> Value {
    let like = format!("{tag}/%");
    let Ok(mut st) = conn.prepare(
        "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id IN
           (SELECT fact_id FROM fact_tags WHERE tag = ?2 OR tag LIKE ?3)
         ORDER BY f.created_at DESC LIMIT ?1",
    ) else {
        return json!([]);
    };
    let ids: Vec<String> = st
        .query_map(rusqlite::params![limit as i64, tag, like], |r| r.get(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    Value::Array(
        ids.iter()
            .filter_map(|id| facts::read_fact(conn, id).ok().flatten())
            .map(|f| {
                let mut row = f.to_json();
                row["extract"] = json!(extract(&f.text, 160));
                row
            })
            .collect(),
    )
}
