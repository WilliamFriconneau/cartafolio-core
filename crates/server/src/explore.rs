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

/// SQL: what counts as a note of the person. The page's own furniture (filing rules, dashboard, living pages) does
/// not, so every count on every screen is the number of notes the journal lists.
pub const NOT_FURNITURE: &str = "id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')";

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
        "fields": memcp_core::autotag::fields_json(&f.tags),
        "author": f.author,
        "created_at": f.created_at,
        "created": iso(f.created_at),
        "status": f.status,
        "replaces": f.replaces,
    })
}

/// A note, with the file it describes when it is the note of an attachment.
fn row_with_file(conn: &Connection, f: &facts::Fact) -> Value {
    let mut row = fact_row(f);
    if f.typ.as_deref() == Some("file")
        && let Ok(file) = conn.query_row(
            "SELECT id, name, mime, size FROM files WHERE fact_id = ?1 AND status = 'active' LIMIT 1",
            [&f.id],
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
        row["file"] = file;
        if let Some(v) = voice_state(conn, f) {
            row["voice"] = v;
        }
    }
    row
}

/// Where a voice note stands: `waiting` for its text, `transcribed`, or `failed` (with the id of the failure note, which
/// the person retires to have it read again). Only for a recording still under `inbox/audio`.
fn voice_state(conn: &Connection, f: &facts::Fact) -> Option<Value> {
    if !f.tags.iter().any(|t| t == "inbox/audio") {
        return None;
    }
    let file = conn
        .query_row(
            "SELECT id FROM files WHERE fact_id = ?1 AND status = 'active' AND mime LIKE 'audio/%' LIMIT 1",
            [&f.id],
            |r| r.get::<_, String>(0),
        )
        .ok()?;
    let waiting = memcp_core::files::recordings_waiting(conn)
        .unwrap_or_default()
        .iter()
        .any(|r| r.0 == file);
    if !waiting {
        return Some(json!({ "state": "transcribed" }));
    }
    let marker: Option<String> = conn
        .query_row(
            "SELECT d.id FROM facts d JOIN fact_tags x ON x.fact_id = d.id
             WHERE d.status = 'active' AND x.tag = ?1 AND instr(d.text, ?2) > 0 LIMIT 1",
            rusqlite::params![memcp_core::files::VOICE_FAILED_TAG, file],
            |r| r.get(0),
        )
        .ok();
    Some(match marker {
        Some(m) => json!({ "state": "failed", "marker": m }),
        None => json!({ "state": "waiting" }),
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
    let mut counts = facts::counts(&conn).unwrap_or(Value::Null);
    if let Ok(n) = conn.query_row(
        &format!("SELECT count(*) FROM facts WHERE status = 'active' AND {NOT_FURNITURE}"),
        [],
        |r| r.get::<_, i64>(0),
    ) {
        counts["active"] = json!(n);
    }
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
            "rules": rules_view(app, ctl, me, &space, &conn),
            "rules_history": rules_history(&conn),
        }),
    )
}

/// The rules of a space, with which keywords it shares with another of the person's spaces: a keyword only this
/// space lists is *unique* and files a note here at once; a shared one only scores.
fn rules_view(app: &App, ctl: &Connection, me: &User, space: &Space, conn: &Connection) -> Value {
    let mut rules = read_rules(conn).1;
    let mine = clean_list(rules.get("keywords"), 100, false);
    let mut shared: Vec<String> = vec![];
    if let Ok(reach) = users::access_of(ctl, &me.id) {
        for other in reach.keys().filter(|s| *s != space) {
            let Ok(c) = app.brain.open_space(other) else {
                continue;
            };
            let theirs = clean_list(read_rules(&c).1.get("keywords"), 100, false);
            shared.extend(mine.iter().filter(|k| theirs.contains(k)).cloned());
        }
    }
    shared.sort();
    shared.dedup();
    rules["shared"] = json!(shared);
    rules
}

/// Who changed the filing rules, when, and what the keywords and tags were each time (newest first).
fn rules_history(conn: &Connection) -> Vec<Value> {
    let Some(id) = read_rules(conn).0 else {
        return vec![];
    };
    let chain = facts::history(conn, &id).unwrap_or_default();
    let mut out = vec![];
    let mut before: (Vec<String>, Vec<String>) = (vec![], vec![]);
    let mut reason: Option<String> = None;
    for f in &chain {
        let doc = serde_json::from_str::<Value>(f.text.trim()).unwrap_or(Value::Null);
        let now = (
            clean_list(doc.get("keywords"), 100, false),
            clean_list(doc.get("tags"), 100, true),
        );
        let diff = |a: &[String], b: &[String]| -> (Vec<String>, Vec<String>) {
            (
                b.iter().filter(|x| !a.contains(x)).cloned().collect(),
                a.iter().filter(|x| !b.contains(x)).cloned().collect(),
            )
        };
        let (kw_added, kw_removed) = diff(&before.0, &now.0);
        let (tag_added, tag_removed) = diff(&before.1, &now.1);
        // The reason a version was made is written on the version it replaced.
        let janitor = f.author.contains("janitor")
            || reason.as_deref().is_some_and(|r| r.contains("janitor"));
        out.push(json!({
            "by": f.author, "at": iso(f.created_at), "janitor": janitor,
            "keywords_added": kw_added, "keywords_removed": kw_removed,
            "tags_added": tag_added, "tags_removed": tag_removed,
        }));
        before = now;
        reason = f.status_reason.clone();
    }
    out.reverse();
    out.truncate(30);
    out
}

/// The dashboard of a space, with no model: a JSON document (DASHBOARDS.md) kept in the space as an
/// active fact tagged `dashboard`, or, when there is none, the plain default (the notes day by day).
/// Each section is computed here from the facts of the space; a section the catalogue does not know
/// is shown as an error and nothing else, and a document is never run, only read.
fn dashboard(conn: &Connection, _kind: &str, now: i64) -> Value {
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
                .map(|s| section(conn, s, now))
                .collect()
        })
        .unwrap_or_default();
    json!({ "source": source, "title": title, "sections": sections })
}

const MAX_SECTIONS: usize = 20;

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
    let from_day = iso(now - days_window * DAY);
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
    // `status=retired`: the notes that were removed, newest first, for bringing one back.
    let rows: Vec<facts::Fact> = if req.query.get("status").map(String::as_str) == Some("retired") {
        let Ok(mut st) = conn.prepare(
            "SELECT id FROM facts WHERE status = 'retired'
             AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
             ORDER BY status_at DESC, rowid DESC LIMIT ?1",
        ) else {
            return err(500, "storage failed");
        };
        let ids: Vec<String> = st
            .query_map([limit as i64], |r| r.get(0))
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        ids.iter()
            .filter_map(|id| facts::read_fact(&conn, id).ok().flatten())
            .collect()
    } else if q.is_empty() {
        let like = format!("{tag}/%");
        let sql = if tag.is_empty() {
            "SELECT id FROM facts WHERE status = 'active'
             AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
             ORDER BY created_at DESC LIMIT ?1"
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
        &json!({ "facts": rows.iter().map(|f| row_with_file(&conn, f)).collect::<Vec<_>>() }),
    )
}

/// The tags the page keeps out of every list: the notes that are its own furniture.
fn system_tag(tag: &str) -> bool {
    let top = tag.split('/').next().unwrap_or("");
    matches!(
        top,
        "dashboard" | "rules" | "living" | "living-meta" | "index" | "guide"
    )
}

/// The spaces of the person that a cross-space view reads: theirs, never the sorting box, optionally one only.
fn readable_spaces(
    app: &App,
    ctl: &Connection,
    me: &User,
    only: Option<&str>,
) -> Result<Vec<Space>, Response> {
    let reach = users::access_of(ctl, &me.id).map_err(fail)?;
    if let Some(name) = only.filter(|s| !s.is_empty()) {
        let space = Space::new(name).map_err(|_| err(400, "invalid space"))?;
        if !reach.contains_key(&space) {
            return Err(err(404, "unknown space"));
        }
        return Ok(vec![space]);
    }
    Ok(app
        .brain
        .spaces()
        .map_err(fail)?
        .into_iter()
        // Inbox too: the journal is every note of every space (docs/UI-SPEC.md), what waits to be filed included.
        .filter(|s| reach.contains_key(s))
        .collect())
}

/// `GET /journal[?space=NAME][&tag=PATH][&q=WORDS][&limit=N][&before=TS&before_id=ID]`: the notes of every space the
/// person reads, newest first, each with its space. `before` and `before_id` continue after the last note of a page
/// (`next` in the answer); `tags` (first page only) are the tags to filter by, with their counts across the spaces.
pub fn journal(app: &App, ctl: &Connection, me: &User, req: &Request, now: i64) -> Response {
    let spaces = match readable_spaces(app, ctl, me, req.query.get("space").map(String::as_str)) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let limit = req
        .query
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, PAGE_MAX);
    let before = req.query.get("before").and_then(|b| b.parse::<i64>().ok());
    let before_id = req.query.get("before_id").cloned().unwrap_or_default();
    let tag = req.query.get("tag").map(String::as_str).unwrap_or("");
    let q = req.query.get("q").map(|s| s.trim()).unwrap_or("");
    let past = |f: &facts::Fact| match before {
        None => true,
        Some(t) => f.created_at < t || (f.created_at == t && f.id < before_id),
    };
    let mut rows: Vec<(facts::Fact, Value)> = vec![];
    let mut tag_totals: std::collections::BTreeMap<String, i64> = Default::default();
    for space in &spaces {
        let Ok(conn) = app.brain.open_space(space) else {
            continue;
        };
        if before.is_none() {
            for (t, n) in facts::tag_counts(&conn).unwrap_or_default() {
                if !system_tag(&t) && t != "field" && !t.starts_with("field/") {
                    *tag_totals.entry(t).or_insert(0) += n;
                }
            }
        }
        let found: Vec<facts::Fact> = if q.is_empty() {
            let like = format!("{tag}/%");
            let sql = format!(
                "SELECT id FROM facts WHERE status = 'active'
                 AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
                 {}
                 AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
                 ORDER BY created_at DESC, id DESC LIMIT ?1",
                if tag.is_empty() {
                    ""
                } else {
                    "AND id IN (SELECT fact_id FROM fact_tags WHERE tag = ?4 OR tag LIKE ?5)"
                }
            );
            let Ok(mut st) = conn.prepare(&sql) else {
                continue;
            };
            let take = (limit + 1) as i64;
            let ids: Vec<String> = if tag.is_empty() {
                st.query_map(rusqlite::params![take, before, before_id], |r| r.get(0))
                    .map(|r| r.flatten().collect())
                    .unwrap_or_default()
            } else {
                st.query_map(rusqlite::params![take, before, before_id, tag, like], |r| {
                    r.get(0)
                })
                .map(|r| r.flatten().collect())
                .unwrap_or_default()
            };
            ids.iter()
                .filter_map(|id| facts::read_fact(&conn, id).ok().flatten())
                .collect()
        } else {
            let opts = RecallOptions {
                limit: limit * 3 + 1,
                include_expired: true,
                tags: if tag.is_empty() {
                    vec![]
                } else {
                    vec![tag.to_string()]
                },
                ..Default::default()
            };
            match facts::recall(&conn, q, &opts, now) {
                Ok(hits) => hits
                    .into_iter()
                    .map(|h| h.fact)
                    .filter(|f| past(f) && !f.tags.iter().any(|t| system_tag(t)))
                    .collect(),
                Err(e) => return fail(e),
            }
        };
        for f in found {
            let mut row = row_with_file(&conn, &f);
            row["space"] = json!(space.as_str());
            rows.push((f, row));
        }
    }
    rows.sort_by(|a, b| (b.0.created_at, &b.0.id).cmp(&(a.0.created_at, &a.0.id)));
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = if more {
        rows.last()
            .map(|(f, _)| json!({ "before": f.created_at, "before_id": f.id }))
    } else {
        None
    };
    let mut out = json!({
        "facts": rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>(),
        "more": more,
        "next": next,
        "spaces": spaces.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    });
    if before.is_none() {
        let mut tags: Vec<(String, i64)> = tag_totals.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        tags.truncate(80);
        out["tags"] = json!(
            tags.into_iter()
                .map(|(t, n)| json!({ "tag": t, "count": n }))
                .collect::<Vec<_>>()
        );
    }
    Response::json(200, &out)
}

/// `GET /links[?space=NAME]`: the graph of what the person reads (the `links` tool, as the signed-in person) with
/// the keywords of each space added: nodes are spaces, tags, keywords and library folders; edges are what they
/// share. The sorting box and the page's own furniture (rules, dashboard…) are left out.
pub fn graph(app: &App, ctl: &Connection, me: &User, req: &Request) -> Response {
    let spaces = match readable_spaces(app, ctl, me, req.query.get("space").map(String::as_str)) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let names: Vec<&str> = spaces.iter().map(|s| s.as_str()).collect();
    let mut args = json!({ "max_tags": 150 });
    if let Some(s) = req.query.get("space").filter(|s| !s.is_empty()) {
        args["space"] = json!(s);
    }
    let mut g = match crate::admin::living(app, ctl, me, "links", &args) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut nodes: Vec<Value> = g["nodes"].as_array().cloned().unwrap_or_default();
    let mut edges: Vec<Value> = g["edges"].as_array().cloned().unwrap_or_default();
    let in_scope = |id: &str| {
        id.strip_prefix("space:")
            .map(|s| names.contains(&s))
            .unwrap_or(true)
    };
    let hidden_tag = |id: &str| id.strip_prefix("tag:").is_some_and(system_tag);
    nodes.retain(|n| {
        let id = n["id"].as_str().unwrap_or("");
        in_scope(id) && !hidden_tag(id)
    });
    edges.retain(|e| {
        let (a, b) = (
            e["source"].as_str().unwrap_or(""),
            e["target"].as_str().unwrap_or(""),
        );
        in_scope(a) && in_scope(b) && !hidden_tag(a) && !hidden_tag(b)
    });
    // A space is as big as the notes the journal lists for it.
    for n in nodes.iter_mut().filter(|n| n["type"] == "space") {
        let Some(name) = n["id"].as_str().and_then(|i| i.strip_prefix("space:")) else {
            continue;
        };
        if let Ok(space) = Space::new(name)
            && let Ok(conn) = app.brain.open_space(&space)
            && let Ok(c) = conn.query_row(
                &format!("SELECT count(*) FROM facts WHERE status = 'active' AND {NOT_FURNITURE}"),
                [],
                |r| r.get::<_, i64>(0),
            )
        {
            n["count"] = json!(c);
        }
    }
    // The keywords of each space: a node each, joined to every space that lists it, sized by the notes that cite it.
    let mut seen: std::collections::BTreeMap<String, i64> = Default::default();
    for space in &spaces {
        let Ok(conn) = app.brain.open_space(space) else {
            continue;
        };
        for k in clean_list(read_rules(&conn).1.get("keywords"), 100, false) {
            let like = format!(
                "%{}%",
                k.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM facts WHERE status = 'active' AND lower(text) LIKE ?1 ESCAPE '\\'
                     AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag = 'rules')",
                    [&like],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            *seen.entry(k.clone()).or_insert(0) += n;
            edges.push(
                json!({ "type": "keyword", "source": format!("space:{space}"),
                "target": format!("keyword:{k}"), "count": n.max(1) }),
            );
        }
    }
    for (k, n) in seen {
        nodes.push(
            json!({ "id": format!("keyword:{k}"), "type": "keyword", "label": k, "count": n.max(1) }),
        );
    }
    g["nodes"] = json!(nodes);
    g["edges"] = json!(edges);
    Response::json(200, &g)
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
            Response::json(
                200,
                &json!({ "fact": row_with_file(&conn, &f), "history": history }),
            )
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
            let mut new = NewFact {
                tags: tags_of(body),
                ..NewFact::agent(text_of(body)?, "web-page")
            };
            // The node tags what the page writes, as it tags every write.
            let text = new.text.clone();
            memcp_core::autotag::add_to(&app.brain, &me.id, &space, &conn, &text, &mut new.tags);
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
            let old = facts::read_fact(&conn, id).map_err(fail)?;
            let retagged = !new.tags.is_empty();
            let f = facts::supersede(&conn, &space, &who, id, new, reason).map_err(fail)?;
            // The person changed the tags of a note: the node learns it.
            if let (true, Some(old)) = (retagged, old) {
                memcp_core::autotag::learn_change(
                    &app.brain, &who, &space, &f.text, &old.tags, &f.tags,
                );
            }
            Ok(json!({ "result": "replaced", "fact": fact_row(&f) }))
        }
        "/fact/restore" => {
            let id = body
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| err(400, "id is required"))?;
            let reason = body.get("reason").and_then(Value::as_str);
            let f = facts::restore(&conn, &space, &who, id, reason).map_err(fail)?;
            Ok(json!({ "result": "restored", "fact": fact_row(&f) }))
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

/// `GET /pairing/status?code=…`: whether the code shown on the page was used, and by which connection.
pub fn pairing_status(ctl: &Connection, me: &User, req: &Request, now: i64) -> Response {
    use memcp_core::pairing::{self, Standing};
    let code = req.query.get("code").map(String::as_str).unwrap_or("");
    let body = match pairing::standing(ctl, &me.id, code, now) {
        Standing::Waiting => json!({ "state": "waiting" }),
        Standing::Joined(agent) => json!({ "state": "joined", "connection": agent }),
        Standing::Expired => json!({ "state": "expired" }),
        Standing::Unknown => json!({ "state": "unknown" }),
    };
    Response::json(200, &body)
}

/// The filing rules of a space: the tags and the keywords that send a note there, kept as one note of the space
/// tagged `rules` (a JSON document, `{"version":1,"tags":[…],"keywords":[…]}`), so that they have a history.
fn read_rules(conn: &Connection) -> (Option<String>, Value) {
    let found = conn
        .prepare(
            "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id IN
             (SELECT fact_id FROM fact_tags WHERE tag = 'rules') ORDER BY f.created_at DESC LIMIT 1",
        )
        .ok()
        .and_then(|mut st| {
            st.query_row([], |r| r.get::<_, String>(0)).ok()
        });
    if let Some(id) = found
        && let Some(f) = facts::read_fact(conn, &id).ok().flatten()
        && let Ok(doc) = serde_json::from_str::<Value>(f.text.trim())
        && doc.get("version").and_then(Value::as_i64) == Some(1)
    {
        return (Some(id), doc);
    }
    (None, json!({ "version": 1, "tags": [], "keywords": [] }))
}

fn clean_list(v: Option<&Value>, max: usize, tags: bool) -> Vec<String> {
    let mut out: Vec<String> = v
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|t| {
            if tags {
                facts::normalize_tag(t).ok()
            } else {
                let k = t.trim().trim_start_matches('#').to_lowercase();
                (!k.is_empty() && k.chars().count() <= 40).then_some(k)
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out.truncate(max);
    out
}

/// Writes the rules of a space (a new version of the note, the old one kept in the history).
fn write_rules(
    conn: &Connection,
    space: &Space,
    who: &str,
    tags: Vec<String>,
    keywords: Vec<String>,
) -> Result<(), Response> {
    let doc = json!({ "version": 1, "tags": tags, "keywords": keywords }).to_string();
    let mut new = NewFact::agent(&doc, "web-page");
    new.tags = vec!["rules".into()];
    match read_rules(conn).0 {
        Some(old) => facts::supersede(conn, space, who, &old, new, Some("rules changed"))
            .map(|_| ())
            .map_err(fail),
        None => match facts::remember(conn, space, who, new, &[]).map_err(fail)? {
            Remembered::NeedsDecision(c) => {
                // The settings note is never a near duplicate of a note: write it past the candidates.
                let ids: Vec<String> = c.into_iter().map(|c| c.id).collect();
                let mut again = NewFact::agent(&doc, "web-page");
                again.tags = vec!["rules".into()];
                facts::remember(conn, space, who, again, &ids)
                    .map(|_| ())
                    .map_err(fail)
            }
            _ => Ok(()),
        },
    }
}

/// `POST /space/rules` {space, tags?, keywords?} and `POST /space/move-tag` {from, to, tag, dry_run?}.
pub fn space_write(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    body: &Value,
) -> Result<Value, Response> {
    let who = author(me);
    match path {
        "/space/rules" => {
            let name = body
                .get("space")
                .and_then(Value::as_str)
                .ok_or_else(|| err(400, "space is required"))?;
            let (space, conn) = own(app, ctl, me, name)?;
            let (_, old) = read_rules(&conn);
            let tags = body
                .get("tags")
                .map(|v| clean_list(Some(v), 100, true))
                .unwrap_or_else(|| clean_list(old.get("tags"), 100, true));
            let keywords = body
                .get("keywords")
                .map(|v| clean_list(Some(v), 100, false))
                .unwrap_or_else(|| clean_list(old.get("keywords"), 100, false));
            if body.get("tags").is_some() || body.get("keywords").is_some() {
                write_rules(&conn, &space, &who, tags, keywords)?;
            }
            let list = |k: &str| -> Vec<String> {
                body.get(k)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let edit = memcp_core::rules::Edit {
                add_tags: list("add_tags"),
                remove_tags: list("remove_tags"),
                add_keywords: list("add_keywords"),
                remove_keywords: list("remove_keywords"),
            };
            memcp_core::rules::edit(&conn, &space, &who, &edit).map_err(fail)?;
            Ok(json!({ "result": "ok" }))
        }
        "/space/move-tag" => {
            let get = |k: &str| {
                body.get(k)
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(400, &format!("{k} is required")))
            };
            let (from, _) = own(app, ctl, me, get("from")?)?;
            let (to, to_conn) = own(app, ctl, me, get("to")?)?;
            if from == to {
                return Err(err(400, "a note cannot move to the space it is in"));
            }
            let tag = get("tag")?;
            let dry = body.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
            let moved = app
                .brain
                .move_by_tag(&from, &to, tag, &who, dry)
                .map_err(fail)?;
            if !dry && moved.moved + moved.already + moved.resumed > 0 {
                // What was moved is now the target's: a note with this tag is filed there from now on.
                let (_, old) = read_rules(&to_conn);
                let mut tags = clean_list(old.get("tags"), 100, true);
                if let Ok(t) = facts::normalize_tag(tag) {
                    tags.push(t);
                }
                tags.sort();
                tags.dedup();
                let keywords = clean_list(old.get("keywords"), 100, false);
                write_rules(&to_conn, &to, &who, tags, keywords)?;
            }
            Ok(moved.to_json(&from, &to, tag, dry))
        }
        _ => Err(err(404, "not found")),
    }
}
