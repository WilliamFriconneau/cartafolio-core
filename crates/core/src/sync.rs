//! Peer synchronisation, the part that needs no network.
//!
//! A space keeps an **append-only log of events** (see `docs/FEDERATION.md`): a fact created, replaced or
//! retired, a file kept or retired, a layout field changed. Every event has a stable id (`eid`) derived from
//! what changed, so the same change received twice is one row, and a hybrid logical clock stamp (`hlc`).
//! A peer reads the log of another after its own cursor and applies it here, idempotently. Nothing is
//! overwritten and nothing is erased by a merge: what two nodes changed differently is kept both ways and
//! listed as a dispute.
//!
//! The log holds the changes made here and the ones received from peers (each tagged with the peer that
//! delivered it, `via`), so a third node can be fed from either. A peer is never sent back what it sent.
//!
//! Not synchronised yet: the raw records apps deposit (`device_docs`, `fact_origin`), the health record
//! table, guides and approvals, and the repointing of imported passages.

use std::collections::BTreeSet;

use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::facts;
use crate::space::Space;
use crate::text::{self, hex};
use crate::util;

/// How far ahead of this node's clock an event may be stamped, in milliseconds.
pub const MAX_DRIFT_MS: i64 = 60 * 60 * 1000;
/// A peer that moved events this recently is shown as sending or receiving.
pub const ACTIVE_SECS: i64 = 10;
/// Events in one page.
pub const PAGE_EVENTS: usize = 200;
/// Soft limit of the bytes of one page (a page always holds at least one event).
pub const PAGE_BYTES: usize = 8 * 1024 * 1024;
/// The prefix of a connection made for a peer.
pub const PEER_PREFIX: &str = "peer-";

/// The version of the exchange between peers. A node refuses a peer that speaks a newer one, and says which.
pub const PROTOCOL: i64 = 1;

/// One row of the log, as it travels.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The position in the log of the node that sent it. Meaningless elsewhere.
    pub seq: i64,
    pub eid: String,
    pub hlc: String,
    pub origin: String,
    pub kind: String,
    pub at: i64,
    pub payload: Value,
}

impl Event {
    pub fn to_json(&self) -> Value {
        json!({
            "seq": self.seq, "eid": self.eid, "hlc": self.hlc, "origin": self.origin,
            "kind": self.kind, "at": self.at, "payload": self.payload,
        })
    }

    pub fn from_json(v: &Value) -> Result<Event> {
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error::Invalid(format!("event: {k} is required")))
        };
        Ok(Event {
            seq: v.get("seq").and_then(Value::as_i64).unwrap_or(0),
            eid: text("eid")?,
            hlc: text("hlc")?,
            origin: text("origin")?,
            kind: text("kind")?,
            at: v
                .get("at")
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::Invalid("event: at is required".into()))?,
            payload: v
                .get("payload")
                .filter(|p| p.is_object())
                .cloned()
                .ok_or_else(|| Error::Invalid("event: payload must be an object".into()))?,
        })
    }
}

/// A page of the log.
#[derive(Clone, Debug)]
pub struct Page {
    pub events: Vec<Event>,
    /// The position to ask for next (past events left out because the asker sent them).
    pub next: i64,
    /// The last position of this log.
    pub head: i64,
    pub more: bool,
}

/// What applying events did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub applied: usize,
    /// Already known, or without effect here.
    pub skipped: usize,
    /// Changes that put a fact in dispute.
    pub disputed: usize,
    /// Events refused as malformed, with the reason. They are not kept.
    pub rejected: Vec<(String, String)>,
    /// How many events, from the first, are dealt with (applied, skipped or rejected).
    pub consumed: usize,
    /// Set when an event needs one that has not arrived: the rest waits.
    pub stopped: Option<String>,
}

// --- clock -------------------------------------------------------------------------------------

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT v FROM meta WHERE k = ?1", [key], |r| r.get(0))
        .optional()?)
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![key, value],
    )?;
    Ok(())
}

/// The identifier of this copy of the space, made once.
fn replica(conn: &Connection) -> Result<String> {
    if let Some(r) = meta_get(conn, "replica")? {
        return Ok(r);
    }
    conn.execute(
        "INSERT OR IGNORE INTO meta (k, v) VALUES ('replica', ?1)",
        [util::random_base32(8)],
    )?;
    meta_get(conn, "replica")?.ok_or_else(|| Error::Schema("replica not written".into()))
}

/// `(milliseconds, counter)` of a stamp `MMMMMMMMMMMMMMM.CCCCC.replica`.
fn parse_hlc(h: &str) -> Option<(i64, i64)> {
    let mut it = h.split('.');
    let ms = it.next()?.parse::<i64>().ok()?;
    let c = it.next()?.parse::<i64>().ok()?;
    let r = it.next()?;
    (it.next().is_none() && !r.is_empty() && r.len() <= 16 && ms >= 0 && c >= 0).then_some((ms, c))
}

fn last_hlc(conn: &Connection) -> Result<(i64, i64)> {
    Ok(meta_get(conn, "hlc_last")?
        .and_then(|s| {
            let (a, b) = s.split_once('.')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((0, 0)))
}

/// A new stamp, after every one this copy has issued or seen.
pub fn stamp(conn: &Connection) -> Result<String> {
    let (ms, c) = last_hlc(conn)?;
    let now = util::now_ms();
    let (ms, c) = if now > ms { (now, 0) } else { (ms, c + 1) };
    meta_set(conn, "hlc_last", &format!("{ms}.{c}"))?;
    Ok(format!("{ms:015}.{c:05}.{}", replica(conn)?))
}

/// Takes a stamp seen on another copy into account, so a later local stamp is after it.
fn observe(conn: &Connection, hlc: &str) -> Result<()> {
    if let Some(seen) = parse_hlc(hlc)
        && seen > last_hlc(conn)?
    {
        meta_set(conn, "hlc_last", &format!("{}.{}", seen.0, seen.1))?;
    }
    Ok(())
}

// --- writing the log -----------------------------------------------------------------------------

// One row of the log: each argument is one of its columns.
#[allow(clippy::too_many_arguments)]
fn record_raw(
    conn: &Connection,
    eid: &str,
    hlc: &str,
    origin: &str,
    via: Option<&str>,
    kind: &str,
    at: i64,
    payload: &Value,
) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO sync_log (eid, hlc, origin, via, kind, at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![eid, hlc, origin, via, kind, at, payload.to_string()],
    )?;
    Ok(n == 1)
}

/// Logs a change made here.
fn record(conn: &Connection, eid: &str, kind: &str, at: i64, payload: &Value) -> Result<()> {
    let hlc = stamp(conn)?;
    let origin = replica(conn)?;
    record_raw(conn, eid, &hlc, &origin, None, kind, at, payload)?;
    Ok(())
}

fn fact_payload(conn: &Connection, id: &str) -> Result<Value> {
    let row = conn
        .query_row(
            "SELECT kind, type, fields, text, source_kind, source_path, source_hash, source_range, source_ref,
                    source_agent, author, created_at, valid_from, valid_to, replaces, status
             FROM facts WHERE id = ?1",
            [id],
            |r| {
                let status: String = r.get(15)?;
                Ok(json!({
                    "id": id, "kind": r.get::<_, String>(0)?, "type": r.get::<_, Option<String>>(1)?,
                    "fields": r.get::<_, Option<String>>(2)?, "text": r.get::<_, String>(3)?,
                    "source_kind": r.get::<_, String>(4)?, "source_path": r.get::<_, Option<String>>(5)?,
                    "source_hash": r.get::<_, Option<String>>(6)?, "source_range": r.get::<_, Option<String>>(7)?,
                    "source_ref": r.get::<_, Option<String>>(8)?, "source_agent": r.get::<_, Option<String>>(9)?,
                    "author": r.get::<_, String>(10)?, "created_at": r.get::<_, i64>(11)?,
                    "valid_from": r.get::<_, i64>(12)?, "valid_to": r.get::<_, Option<i64>>(13)?,
                    "replaces": r.get::<_, Option<String>>(14)?,
                    "status": if status == "proposed" { "proposed" } else { "active" },
                }))
            },
        )
        .optional()?
        .ok_or_else(|| Error::UnknownFact(id.into()))?;
    let tags: Vec<String> = conn
        .prepare("SELECT tag FROM fact_tags WHERE fact_id = ?1 ORDER BY tag")?
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut row = row;
    row["tags"] = json!(tags);
    Ok(row)
}

/// A fact was written here.
pub(crate) fn record_fact_created(conn: &Connection, id: &str) -> Result<()> {
    let p = fact_payload(conn, id)?;
    let at = p["created_at"].as_i64().unwrap_or_else(util::now);
    record(conn, &format!("c:{id}"), "fact_created", at, &p)
}

/// `old` was replaced by `by` (which is logged as created before this).
pub(crate) fn record_fact_superseded(
    conn: &Connection,
    old: &str,
    by: &str,
    actor: &str,
    reason: Option<&str>,
) -> Result<()> {
    let at = util::now();
    let p = json!({ "old": old, "by": by, "actor": actor, "reason": reason, "at": at });
    record(conn, &format!("s:{old}:{by}"), "fact_superseded", at, &p)
}

pub(crate) fn record_fact_retired(
    conn: &Connection,
    id: &str,
    actor: &str,
    reason: &str,
) -> Result<()> {
    let at = util::now();
    let p = json!({ "id": id, "actor": actor, "reason": reason, "at": at });
    record(conn, &format!("r:{id}"), "fact_retired", at, &p)
}

/// A file was kept here (the bytes are read from disk when the event is sent).
pub(crate) fn record_file_put(conn: &Connection, id: &str) -> Result<()> {
    let mut p = conn.query_row(
        "SELECT name, mime, size, sha256, note, fact_id, author, created_at FROM files WHERE id = ?1",
        [id],
        |r| {
            Ok(json!({
                "id": id, "name": r.get::<_, String>(0)?, "mime": r.get::<_, String>(1)?,
                "size": r.get::<_, i64>(2)?, "sha256": r.get::<_, String>(3)?,
                "note": r.get::<_, String>(4)?, "fact_id": r.get::<_, Option<String>>(5)?,
                "author": r.get::<_, String>(6)?, "created_at": r.get::<_, i64>(7)?,
            }))
        },
    )?;
    if let Some(f) = p["fact_id"].as_str() {
        let linked: Option<(Option<String>, Option<String>)> = conn
            .query_row("SELECT type, fields FROM facts WHERE id = ?1", [f], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        if let Some((t, fields)) = linked {
            p["fact_type"] = json!(t);
            p["fact_fields"] = json!(fields);
        }
    }
    let at = p["created_at"].as_i64().unwrap_or_else(util::now);
    record(conn, &format!("f:{id}"), "file_put", at, &p)
}

pub(crate) fn record_file_retired(conn: &Connection, id: &str, reason: &str) -> Result<()> {
    let at = util::now();
    let p = json!({ "id": id, "reason": reason, "at": at });
    record(conn, &format!("fr:{id}"), "file_retired", at, &p)
}

/// A layout field of the space changed here (`label`, `parent` or `position`).
pub(crate) fn record_layout(conn: &Connection, field: &str, value: Value) -> Result<()> {
    let hlc = stamp(conn)?;
    let origin = replica(conn)?;
    let p = json!({ "field": field, "value": value });
    record_raw(
        conn,
        &format!("l:{field}:{hlc}"),
        &hlc,
        &origin,
        None,
        "layout_changed",
        util::now(),
        &p,
    )?;
    conn.execute(
        "INSERT INTO sync_fields (field, hlc) VALUES (?1, ?2) ON CONFLICT(field) DO UPDATE SET hlc = excluded.hlc",
        params![field, hlc],
    )?;
    Ok(())
}

/// A space that existed before the log gets the events of what it already holds, once, before anything else is
/// written: its facts, their replacements and retirements, its files. This is what makes the first join a
/// full copy.
pub(crate) fn ensure_log(conn: &Connection) -> Result<()> {
    if meta_get(conn, "sync_backfilled")?.is_some() {
        return Ok(());
    }
    // The write lock is taken at once (a deferred transaction that reads first and writes after cannot wait for the
    // writers already at work: « database is locked »), and the flag is read again once it is held, so that several
    // workers opening the space together backfill it once.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let done = (|| -> Result<()> {
        if meta_get(conn, "sync_backfilled")?.is_some() {
            return Ok(());
        }
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM facts ORDER BY created_at, rowid")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in &ids {
            record_fact_created(conn, id)?;
        }
        type Closed = (
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
        );
        let closed: Vec<Closed> = conn
            .prepare(
                "SELECT id, status, replaced_by, status_at, status_by, status_reason FROM facts
                 WHERE status IN ('replaced','retired') ORDER BY status_at, rowid",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (id, status, by, at, actor, reason) in closed {
            let actor = actor.unwrap_or_default();
            let at = at.unwrap_or_else(util::now);
            if status == "replaced" {
                if let Some(by) = by.filter(|b| *b != id) {
                    let p =
                        json!({ "old": id, "by": by, "actor": actor, "reason": reason, "at": at });
                    record(conn, &format!("s:{id}:{by}"), "fact_superseded", at, &p)?;
                }
            } else {
                let p = json!({ "id": id, "actor": actor, "reason": reason.unwrap_or_default(), "at": at });
                record(conn, &format!("r:{id}"), "fact_retired", at, &p)?;
            }
        }
        let files: Vec<(String, String, Option<String>)> = conn
            .prepare("SELECT id, status, retired_reason FROM files ORDER BY created_at, rowid")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, status, reason) in files {
            record_file_put(conn, &id)?;
            if status == "retired" {
                record_file_retired(conn, &id, &reason.unwrap_or_default())?;
            }
        }
        meta_set(conn, "sync_backfilled", &util::now().to_string())
    })();
    match done {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// The layout the person already gave this space, as events, once.
fn ensure_layout_log(brain: &Brain, space: &Space, conn: &Connection) -> Result<()> {
    if meta_get(conn, "sync_layout_logged")?.is_some() {
        return Ok(());
    }
    let ctl = brain.open_control()?;
    let row: Option<(Option<String>, Option<String>, i64)> = ctl
        .query_row(
            "SELECT label, parent, position FROM spaces WHERE name = ?1",
            [space.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((label, parent, position)) = row {
        if let Some(l) = label {
            record_layout(conn, "label", json!(l))?;
        }
        if let Some(p) = parent {
            record_layout(conn, "parent", json!(p))?;
        }
        if position != 0 {
            record_layout(conn, "position", json!(position))?;
        }
    }
    meta_set(conn, "sync_layout_logged", "1")
}

// --- reading the log -----------------------------------------------------------------------------

/// The last position of the space's log.
pub fn head(brain: &Brain, space: &Space) -> Result<i64> {
    let conn = brain.open_space(space)?;
    ensure_layout_log(brain, space, &conn)?;
    Ok(
        conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM sync_log", [], |r| {
            r.get(0)
        })?,
    )
}

/// The events after position `after`, at most `limit` and about `max_bytes`, without the ones `exclude_via`
/// delivered. A file's bytes travel inside its event.
pub fn events_after(
    brain: &Brain,
    space: &Space,
    after: i64,
    limit: usize,
    max_bytes: usize,
    exclude_via: Option<&str>,
) -> Result<Page> {
    let conn = brain.open_space(space)?;
    ensure_layout_log(brain, space, &conn)?;
    let head: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM sync_log", [], |r| {
        r.get(0)
    })?;
    let mut st = conn.prepare(
        "SELECT seq, eid, hlc, origin, via, kind, at, payload FROM sync_log WHERE seq > ?1 ORDER BY seq LIMIT ?2",
    )?;
    let rows = st.query_map(
        params![
            after.max(0),
            i64::try_from(limit.max(1)).unwrap_or(i64::MAX)
        ],
        |r| {
            Ok((
                Event {
                    seq: r.get(0)?,
                    eid: r.get(1)?,
                    hlc: r.get(2)?,
                    origin: r.get(3)?,
                    kind: r.get(5)?,
                    at: r.get(6)?,
                    payload: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(Value::Null),
                },
                r.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    let mut events = Vec::new();
    let mut next = after.max(0);
    let mut bytes = 0usize;
    let mut stopped_early = false;
    for row in rows {
        let (mut ev, via) = row?;
        if bytes >= max_bytes && !events.is_empty() {
            stopped_early = true;
            break;
        }
        next = ev.seq;
        if exclude_via.is_some() && via.as_deref() == exclude_via {
            continue;
        }
        if ev.kind == "file_put" {
            attach_bytes(brain, space, &mut ev)?;
        }
        bytes += ev.payload.to_string().len();
        events.push(ev);
    }
    Ok(Page {
        events,
        next,
        head,
        more: stopped_early || next < head,
    })
}

fn attach_bytes(brain: &Brain, space: &Space, ev: &mut Event) -> Result<()> {
    let sha = ev.payload["sha256"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    match crate::files::read_blob(brain, space, &sha)? {
        Some(b) => {
            ev.payload["data"] = json!(base64::engine::general_purpose::STANDARD.encode(b));
        }
        None => ev.payload["erased"] = json!(true),
    }
    Ok(())
}

/// Facts in dispute: the fact and what was done to it differently (the ids of its replacements, `retired`).
pub fn disputes(conn: &Connection) -> Result<Vec<(String, Vec<String>)>> {
    let mut st = conn.prepare("SELECT fact_id, alt FROM sync_disputes ORDER BY fact_id, alt")?;
    let rows: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (f, a) in rows {
        match out.last_mut() {
            Some((id, alts)) if *id == f => alts.push(a),
            _ => out.push((f, vec![a])),
        }
    }
    Ok(out)
}

// --- applying events -----------------------------------------------------------------------------

enum Step {
    Applied,
    Skipped,
    Disputed,
    /// Logged, without effect, and told to the sender.
    Refused(String),
    /// About something this replica's window left out or pruned: ignored, and not logged (see [`prune_window`]).
    Dropped,
}

/// A problem with one event.
enum Fault {
    /// The event is malformed: refused, not kept.
    Bad(String),
    /// It needs an event that has not arrived: it and the ones after wait.
    Missing(String),
    Fatal(Error),
}

impl From<rusqlite::Error> for Fault {
    fn from(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Fault::Bad(format!("refused by the store: {e}"))
            }
            _ => Fault::Fatal(Error::Sqlite(e)),
        }
    }
}

impl From<Error> for Fault {
    fn from(e: Error) -> Self {
        match e {
            Error::Sqlite(s) => Fault::from(s),
            Error::Invalid(m) => Fault::Bad(m),
            other => Fault::Fatal(other),
        }
    }
}

type Outcome = std::result::Result<Step, Fault>;

fn req<'a>(p: &'a Value, k: &str) -> std::result::Result<&'a str, Fault> {
    p.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Fault::Bad(format!("{k} is required")))
}

fn opt<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

fn plain_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn id_of<'a>(p: &'a Value, k: &str) -> std::result::Result<&'a str, Fault> {
    let s = req(p, k)?;
    if plain_id(s) {
        Ok(s)
    } else {
        Err(Fault::Bad(format!("{k} is not a valid identifier")))
    }
}

/// The eid an event of this kind must carry: it is derived from what changed, so a sender cannot reuse an
/// identifier for something else.
fn expected_eid(ev: &Event) -> std::result::Result<String, Fault> {
    let p = &ev.payload;
    Ok(match ev.kind.as_str() {
        "fact_created" => format!("c:{}", id_of(p, "id")?),
        "fact_superseded" => format!("s:{}:{}", id_of(p, "old")?, id_of(p, "by")?),
        "fact_retired" => format!("r:{}", id_of(p, "id")?),
        "file_put" => format!("f:{}", id_of(p, "id")?),
        "file_retired" => format!("fr:{}", id_of(p, "id")?),
        "layout_changed" => format!("l:{}:{}", req(p, "field")?, ev.hlc),
        other => return Err(Fault::Bad(format!("unknown event kind {other:?}"))),
    })
}

fn add_dispute(conn: &Connection, fact: &str, alt: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO sync_disputes (fact_id, alt, at) VALUES (?1, ?2, ?3)",
        params![fact, alt, util::now()],
    )?;
    Ok(())
}

fn fact_status(conn: &Connection, id: &str) -> rusqlite::Result<Option<(String, Option<String>)>> {
    conn.query_row(
        "SELECT status, replaced_by FROM facts WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

fn exists(conn: &Connection, table: &str, id: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            &format!("SELECT 1 FROM {table} WHERE id = ?1"),
            [id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn apply_created(conn: &Connection, space: &Space, p: &Value, via: &str) -> Outcome {
    let id = id_of(p, "id")?;
    if exists(conn, "facts", id)? {
        return Ok(Step::Skipped);
    }
    let kind = req(p, "kind")?;
    facts::check_kind(kind).map_err(|e| Fault::Bad(e.to_string()))?;
    let body = req(p, "text")?;
    if body.is_empty() {
        return Err(Fault::Bad("text is empty".into()));
    }
    let source_kind = req(p, "source_kind")?;
    let status = match opt(p, "status") {
        Some("proposed") => "proposed",
        _ => "active",
    };
    let replaces = opt(p, "replaces");
    if let Some(r) = replaces {
        if !plain_id(r) {
            return Err(Fault::Bad("replaces is not a valid identifier".into()));
        }
        // A predecessor this replica pruned or left out is not waited for: the link to it is not kept.
        if !exists(conn, "facts", r)? && !gone(conn, "fact", r)? {
            return Err(Fault::Missing(format!("fact {r} has not arrived")));
        }
    }
    let replaces = replaces.filter(|r| exists(conn, "facts", r).unwrap_or(false));
    let tags: Vec<String> = p
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let tags = facts::normalize_tags(&tags).map_err(|e| Fault::Bad(e.to_string()))?;
    // The same text active twice is refused by the store; a copy that comes from another node is kept as it
    // is, with a digest of its own, and the janitor merges duplicates as it does today.
    let mut hash = text::text_hash(body);
    if status == "active" && facts::active_by_hash(conn, &hash)?.is_some() {
        hash = text::text_hash(&format!("{id}\u{0}{body}"));
    }
    let mut source_ref = opt(p, "source_ref").map(str::to_string);
    if let Some(r) = &source_ref
        && status == "active"
        && conn
            .query_row(
                "SELECT 1 FROM facts WHERE source_ref = ?1 AND status = 'active'",
                [r],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
    {
        source_ref = Some(format!("{r}#{id}"));
    }
    let created_at = p
        .get("created_at")
        .and_then(Value::as_i64)
        .ok_or_else(|| Fault::Bad("created_at is required".into()))?;
    let valid_from = p
        .get("valid_from")
        .and_then(Value::as_i64)
        .unwrap_or(created_at);
    conn.execute(
        "INSERT INTO facts (id, space, kind, type, fields, text, text_hash, source_kind, source_path, source_hash,
            source_range, source_ref, source_agent, author, created_at, valid_from, valid_to, replaces, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            id,
            space.as_str(),
            kind,
            opt(p, "type"),
            opt(p, "fields"),
            body,
            hash,
            source_kind,
            opt(p, "source_path"),
            opt(p, "source_hash"),
            opt(p, "source_range"),
            source_ref,
            opt(p, "source_agent"),
            req(p, "author")?,
            created_at,
            valid_from,
            p.get("valid_to").and_then(Value::as_i64),
            replaces,
            status
        ],
    )?;
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO facts_fts (rowid, stem) VALUES (?1, ?2)",
        params![rowid, text::stems(body).join(" ")],
    )?;
    for t in &tags {
        conn.execute(
            "INSERT OR IGNORE INTO fact_tags (fact_id, tag) VALUES (?1, ?2)",
            params![id, t],
        )?;
    }
    facts::event(
        conn,
        id,
        req(p, "author")?,
        "create",
        json!({ "replaces": replaces, "synced_from": via }),
    )
    .map_err(Fault::from)?;
    conn.execute(
        "DELETE FROM window_gone WHERE kind = 'fact' AND id = ?1",
        [id],
    )?;
    Ok(Step::Applied)
}

fn apply_superseded(conn: &Connection, p: &Value, at: i64, via: &str) -> Outcome {
    let (old, by) = (id_of(p, "old")?, id_of(p, "by")?);
    let actor = opt(p, "actor").unwrap_or("peer");
    let Some((status, current)) = fact_status(conn, old)? else {
        if gone(conn, "fact", old)? {
            return Ok(Step::Dropped);
        }
        return Err(Fault::Missing(format!("fact {old} has not arrived")));
    };
    if !exists(conn, "facts", by)? {
        if !gone(conn, "fact", by)? {
            return Err(Fault::Missing(format!("fact {by} has not arrived")));
        }
        // Replaced by a note this replica does not hold: an open fact is closed as retired, with that reason.
        if status == "active" || status == "proposed" {
            let reason = "replaced by a note outside this replica's window";
            conn.execute(
                "UPDATE facts SET status = 'retired', status_at = ?2, status_by = ?3, status_reason = ?4 WHERE id = ?1",
                params![old, at, actor, reason],
            )?;
            facts::event(
                conn,
                old,
                actor,
                "retire",
                json!({ "reason": reason, "synced_from": via }),
            )
            .map_err(Fault::from)?;
            return Ok(Step::Applied);
        }
        return Ok(Step::Dropped);
    }
    let history = |conn: &Connection| {
        facts::event(
            conn,
            old,
            actor,
            "supersede",
            json!({ "by": by, "reason": opt(p, "reason"), "synced_from": via }),
        )
    };
    match status.as_str() {
        "replaced" => {
            let current = current.unwrap_or_default();
            if current == by {
                return Ok(Step::Skipped);
            }
            // Replaced two ways: both replacements stay, the old one shows the smaller id, the same on every node.
            add_dispute(conn, old, &current)?;
            add_dispute(conn, old, by)?;
            if by < current.as_str() {
                conn.execute(
                    "UPDATE facts SET replaced_by = ?2 WHERE id = ?1",
                    params![old, by],
                )?;
            }
            history(conn).map_err(Fault::from)?;
            Ok(Step::Disputed)
        }
        "retired" => {
            // Retired on one node, replaced on another: shown in dispute, the replacement leads.
            add_dispute(conn, old, "retired")?;
            add_dispute(conn, old, by)?;
            conn.execute(
                "UPDATE facts SET status = 'replaced', replaced_by = ?2 WHERE id = ?1",
                params![old, by],
            )?;
            history(conn).map_err(Fault::from)?;
            Ok(Step::Disputed)
        }
        _ => {
            conn.execute(
                "UPDATE facts SET status = 'replaced', replaced_by = ?2, status_at = ?3, status_by = ?4, status_reason = ?5
                 WHERE id = ?1",
                params![old, by, at, actor, opt(p, "reason")],
            )?;
            history(conn).map_err(Fault::from)?;
            Ok(Step::Applied)
        }
    }
}

fn apply_retired(conn: &Connection, p: &Value, at: i64, via: &str) -> Outcome {
    let id = id_of(p, "id")?;
    let actor = opt(p, "actor").unwrap_or("peer");
    let reason = opt(p, "reason").unwrap_or_default();
    let Some((status, current)) = fact_status(conn, id)? else {
        if gone(conn, "fact", id)? {
            return Ok(Step::Dropped);
        }
        return Err(Fault::Missing(format!("fact {id} has not arrived")));
    };
    let history = |conn: &Connection| {
        facts::event(
            conn,
            id,
            actor,
            "retire",
            json!({ "reason": reason, "synced_from": via }),
        )
    };
    match status.as_str() {
        "retired" => Ok(Step::Skipped),
        "replaced" => {
            add_dispute(conn, id, "retired")?;
            if let Some(c) = current {
                add_dispute(conn, id, &c)?;
            }
            let mut st = conn.prepare("SELECT id FROM facts WHERE replaces = ?1")?;
            let others: Vec<String> = st
                .query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for o in others {
                add_dispute(conn, id, &o)?;
            }
            history(conn).map_err(Fault::from)?;
            Ok(Step::Disputed)
        }
        _ => {
            conn.execute(
                "UPDATE facts SET status = 'retired', status_at = ?2, status_by = ?3, status_reason = ?4 WHERE id = ?1",
                params![id, at, actor, reason],
            )?;
            history(conn).map_err(Fault::from)?;
            Ok(Step::Applied)
        }
    }
}

/// `name (origin date).ext`: the name a file gets when another file of a different content has the same one.
fn disputed_name(name: &str, origin: &str, created_at: i64) -> String {
    let tag = format!("{origin} {}", util::compact(created_at));
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 8 => format!("{stem} ({tag}).{ext}"),
        _ => format!("{name} ({tag})"),
    }
}

fn apply_file_put(brain: &Brain, space: &Space, conn: &Connection, ev: &Event) -> Outcome {
    let p = &ev.payload;
    let id = id_of(p, "id")?;
    if exists(conn, "files", id)? {
        return Ok(Step::Skipped);
    }
    let sha = req(p, "sha256")?;
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Fault::Bad("sha256 is not a digest".into()));
    }
    let size = p
        .get("size")
        .and_then(Value::as_i64)
        .ok_or_else(|| Fault::Bad("size is required".into()))?;
    let created_at = p.get("created_at").and_then(Value::as_i64).unwrap_or(ev.at);
    let fact_id = opt(p, "fact_id");
    if let Some(f) = fact_id {
        if !plain_id(f) {
            return Err(Fault::Bad("fact_id is not a valid identifier".into()));
        }
        if !exists(conn, "facts", f)? {
            if gone(conn, "fact", f)? {
                mark_gone(conn, "file", id)?;
                return Ok(Step::Dropped);
            }
            return Err(Fault::Missing(format!("fact {f} has not arrived")));
        }
    }
    if p.get("erased").and_then(Value::as_bool) != Some(true) {
        let data = req(p, "data")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| Fault::Bad("data is not base64".into()))?;
        if hex(&Sha256::digest(&bytes)) != sha || i64::try_from(bytes.len()).ok() != Some(size) {
            return Err(Fault::Bad("the bytes do not match their digest".into()));
        }
        crate::files::store_blob(brain, space, sha, &bytes).map_err(Fault::from)?;
    }
    // The same name with other content: both stay, and the later one by the clock takes the name with its
    // node and time, on every node alike.
    let mut name = req(p, "name")?.to_string();
    let clash: Option<(String, String)> = conn
        .query_row(
            "SELECT id, sha256 FROM files WHERE name = ?1 AND status = 'active' AND sha256 <> ?2 LIMIT 1",
            params![name, sha],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((other, _)) = clash {
        let theirs: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT hlc, origin, at FROM sync_log WHERE eid = ?1",
                [format!("f:{other}")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match theirs {
            Some((hlc, origin, at)) if (hlc.as_str(), other.as_str()) > (ev.hlc.as_str(), id) => {
                conn.execute(
                    "UPDATE files SET name = ?2 WHERE id = ?1",
                    params![other, disputed_name(&name, &origin, at)],
                )?;
            }
            _ => name = disputed_name(&name, &ev.origin, created_at),
        }
    }
    conn.execute(
        "INSERT INTO files (id, name, mime, size, sha256, note, fact_id, author, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            name,
            req(p, "mime")?,
            size,
            sha,
            opt(p, "note").unwrap_or_default(),
            fact_id,
            req(p, "author")?,
            created_at
        ],
    )?;
    if let (Some(f), Some("file")) = (fact_id, opt(p, "fact_type")) {
        conn.execute(
            "UPDATE facts SET type = 'file', fields = ?2 WHERE id = ?1",
            params![f, opt(p, "fact_fields")],
        )?;
    }
    Ok(Step::Applied)
}

fn apply_file_retired(conn: &Connection, p: &Value) -> Outcome {
    let id = id_of(p, "id")?;
    let status: Option<String> = conn
        .query_row("SELECT status FROM files WHERE id = ?1", [id], |r| r.get(0))
        .optional()?;
    match status.as_deref() {
        None if gone(conn, "file", id)? => Ok(Step::Dropped),
        None => Err(Fault::Missing(format!("file {id} has not arrived"))),
        Some("retired") => Ok(Step::Skipped),
        Some(_) => {
            conn.execute(
                "UPDATE files SET status = 'retired', retired_reason = ?2 WHERE id = ?1",
                params![id, opt(p, "reason").unwrap_or_default()],
            )?;
            Ok(Step::Applied)
        }
    }
}

fn apply_layout(brain: &Brain, space: &Space, conn: &Connection, ev: &Event) -> Outcome {
    let field = req(&ev.payload, "field")?;
    if !["label", "parent", "position"].contains(&field) {
        return Err(Fault::Bad("unknown layout field".into()));
    }
    let newest: Option<String> = conn
        .query_row(
            "SELECT hlc FROM sync_fields WHERE field = ?1",
            [field],
            |r| r.get(0),
        )
        .optional()?;
    if newest.as_deref().is_some_and(|h| h >= ev.hlc.as_str()) {
        return Ok(Step::Skipped);
    }
    let value = ev.payload.get("value").cloned().unwrap_or(Value::Null);
    let ctl = brain.open_control().map_err(Fault::from)?;
    match field {
        "label" => {
            let label = match value.as_str() {
                Some(l) => {
                    Some(crate::layout::clean_label(l).map_err(|e| Fault::Bad(e.to_string()))?)
                }
                None => None,
            };
            ctl.execute(
                "UPDATE spaces SET label = ?2 WHERE name = ?1",
                params![space.as_str(), label],
            )?;
        }
        "position" => {
            let n = value
                .as_i64()
                .ok_or_else(|| Fault::Bad("position must be a number".into()))?;
            ctl.execute(
                "UPDATE spaces SET position = ?2 WHERE name = ?1",
                params![space.as_str(), n],
            )?;
        }
        _ => {
            let parent = match value.as_str() {
                None => None,
                Some(p) => Some(Space::new(p).map_err(|e| Fault::Bad(e.to_string()))?),
            };
            if let Some(parent) = &parent {
                let known: Option<i64> = ctl
                    .query_row(
                        "SELECT 1 FROM spaces WHERE name = ?1",
                        [parent.as_str()],
                        |r| r.get(0),
                    )
                    .optional()?;
                if known.is_none() {
                    return Ok(Step::Refused(format!(
                        "parent {parent} is not on this node"
                    )));
                }
                // Two nodes moving two spaces under each other: the later change is refused and reported.
                let mut up = Some(parent.as_str().to_string());
                let mut hops = 0;
                while let Some(cur) = up {
                    if cur == space.as_str() {
                        return Ok(Step::Refused("this move would make a cycle".into()));
                    }
                    hops += 1;
                    if hops > 64 {
                        break;
                    }
                    up = ctl
                        .query_row("SELECT parent FROM spaces WHERE name = ?1", [&cur], |r| {
                            r.get(0)
                        })
                        .optional()?
                        .flatten();
                }
            }
            ctl.execute(
                "UPDATE spaces SET parent = ?2 WHERE name = ?1",
                params![space.as_str(), parent.as_ref().map(Space::as_str)],
            )?;
        }
    }
    conn.execute(
        "INSERT INTO sync_fields (field, hlc) VALUES (?1, ?2) ON CONFLICT(field) DO UPDATE SET hlc = excluded.hlc",
        params![field, ev.hlc],
    )?;
    Ok(Step::Applied)
}

// --- the window --------------------------------------------------------------------------------

fn gone(conn: &Connection, kind: &str, id: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM window_gone WHERE kind = ?1 AND id = ?2",
            params![kind, id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn mark_gone(conn: &Connection, kind: &str, id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO window_gone (kind, id, at) VALUES (?1, ?2, ?3)",
        params![kind, id, util::now()],
    )?;
    Ok(())
}

/// A tag that keeps a note on every replica: `pin`, or anything under `pin/`.
fn is_pin(tag: &str) -> bool {
    tag == "pin" || tag.starts_with("pin/")
}

/// The tags of the boards and guides a space is read through: kept while the note is open, because they are
/// the current state, not history.
fn is_structural(tag: &str) -> bool {
    matches!(tag, "dashboard" | "rules" | "index" | "guide")
        || tag.starts_with("living/")
        || tag.starts_with("living-meta/")
}

/// Whether a note outside the window stays: pinned, or an open board or guide note.
fn stays<S: AsRef<str>>(tags: &[S], open: bool) -> bool {
    tags.iter().any(|t| is_pin(t.as_ref()))
        || (open && tags.iter().any(|t| is_structural(t.as_ref())))
}

/// What a replica that holds a window leaves out of what it receives: a note created before `cutoff` that is
/// not pinned (and not an open board note), a file of such a note or an old file with no note. `None` lets the
/// event through. What is left out is remembered, so what comes later about it is ignored.
fn window_gate(
    conn: &Connection,
    ev: &Event,
    cutoff: i64,
) -> std::result::Result<Option<Step>, Fault> {
    let p = &ev.payload;
    match ev.kind.as_str() {
        "fact_created" => {
            let id = id_of(p, "id")?;
            let Some(created_at) = p.get("created_at").and_then(Value::as_i64) else {
                return Ok(None);
            };
            let tags: Vec<&str> = p
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let open = opt(p, "status") != Some("retired");
            if created_at < cutoff && !exists(conn, "facts", id)? && !stays(&tags, open) {
                mark_gone(conn, "fact", id)?;
                return Ok(Some(Step::Dropped));
            }
        }
        "file_put" => {
            let id = id_of(p, "id")?;
            let created_at = p.get("created_at").and_then(Value::as_i64).unwrap_or(ev.at);
            if opt(p, "fact_id").is_none() && created_at < cutoff && !exists(conn, "files", id)? {
                mark_gone(conn, "file", id)?;
                return Ok(Some(Step::Dropped));
            }
        }
        _ => {}
    }
    Ok(None)
}

fn apply_one(
    brain: &Brain,
    space: &Space,
    conn: &Connection,
    ev: &Event,
    via: &str,
    cutoff: Option<i64>,
) -> Outcome {
    if ev.eid.len() > 200 || expected_eid(ev)? != ev.eid {
        return Err(Fault::Bad(
            "the identifier does not match the change".into(),
        ));
    }
    match parse_hlc(&ev.hlc) {
        None => return Err(Fault::Bad("the clock stamp is malformed".into())),
        Some((ms, _)) if ms > util::now_ms().saturating_add(MAX_DRIFT_MS) => {
            return Err(Fault::Bad(
                "the clock stamp is too far ahead of this node".into(),
            ));
        }
        Some(_) => {}
    }
    if let Some(cutoff) = cutoff
        && let Some(step) = window_gate(conn, ev, cutoff)?
    {
        return Ok(step);
    }
    let p = &ev.payload;
    match ev.kind.as_str() {
        "fact_created" => apply_created(conn, space, p, via),
        "fact_superseded" => apply_superseded(conn, p, ev.at, via),
        "fact_retired" => apply_retired(conn, p, ev.at, via),
        "file_put" => apply_file_put(brain, space, conn, ev),
        "file_retired" => apply_file_retired(conn, p),
        _ => apply_layout(brain, space, conn, ev),
    }
}

/// Applies events received from the peer `via`, in order. An event already known changes nothing; a
/// malformed one is refused and the rest goes on; one that needs an event that has not arrived stops the
/// batch (`stopped`), and the sender sends it again later.
pub fn apply(brain: &Brain, space: &Space, events: &[Event], via: &str) -> Result<Applied> {
    apply_window(brain, space, events, via, None)
}

/// [`apply`] for a replica that holds a window: with `cutoff` (Unix seconds, see [`window_cutoff`]) a note
/// created before it is left out unless it is pinned, and so are its files and what is said about it later.
pub fn apply_window(
    brain: &Brain,
    space: &Space,
    events: &[Event],
    via: &str,
    cutoff: Option<i64>,
) -> Result<Applied> {
    let conn = brain.open_space(space)?;
    let mut out = Applied::default();
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    for ev in events {
        if tx
            .query_row("SELECT 1 FROM sync_log WHERE eid = ?1", [&ev.eid], |_| {
                Ok(())
            })
            .optional()?
            .is_some()
        {
            out.skipped += 1;
            out.consumed += 1;
            continue;
        }
        tx.execute_batch("SAVEPOINT ev")?;
        match apply_one(brain, space, &tx, ev, via, cutoff) {
            Ok(step) => {
                observe(&tx, &ev.hlc)?;
                if !matches!(step, Step::Dropped) {
                    record_raw(
                        &tx,
                        &ev.eid,
                        &ev.hlc,
                        &ev.origin,
                        Some(via),
                        &ev.kind,
                        ev.at,
                        &ev.payload,
                    )?;
                }
                tx.execute_batch("RELEASE ev")?;
                match step {
                    Step::Applied => out.applied += 1,
                    Step::Skipped | Step::Dropped => out.skipped += 1,
                    Step::Disputed => {
                        out.applied += 1;
                        out.disputed += 1;
                    }
                    Step::Refused(why) => out.rejected.push((ev.eid.clone(), why)),
                }
                out.consumed += 1;
            }
            Err(Fault::Bad(why)) => {
                tx.execute_batch("ROLLBACK TO ev; RELEASE ev")?;
                out.rejected.push((ev.eid.clone(), why));
                out.consumed += 1;
            }
            Err(Fault::Missing(why)) => {
                tx.execute_batch("ROLLBACK TO ev; RELEASE ev")?;
                out.stopped = Some(why);
                break;
            }
            Err(Fault::Fatal(e)) => return Err(e),
        }
    }
    tx.commit()?;
    Ok(out)
}

// --- cursors and status --------------------------------------------------------------------------

/// How far this node has read `peer`'s log of `space`, and how far `peer` has read this one's.
pub fn cursors(brain: &Brain, peer: &str, space: &Space) -> Result<(i64, i64)> {
    let ctl = brain.open_control()?;
    Ok(ctl
        .query_row(
            "SELECT pulled, pushed FROM sync_state WHERE peer = ?1 AND space = ?2",
            params![peer, space.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0)))
}

/// Which cursor to move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Pulled,
    Pushed,
}

pub fn set_cursor(brain: &Brain, peer: &str, space: &Space, which: Cursor, to: i64) -> Result<()> {
    let ctl = brain.open_control()?;
    let col = match which {
        Cursor::Pulled => "pulled",
        Cursor::Pushed => "pushed",
    };
    ctl.execute(
        &format!(
            "INSERT INTO sync_state (peer, space, {col}) VALUES (?1, ?2, ?3)
             ON CONFLICT(peer, space) DO UPDATE SET {col} = excluded.{col}"
        ),
        params![peer, space.as_str(), to],
    )?;
    Ok(())
}

/// Notes that `peer` moved events of `space`: the direction as this node sees it (`sending`: its events
/// leave, `receiving`: the peer's arrive), how far it got and how far there is to go.
pub fn note_transfer(
    brain: &Brain,
    peer: &str,
    space: &Space,
    direction: &str,
    done: i64,
    total: i64,
    now: i64,
) -> Result<()> {
    if !["sending", "receiving"].contains(&direction) {
        return Err(Error::Invalid("direction is sending or receiving".into()));
    }
    let total = total.max(0);
    let done = done.clamp(0, total);
    let ctl = brain.open_control()?;
    ctl.execute(
        "INSERT INTO sync_state (peer, space, dir, done, total, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(peer, space) DO UPDATE SET dir = excluded.dir, done = excluded.done,
           total = excluded.total, at = excluded.at",
        params![peer, space.as_str(), direction, done, total, now],
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceProgress {
    pub space: String,
    pub done: i64,
    pub total: i64,
    pub percent: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStatus {
    /// The connection's name, `peer-<device>-xxxx`.
    pub peer: String,
    /// The machine's name as the person gave it.
    pub name: String,
    /// `sending`, `receiving` or `idle`.
    pub direction: String,
    pub spaces: Vec<SpaceProgress>,
    pub last_activity: Option<i64>,
}

/// `peer-macbook-ab12` is the machine `macbook`.
pub fn machine_name(peer: &str) -> String {
    let rest = peer.strip_prefix(PEER_PREFIX).unwrap_or(peer);
    match rest.rsplit_once('-') {
        Some((name, tail)) if !name.is_empty() && tail.len() == 4 => name.to_string(),
        _ => rest.to_string(),
    }
}

/// Every peer: those that hold a connection here and those this node syncs with, what each moved last and when.
pub fn status(brain: &Brain, now: i64) -> Result<Vec<PeerStatus>> {
    let ctl = brain.open_control()?;
    let mut peers: BTreeSet<String> = ctl
        .prepare("SELECT agent FROM connections WHERE agent LIKE 'peer-%' AND revoked_at IS NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let rows: Vec<(String, String, String, i64, i64, i64)> = ctl
        .prepare("SELECT peer, space, dir, done, total, at FROM sync_state ORDER BY peer, space")?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    peers.extend(rows.iter().map(|r| r.0.clone()));
    let mut out = Vec::new();
    for peer in peers {
        let mine: Vec<_> = rows.iter().filter(|r| r.0 == peer && r.5 > 0).collect();
        let latest = mine.iter().max_by_key(|r| r.5);
        let direction = match latest {
            Some(r) if now - r.5 <= ACTIVE_SECS => r.2.clone(),
            _ => "idle".to_string(),
        };
        out.push(PeerStatus {
            name: machine_name(&peer),
            direction,
            last_activity: latest.map(|r| r.5),
            spaces: mine
                .iter()
                .map(|r| SpaceProgress {
                    space: r.1.clone(),
                    done: r.3,
                    total: r.4,
                    percent: if r.4 <= 0 {
                        100
                    } else {
                        (r.3 * 100 / r.4).min(100)
                    },
                })
                .collect(),
            peer,
        });
    }
    Ok(out)
}

impl PeerStatus {
    /// This peer as a direct peer of the directory's union.
    pub fn to_local(&self) -> crate::directory::Local {
        crate::directory::Local {
            connection: self.peer.clone(),
            name: self.name.clone(),
            direction: self.direction.clone(),
            spaces: self.spaces.clone(),
            last_activity: self.last_activity,
            ..Default::default()
        }
    }
}

/// Every peer of the memory: the direct ones (connections held here, peers synced with) with direction and
/// progress, and every node the peer directory knows, each row once (see `directory::union`).
pub fn peers_view(brain: &Brain, now: i64) -> Result<Vec<Value>> {
    let locals = status(brain, now)?
        .iter()
        .map(PeerStatus::to_local)
        .collect();
    crate::directory::union(brain, now, locals)
}

impl PeerStatus {
    pub fn to_json(&self) -> Value {
        json!({
            "peer": self.peer,
            "name": self.name,
            "direction": self.direction,
            "spaces": self.spaces.iter().map(|s| json!({
                "space": s.space, "done": s.done, "total": s.total, "percent": s.percent,
            })).collect::<Vec<_>>(),
            "last_activity": self.last_activity,
        })
    }
}

/// Makes the space on this node when a peer has it and this node does not (the first join is a full copy).
pub fn ensure_space(brain: &Brain, space: &Space, kind: &str) -> Result<()> {
    if brain.has_space(space)? {
        return Ok(());
    }
    brain.add_space(space)?;
    // A kind this node's catalogue does not know stays `custom`.
    let _ = brain.set_space_kind(space, kind);
    Ok(())
}

// --- a replica that holds a window ---------------------------------------------------------------
//
// A phone holds the last N days of a memory, not all of it. The setting is the replica's own (the control
// database), off by default.
//
// - Catch-up leaves out what is older than the window, except what is pinned (see `is_pin`) and the open notes
//   of boards and guides (`is_structural`).
// - [`prune_window`] removes what is older than the window, only once every peer this replica syncs with has
//   it. It is LOCAL: it writes no event and nothing tells a peer. What a peer says later about a note that is
//   gone here (replaced, retired, a file added) is ignored: this replica does not keep the newer state of a
//   note it does not hold, and the peers still hold the whole history. A note replaced by one this replica
//   does not hold, while it still holds the old one (it is pinned), is closed as retired with that reason.
// - Pruning deletes the note, its tags, its history, its search entry, its files (and their bytes) and the log
//   rows about them. A node that joins later is therefore not fed those notes by this replica, only by the
//   ones that hold everything.

const WINDOW_KEY: &str = "window_days";
const WINDOW_LAST_KEY: &str = "window_last_prune";
const DAY: i64 = 86_400;

/// The window of this replica in days, or `None` (the default): it holds everything.
pub fn window_days(brain: &Brain) -> Result<Option<u32>> {
    let ctl = brain.open_control()?;
    Ok(meta_get(&ctl, WINDOW_KEY)?.and_then(|v| v.parse().ok()))
}

/// Sets the window (`Some(days)`, 1 to 36500) or turns it off (`None`). Turning it off does not bring back
/// what was pruned; a node that holds everything does.
pub fn set_window_days(brain: &Brain, days: Option<u32>) -> Result<()> {
    let ctl = brain.open_control()?;
    match days {
        Some(d) if (1..=36_500).contains(&d) => meta_set(&ctl, WINDOW_KEY, &d.to_string()),
        Some(_) => Err(Error::Invalid(
            "a window is 1 to 36500 days; none means everything".into(),
        )),
        None => {
            ctl.execute("DELETE FROM meta WHERE k = ?1", [WINDOW_KEY])?;
            Ok(())
        }
    }
}

/// The instant (Unix seconds) before which a note is outside the window, or `None` without a window.
pub fn window_cutoff(brain: &Brain, now: i64) -> Result<Option<i64>> {
    Ok(window_days(brain)?.map(|d| now.saturating_sub(i64::from(d) * DAY)))
}

/// What one pruning removed or kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub space: String,
    /// Notes removed, whatever their status.
    pub facts: usize,
    pub tags: usize,
    pub files: usize,
    /// Notes outside the window that stay because a peer has not acknowledged them yet.
    pub kept_unacknowledged: usize,
    /// Notes outside the window that stay because they are pinned (or open board notes, or waiting for approval).
    pub kept_pinned: usize,
}

impl Pruned {
    pub fn to_json(&self) -> Value {
        json!({
            "space": self.space, "facts": self.facts, "tags": self.tags, "files": self.files,
            "kept_unacknowledged": self.kept_unacknowledged, "kept_pinned": self.kept_pinned,
        })
    }
}

type LogRows = Vec<(i64, Option<String>)>;

/// Whether every peer of the space holds the log rows named: for each row, the peer sent it here, or this node
/// has pushed past it. No peer, or no row, is never enough.
fn acknowledged(rows: &LogRows, peers: &[(String, i64)]) -> bool {
    !peers.is_empty()
        && !rows.is_empty()
        && rows.iter().all(|(seq, via)| {
            peers
                .iter()
                .all(|(peer, pushed)| via.as_deref() == Some(peer.as_str()) || pushed >= seq)
        })
}

fn log_rows(conn: &Connection, eid: &str, prefix: bool) -> Result<LogRows> {
    let sql = if prefix {
        "SELECT seq, via FROM sync_log WHERE substr(eid, 1, length(?1)) = ?1"
    } else {
        "SELECT seq, via FROM sync_log WHERE eid = ?1"
    };
    Ok(conn
        .prepare(sql)?
        .query_map([eid], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn strings(conn: &Connection, sql: &str, arg: impl rusqlite::ToSql) -> Result<Vec<String>> {
    Ok(conn
        .prepare(sql)?
        .query_map([arg], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Removes from this replica, for good, the notes of `space` created more than `days` days before `now`, with
/// their tags, history, files and log rows, but only those that every peer this replica syncs with has
/// acknowledged, that are not pinned (`pin`, `pin/…`) and that no kept note points to. Open board notes
/// (`dashboard`, `rules`, `index`, `guide`, `living/…`) and notes waiting for approval stay. It writes no
/// event: the pruning stays local.
pub fn prune_window(brain: &Brain, space: &Space, days: u32, now: i64) -> Result<Pruned> {
    let cutoff = now.saturating_sub(i64::from(days) * DAY);
    let conn = brain.open_space(space)?;
    let mut out = Pruned {
        space: space.as_str().to_string(),
        ..Pruned::default()
    };
    // The peers this replica syncs with, and how far it has pushed to each.
    let ctl = brain.open_control()?;
    let mut peers: Vec<(String, i64)> = ctl
        .prepare("SELECT peer, pushed FROM sync_state WHERE space = ?1")?
        .query_map([space.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // A peer that holds a connection here pulls from this node: its progress is its own, so it counts as unknown.
    let blind: i64 = ctl.query_row(
        "SELECT count(*) FROM connections c WHERE c.agent LIKE 'peer-%' AND c.revoked_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM sync_state s WHERE s.peer = c.agent AND s.space = ?1)",
        [space.as_str()],
        |r| r.get(0),
    )?;
    if blind > 0 {
        peers.clear();
    }

    let old: Vec<(String, String)> = conn
        .prepare("SELECT id, status FROM facts WHERE created_at < ?1 ORDER BY created_at, rowid")?
        .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let file_rows = |fid: &str| -> Result<LogRows> {
        let mut rows = log_rows(&conn, &format!("f:{fid}"), false)?;
        rows.extend(log_rows(&conn, &format!("fr:{fid}"), false)?);
        Ok(rows)
    };
    let mut doomed: BTreeSet<String> = BTreeSet::new();
    for (id, status) in &old {
        let tags = strings(&conn, "SELECT tag FROM fact_tags WHERE fact_id = ?1", id)?;
        if status == "proposed" || stays(&tags, status == "active") {
            out.kept_pinned += 1;
            continue;
        }
        let created = log_rows(&conn, &format!("c:{id}"), false)?;
        let mut rows = created.clone();
        rows.extend(log_rows(&conn, &format!("r:{id}"), false)?);
        rows.extend(log_rows(&conn, &format!("s:{id}:"), true)?);
        for fid in strings(&conn, "SELECT id FROM files WHERE fact_id = ?1", id)? {
            rows.extend(file_rows(&fid)?);
        }
        if !created.is_empty() && acknowledged(&rows, &peers) {
            doomed.insert(id.clone());
        } else {
            out.kept_unacknowledged += 1;
        }
    }
    // A note that a kept note was replaced by stays, and so does what that one was replaced by.
    let pointers: Vec<(String, String)> = conn
        .prepare("SELECT id, replaced_by FROM facts WHERE replaced_by IS NOT NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    loop {
        let keep: Vec<&String> = pointers
            .iter()
            .filter(|(id, to)| !doomed.contains(id) && doomed.contains(to))
            .map(|(_, to)| to)
            .collect();
        if keep.is_empty() {
            break;
        }
        for k in keep {
            if doomed.remove(k) {
                out.kept_pinned += 1;
            }
        }
    }
    // Files with no note, older than the window and acknowledged, and the files of the notes that go.
    let mut doomed_files: Vec<String> = Vec::new();
    for fid in strings(
        &conn,
        "SELECT id FROM files WHERE fact_id IS NULL AND created_at < ?1",
        cutoff,
    )? {
        if acknowledged(&file_rows(&fid)?, &peers) {
            doomed_files.push(fid);
        }
    }
    for id in &doomed {
        doomed_files.extend(strings(
            &conn,
            "SELECT id FROM files WHERE fact_id = ?1",
            id,
        )?);
    }
    if doomed.is_empty() && doomed_files.is_empty() {
        return Ok(out);
    }

    let mut blobs: Vec<String> = Vec::new();
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    meta_set(&tx, "window_pruning", "1")?;
    for fid in &doomed_files {
        for eid in [format!("f:{fid}"), format!("fr:{fid}")] {
            tx.execute("DELETE FROM sync_log WHERE eid = ?1", [&eid])?;
        }
        if let Some(sha) = tx
            .query_row("SELECT sha256 FROM files WHERE id = ?1", [fid], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
        {
            blobs.push(sha);
        }
        tx.execute("DELETE FROM files WHERE id = ?1", [fid])?;
        mark_gone(&tx, "file", fid)?;
        out.files += 1;
    }
    for id in &doomed {
        tx.execute(
            "DELETE FROM sync_log WHERE eid IN (?1, ?2)",
            [format!("c:{id}"), format!("r:{id}")],
        )?;
        tx.execute(
            "DELETE FROM sync_log WHERE substr(eid, 1, length(?1)) = ?1",
            [format!("s:{id}:")],
        )?;
        tx.execute("UPDATE facts SET replaces = NULL WHERE replaces = ?1", [id])?;
        tx.execute("DELETE FROM sync_disputes WHERE fact_id = ?1", [id])?;
        tx.execute("DELETE FROM fact_events WHERE fact_id = ?1", [id])?;
        tx.execute("DELETE FROM embeddings WHERE fact_id = ?1", [id])?;
        tx.execute("DELETE FROM fact_origin WHERE fact_id = ?1", [id])?;
        tx.execute("DELETE FROM library_raised WHERE fact_id = ?1", [id])?;
        out.tags += tx.execute("DELETE FROM fact_tags WHERE fact_id = ?1", [id])?;
        let rowid: i64 =
            tx.query_row("SELECT rowid FROM facts WHERE id = ?1", [id], |r| r.get(0))?;
        tx.execute("DELETE FROM facts_fts WHERE rowid = ?1", [rowid])?;
        tx.execute("DELETE FROM facts WHERE id = ?1", [id])?;
        mark_gone(&tx, "fact", id)?;
        out.facts += 1;
    }
    tx.execute("DELETE FROM meta WHERE k = 'window_pruning'", [])?;
    tx.commit()?;
    // The bytes go unless another record of this space still holds them.
    for sha in blobs {
        let held: bool = conn.query_row(
            "SELECT count(*) > 0 FROM files WHERE sha256 = ?1",
            [&sha],
            |r| r.get(0),
        )?;
        if !held {
            crate::files::remove_blob(brain, space, &sha)?;
        }
    }
    Ok(out)
}

/// What the last pruning of this replica did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastPrune {
    pub at: i64,
    pub facts: usize,
    pub files: usize,
    pub kept_unacknowledged: usize,
}

/// The window of this replica and what was pruned last, for the page and the apps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowStatus {
    pub days: Option<u32>,
    pub last_prune: Option<LastPrune>,
}

impl WindowStatus {
    /// `window: N days`, or `window: all` when there is none.
    pub fn label(&self) -> String {
        match self.days {
            Some(d) => format!("window: {d} days"),
            None => "window: all".to_string(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "days": self.days,
            "label": self.label(),
            "last_prune": self.last_prune.as_ref().map(|l| json!({
                "at": l.at, "facts": l.facts, "files": l.files,
                "kept_unacknowledged": l.kept_unacknowledged,
            })),
        })
    }
}

pub fn window_status(brain: &Brain) -> Result<WindowStatus> {
    let ctl = brain.open_control()?;
    let last = meta_get(&ctl, WINDOW_LAST_KEY)?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| {
            let n = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n as usize);
            Some(LastPrune {
                at: v.get("at").and_then(Value::as_i64)?,
                facts: n("facts")?,
                files: n("files")?,
                kept_unacknowledged: n("kept_unacknowledged")?,
            })
        });
    Ok(WindowStatus {
        days: window_days(brain)?,
        last_prune: last,
    })
}

/// Prunes every space of this replica to its window, if it has one, and remembers what was done. Without a
/// window it does nothing and returns `None`.
pub fn prune_all(brain: &Brain, now: i64) -> Result<Option<Vec<Pruned>>> {
    let Some(days) = window_days(brain)? else {
        return Ok(None);
    };
    let mut all = Vec::new();
    for space in brain.spaces()? {
        all.push(prune_window(brain, &space, days, now)?);
    }
    let last = json!({
        "at": now,
        "facts": all.iter().map(|p| p.facts).sum::<usize>(),
        "files": all.iter().map(|p| p.files).sum::<usize>(),
        "kept_unacknowledged": all.iter().map(|p| p.kept_unacknowledged).sum::<usize>(),
    });
    let ctl = brain.open_control()?;
    meta_set(&ctl, WINDOW_LAST_KEY, &last.to_string())?;
    Ok(Some(all))
}
