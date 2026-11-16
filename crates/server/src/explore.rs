//! Exploring a space as its owner: an overview with a dashboard, the tree of tags, the notes, and the
//! edit of a note.
//!
//! The page is the human's way into the memory, as the tools are an assistant's: the same facts, the
//! same history. Only the signed-in person reaches this, and only their own spaces (administrators
//! read nothing of anyone else's). An edit is never an overwrite: a correction is a `supersede` that
//! keeps the old text in the history, a removal is a `retire` with its reason.

use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};
use memcp_core::users::{self, User};
use memcp_core::{Space, util::iso};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Request, Response};

const DAY: i64 = 86_400;
const WINDOW_DAYS: i64 = 30;
const PAGE_MAX: usize = 100;

fn err(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

fn fail(e: impl std::fmt::Display) -> Response {
    err(400, &e.to_string())
}

/// One of the person's own spaces, opened.
fn own(
    app: &App,
    ctl: &Connection,
    me: &User,
    name: &str,
) -> Result<(Space, Connection), Response> {
    let space = Space::new(name).map_err(|_| err(400, "invalid space"))?;
    let reach = users::access_of(ctl, &me.id).map_err(fail)?;
    if !reach.contains_key(&space) {
        return Err(err(404, "unknown space"));
    }
    let conn = app.brain.open_space(&space).map_err(fail)?;
    Ok((space, conn))
}

fn author(me: &User) -> String {
    format!("{}-web", me.name)
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

fn fact_row(f: &facts::Fact) -> Value {
    json!({
        "id": f.id,
        "text": f.text,
        "extract": extract(&f.text, 160),
        "tags": f.tags,
        "author": f.author,
        "created_at": f.created_at,
        "created": iso(f.created_at),
        "status": f.status,
        "replaces": f.replaces,
    })
}

/// `GET /space?space=NAME`: the kind, the place in the tree, the counts, the tree of tags and the
/// dashboard of the space.
pub fn overview(app: &App, ctl: &Connection, me: &User, req: &Request, now: i64) -> Response {
    let Some(name) = req.query.get("space") else {
        return err(400, "space is required");
    };
    let (space, conn) = match own(app, ctl, me, name) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let kind = app
        .brain
        .space_kind(&space)
        .unwrap_or_else(|_| "custom".into());
    let place = app
        .brain
        .layout(&me.id)
        .ok()
        .and_then(|l| l.into_iter().find(|p| p.space == space));
    let counts = facts::counts(&conn).unwrap_or(Value::Null);
    let tags: Vec<Value> = facts::tag_counts(&conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(t, n)| json!({ "tag": t, "count": n }))
        .collect();
    let dashboard = dashboard(&conn, &kind, now);
    Response::json(
        200,
        &json!({
            "space": space.as_str(),
            "label": place.as_ref().map(|p| p.label.clone()),
            "parent": place.as_ref().and_then(|p| p.parent.as_ref().map(|s| s.to_string())),
            "kind": kind,
            "counts": counts,
            "tags": tags,
            "dashboard": dashboard,
        }),
    )
}

/// The dashboard of a space, with no model: what is counted from the facts themselves. Every space
/// shows how many facts it received, day by day; a health space also counts the signs and symptoms
/// of its journal and lists its targets.
fn dashboard(conn: &Connection, kind: &str, now: i64) -> Value {
    let since = now - WINDOW_DAYS * DAY;
    let mut per_day: Vec<Value> = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT date(created_at, 'unixepoch'), count(*) FROM facts
         WHERE status = 'active' AND created_at >= ?1 GROUP BY 1 ORDER BY 1",
    ) && let Ok(rows) = st.query_map([since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    }) {
        for (d, n) in rows.flatten() {
            per_day.push(json!({ "day": d, "count": n }));
        }
    }
    let mut out = json!({
        "window_days": WINDOW_DAYS,
        "facts_per_day": per_day,
    });
    if kind == "health" {
        out["signs"] = signs(conn, now);
        out["targets"] = targets(conn);
    }
    out
}

/// Counts the manifestations (signs and symptoms) of the journal days over the window. The app
/// deposits one fact per day, `[app] day YYYY-MM-DD: {json}`, under a `dossier/…` path.
fn signs(conn: &Connection, now: i64) -> Value {
    let from_day = iso(now - WINDOW_DAYS * DAY);
    let from_day = from_day.get(..10).unwrap_or("").to_string();
    let mut counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut days = 0i64;
    let Ok(mut st) = conn.prepare(
        "SELECT f.text FROM facts f
         WHERE f.status = 'active' AND f.id IN
           (SELECT fact_id FROM fact_tags WHERE tag = 'dossier' OR tag LIKE 'dossier/%')
         ORDER BY f.created_at DESC LIMIT 2000",
    ) else {
        return Value::Null;
    };
    let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) else {
        return Value::Null;
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
    top.truncate(20);
    json!({
        "days_with_entries": days,
        "top": top.into_iter().map(|(c, n)| json!({ "code": c, "count": n })).collect::<Vec<_>>(),
    })
}

/// The targets the person follows: the active facts filed under `cibles`.
fn targets(conn: &Connection) -> Value {
    let Ok(mut st) = conn.prepare(
        "SELECT f.id FROM facts f
         WHERE f.status = 'active' AND f.id IN
           (SELECT fact_id FROM fact_tags WHERE tag = 'cibles' OR tag LIKE 'cibles/%')
         ORDER BY f.created_at DESC LIMIT 12",
    ) else {
        return json!([]);
    };
    let ids: Vec<String> = st
        .query_map([], |r| r.get(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    Value::Array(
        ids.iter()
            .filter_map(|id| facts::read_fact(conn, id).ok().flatten())
            .map(|f| fact_row(&f))
            .collect(),
    )
}

/// `GET /facts?space=NAME[&tag=PATH][&q=WORDS][&limit=N]`: notes, newest first, or the best matches.
pub fn list(app: &App, ctl: &Connection, me: &User, req: &Request, now: i64) -> Response {
    let Some(name) = req.query.get("space") else {
        return err(400, "space is required");
    };
    let (_, conn) = match own(app, ctl, me, name) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let limit = req
        .query
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(40)
        .clamp(1, PAGE_MAX);
    let tag = req.query.get("tag").map(String::as_str).unwrap_or("");
    let q = req.query.get("q").map(|s| s.trim()).unwrap_or("");
    let rows: Vec<facts::Fact> = if q.is_empty() {
        let like = format!("{tag}/%");
        let sql = if tag.is_empty() {
            "SELECT id FROM facts WHERE status = 'active' ORDER BY created_at DESC LIMIT ?1"
                .to_string()
        } else {
            "SELECT id FROM facts WHERE status = 'active' AND id IN
             (SELECT fact_id FROM fact_tags WHERE tag = ?2 OR tag LIKE ?3)
             ORDER BY created_at DESC LIMIT ?1"
                .to_string()
        };
        let Ok(mut st) = conn.prepare(&sql) else {
            return err(500, "storage failed");
        };
        let ids: Vec<String> = if tag.is_empty() {
            st.query_map([limit as i64], |r| r.get(0))
                .map(|r| r.flatten().collect())
                .unwrap_or_default()
        } else {
            st.query_map(rusqlite::params![limit as i64, tag, like], |r| r.get(0))
                .map(|r| r.flatten().collect())
                .unwrap_or_default()
        };
        ids.iter()
            .filter_map(|id| facts::read_fact(&conn, id).ok().flatten())
            .collect()
    } else {
        let opts = RecallOptions {
            limit,
            include_expired: true,
            tags: if tag.is_empty() {
                vec![]
            } else {
                vec![tag.to_string()]
            },
            ..Default::default()
        };
        match facts::recall(&conn, q, &opts, now) {
            Ok(hits) => hits.into_iter().map(|h| h.fact).collect(),
            Err(e) => return fail(e),
        }
    };
    Response::json(
        200,
        &json!({ "facts": rows.iter().map(fact_row).collect::<Vec<_>>() }),
    )
}

/// `GET /fact?space=NAME&id=ID`: a note with its history.
pub fn one(app: &App, ctl: &Connection, me: &User, req: &Request) -> Response {
    let (Some(name), Some(id)) = (req.query.get("space"), req.query.get("id")) else {
        return err(400, "space and id are required");
    };
    let (_, conn) = match own(app, ctl, me, name) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match facts::read_fact(&conn, id) {
        Ok(Some(f)) => {
            let history: Vec<Value> = facts::history(&conn, id)
                .unwrap_or_default()
                .iter()
                .map(fact_row)
                .collect();
            Response::json(200, &json!({ "fact": fact_row(&f), "history": history }))
        }
        Ok(None) => err(404, "unknown note"),
        Err(e) => fail(e),
    }
}

fn tags_of(body: &Value) -> Vec<String> {
    body.get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn text_of(body: &Value) -> Result<&str, Response> {
    body.get("text")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err(400, "text is required"))
}

/// `POST /fact/add`, `/fact/supersede`, `/fact/retire`.
pub fn write(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    body: &Value,
) -> Result<Value, Response> {
    let name = body
        .get("space")
        .and_then(Value::as_str)
        .ok_or_else(|| err(400, "space is required"))?;
    let (space, conn) = own(app, ctl, me, name)?;
    let who = author(me);
    match path {
        "/fact/add" => {
            let new = NewFact {
                tags: tags_of(body),
                ..NewFact::agent(text_of(body)?, "web-page")
            };
            let distinct: Vec<String> = body
                .get("distinct_from")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            match facts::remember(&conn, &space, &who, new, &distinct).map_err(fail)? {
                Remembered::Created(f) => Ok(json!({ "result": "created", "fact": fact_row(&f) })),
                Remembered::Duplicate(f) => {
                    Ok(json!({ "result": "duplicate", "fact": fact_row(&f) }))
                }
                Remembered::NeedsDecision(c) => Err(Response::json(
                    409,
                    &json!({
                        "error": "close notes exist: correct one of them, or add this one anyway",
                        "candidates": c.iter().map(|c| json!({
                            "id": c.id, "extract": extract(&c.text, 160), "similarity": c.similarity
                        })).collect::<Vec<_>>(),
                    }),
                )),
            }
        }
        "/fact/supersede" => {
            let id = body
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| err(400, "id is required"))?;
            let new = NewFact {
                tags: tags_of(body),
                ..NewFact::agent(text_of(body)?, "web-page")
            };
            let reason = body.get("reason").and_then(Value::as_str);
            let f = facts::supersede(&conn, &space, &who, id, new, reason).map_err(fail)?;
            Ok(json!({ "result": "replaced", "fact": fact_row(&f) }))
        }
        "/fact/retire" => {
            let id = body
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| err(400, "id is required"))?;
            let reason = body
                .get("reason")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| err(400, "a reason is required to remove a note"))?;
            let f = facts::retire(&conn, &who, id, reason).map_err(fail)?;
            Ok(json!({ "result": "retired", "fact": fact_row(&f) }))
        }
        _ => Err(err(404, "not found")),
    }
}

/// `GET /layout`: the tree of the person's own spaces (label, parent, position, kind), in order.
pub fn layout_get(app: &App, me: &User) -> Response {
    match app.brain.layout(&me.id) {
        Ok(places) => Response::json(
            200,
            &json!({ "spaces": places.iter().map(|p| json!({
                "space": p.space.as_str(),
                "label": p.label,
                "parent": p.parent.as_ref().map(|s| s.as_str()),
                "position": p.position,
                "kind": p.kind,
            })).collect::<Vec<_>>() }),
        ),
        Err(e) => fail(e),
    }
}

/// `POST /layout/set` (rename, move, reorder) and `POST /layout/add` (a space of the person's own).
/// Only the layout changes: no file moves and no right is rewritten.
pub fn layout_write(app: &App, me: &User, path: &str, body: &Value) -> Result<Value, Response> {
    use memcp_core::layout::Change;
    let parent_of = |v: &Value| -> Result<Option<Space>, Response> {
        match v.as_str().filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(p) => Space::new(p)
                .map(Some)
                .map_err(|_| err(400, "invalid parent")),
        }
    };
    match path {
        "/layout/set" => {
            let name = body
                .get("space")
                .and_then(Value::as_str)
                .ok_or_else(|| err(400, "space is required"))?;
            let space = Space::new(name).map_err(|_| err(400, "invalid space"))?;
            let change = Change {
                label: body.get("label").and_then(Value::as_str).map(String::from),
                parent: match body.get("parent") {
                    None => None,
                    Some(v) => Some(parent_of(v)?),
                },
                position: body.get("position").and_then(Value::as_i64),
            };
            app.brain
                .set_layout(&me.id, &space, &change)
                .map_err(fail)?;
            Ok(json!({ "result": "ok" }))
        }
        "/layout/add" => {
            let label = body
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| err(400, "label is required"))?;
            let parent = match body.get("parent") {
                None => None,
                Some(v) => parent_of(v)?,
            };
            let space = app
                .brain
                .add_custom_space(&me.id, &me.name, label, parent.as_ref())
                .map_err(fail)?;
            Ok(json!({ "result": "created", "space": space.as_str() }))
        }
        _ => Err(err(404, "not found")),
    }
}
