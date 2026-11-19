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
    /// Event id: derived from what changed, so the same change received twice is one row.
    pub eid: String,
    /// Hybrid logical clock stamp (`<milliseconds>.<counter>.<replica>`, fixed width): orders events across nodes without trusting any clock.
    pub hlc: String,
    /// The node that made the change.
    pub origin: String,
    /// What changed (`fact_created`, `fact_superseded`, `fact_retired`, `file_put`, `file_retired`, `layout`...).
    pub kind: String,
    /// When it happened on the origin (seconds since the epoch).
    pub at: i64,
    /// The change itself, a JSON object whose fields depend on `kind`.
    pub payload: Value,
}

impl Event {
    /// The wire form of the event: an object with `seq`, `eid`, `hlc`, `origin`, `kind`, `at` and `payload`.
    pub fn to_json(&self) -> Value {
        json!({
            "seq": self.seq, "eid": self.eid, "hlc": self.hlc, "origin": self.origin,
            "kind": self.kind, "at": self.at, "payload": self.payload,
        })
    }

    /// Reads the wire form back. A missing `seq` reads as 0 (it is meaningless on the receiver).
    ///
    /// # Errors
    /// [`Error::Invalid`] if `eid`, `hlc`, `origin`, `kind` or `at` is missing, or `payload` is not an object.
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
    /// The events, in log order.
    pub events: Vec<Event>,
    /// The position to ask for next (past events left out because the asker sent them).
    pub next: i64,
    /// The last position of this log.
    pub head: i64,
    /// Whether the log holds more after `next`.
    pub more: bool,
    /// The events left out because the peer takes no chunks (a node of an earlier version) and they are about a file
    /// bigger than travels whole ([`crate::files::MAX_INLINE`]): `(seq, eid)`. They are not held by that peer.
    pub withheld: Vec<(i64, String)>,
}

/// A file whose event waits for its chunks: the receiving node lacks them (see [`crate::chunks`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    /// The file's digest.
    pub file: String,
    /// The digests of all its chunks, in order.
    pub chunks: Vec<String>,
    /// The positions of the chunks missing on the receiving node.
    pub missing: Vec<usize>,
}

impl Wanted {
    /// The wire form: `{file, missing: [{n, hash}]}`.
    pub fn to_json(&self) -> Value {
        json!({
            "file": self.file,
            "missing": self.missing.iter().map(|&n| json!({ "n": n, "hash": self.chunks.get(n) })).collect::<Vec<_>>(),
        })
    }
}

/// What applying events did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Events that changed this replica.
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
    /// The events dealt with but not kept here (refused, or left out of this replica's window), by eid: the sender must
    /// not count them as held by this node.
    pub not_kept: Vec<String>,
    /// What waited too many times for an event that never came and went on without it, in words.
    pub gave_up: Vec<String>,
    /// Set when the batch stopped on a file whose chunks are not all here: the caller fetches them, then applies the
    /// rest again.
    pub wants: Option<Wanted>,
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
    record_file_put_with(conn, id, None)
}

/// [`record_file_put`], with the digests of the chunks of a file too big to travel whole (see `crate::chunks`).
pub(crate) fn record_file_put_with(
    conn: &Connection,
    id: &str,
    chunks: Option<&[String]>,
) -> Result<()> {
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
    if let Some(list) = chunks {
        p["chunk_size"] = json!(crate::chunks::CHUNK_BYTES);
        p["chunks"] = json!(list);
    }
    let at = p["created_at"].as_i64().unwrap_or_else(util::now);
    record(conn, &format!("f:{id}"), "file_put", at, &p)
}

/// The digests of a big file's chunks, for its event: read from the bytes held here (an event logged without them).
fn chunk_list(brain: &Brain, space: &Space, sha: &str) -> Result<Option<Vec<String>>> {
    let path = crate::files::blob_path(brain, space, sha);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(crate::chunks::hashes_of(&path)?))
}

/// A file was retired here; `erased` when its bytes were erased for good (a transcribed recording): every node then
/// erases its copy of them too.
pub(crate) fn record_file_retired(
    conn: &Connection,
    id: &str,
    reason: &str,
    erased: bool,
) -> Result<()> {
    let at = util::now();
    let mut p = json!({ "id": id, "reason": reason, "at": at });
    if erased {
        p["erased"] = json!(true);
    }
    record(conn, &format!("fr:{id}"), "file_retired", at, &p)
}

/// Layout fields of the space changed here, logged in one transaction (the caller's, or one of its own) with the
/// stamp of each field; `token`, when given, is kept in the same transaction, so that the control database's mark of
/// this change is known to be logged ([`layout_logged`]).
pub(crate) fn record_layout_fields(
    conn: &Connection,
    fields: &[(&str, Value)],
    token: Option<&str>,
) -> Result<()> {
    let own = conn.is_autocommit();
    if own {
        conn.execute_batch("BEGIN IMMEDIATE")?;
    }
    let done = (|| -> Result<()> {
        // Logged already by another pass (read under the write lock): once.
        if let Some(t) = token
            && meta_get(conn, "layout_token")?.as_deref() == Some(t)
        {
            return Ok(());
        }
        for (field, value) in fields {
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
        }
        if let Some(t) = token {
            meta_set(conn, "layout_token", t)?;
        }
        Ok(())
    })();
    if own {
        match &done {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }
    done
}

/// Whether the layout change marked `token` in the control database is already in this space's log.
pub(crate) fn layout_logged(conn: &Connection, token: &str) -> Result<bool> {
    Ok(meta_get(conn, "layout_token")?.as_deref() == Some(token))
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
                let reason = reason.unwrap_or_default();
                let erased = crate::files::is_erased_reason(&reason);
                record_file_retired(conn, &id, &reason, erased)?;
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

/// The events of files whose record has none (an earlier version wrote the record and its event apart, and a crash
/// could fall between): `f:<id>` for every record, then `fr:<id>` for a retired one, under the ids the file gives, so
/// that writing them twice writes them once. A record whose describing note was left untyped by that crash is typed
/// first, as the put would have. Answers how many events it wrote. Run when the node starts ([`Brain::recover`]).
pub(crate) fn log_unlogged_files(conn: &Connection) -> Result<usize> {
    let unlogged: Vec<(String, Option<String>)> = conn
        .prepare(
            "SELECT id, fact_id FROM files f
             WHERE NOT EXISTS (SELECT 1 FROM sync_log WHERE eid = 'f:' || f.id) ORDER BY created_at, rowid",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let unretired: Vec<(String, Option<String>)> = conn
        .prepare(
            "SELECT id, retired_reason FROM files f WHERE status = 'retired'
             AND NOT EXISTS (SELECT 1 FROM sync_log WHERE eid = 'fr:' || f.id) ORDER BY created_at, rowid",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if unlogged.is_empty() && unretired.is_empty() {
        return Ok(0);
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let mut n = 0;
    for (id, fact) in &unlogged {
        if tx
            .query_row(
                "SELECT 1 FROM sync_log WHERE eid = ?1",
                [format!("f:{id}")],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            continue;
        }
        if let Some(f) = fact {
            tx.execute(
                "UPDATE facts SET type = 'file', fields = ?2 WHERE id = ?1 AND type IS NULL",
                params![f, json!({ "file_id": id }).to_string()],
            )?;
        }
        record_file_put(&tx, id)?;
        n += 1;
    }
    for (id, reason) in &unretired {
        if tx
            .query_row(
                "SELECT 1 FROM sync_log WHERE eid = ?1",
                [format!("fr:{id}")],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            continue;
        }
        let reason = reason.clone().unwrap_or_default();
        record_file_retired(&tx, id, &reason, crate::files::is_erased_reason(&reason))?;
        n += 1;
    }
    tx.commit()?;
    Ok(n)
}

/// The layout of this space is already in its log: a space made here from a peer's word takes its layout from the
/// peer's events, and must not record it again under a stamp of its own (that later stamp would win over a rename made
/// meanwhile at the origin).
pub(crate) fn mark_layout_logged(conn: &Connection) -> Result<()> {
    meta_set(conn, "sync_layout_logged", "1")
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
    let mut fields = vec![];
    if let Some((label, parent, position)) = row {
        if let Some(l) = label {
            fields.push(("label", json!(l)));
        }
        if let Some(p) = parent {
            fields.push(("parent", json!(p)));
        }
        if position != 0 {
            fields.push(("position", json!(position)));
        }
    }
    // The events and the flag in one transaction: written twice, or never, is not possible.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let done = (|| -> Result<()> {
        if meta_get(conn, "sync_layout_logged")?.is_some() {
            return Ok(());
        }
        record_layout_fields(conn, &fields, None)?;
        meta_set(conn, "sync_layout_logged", "1")
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
/// delivered, for a peer that takes no chunks: a file's bytes travel inside its event, and a file bigger than that
/// allows is left out ([`Page::withheld`]). See [`events_after_with`].
pub fn events_after(
    brain: &Brain,
    space: &Space,
    after: i64,
    limit: usize,
    max_bytes: usize,
    exclude_via: Option<&str>,
) -> Result<Page> {
    events_after_with(brain, space, after, limit, max_bytes, exclude_via, false)
}

/// [`events_after`] for a peer that takes chunks when `chunks` is true: a file up to [`crate::files::MAX_INLINE`]
/// travels inside its event, a bigger one as the list of its chunks (`chunk_size`, `chunks`), fetched apart.
pub fn events_after_with(
    brain: &Brain,
    space: &Space,
    after: i64,
    limit: usize,
    max_bytes: usize,
    exclude_via: Option<&str>,
    chunks: bool,
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
    let mut withheld = Vec::new();
    let mut next = after.max(0);
    let mut bytes = 0usize;
    let mut stopped_early = false;
    for row in rows {
        let (mut ev, via) = row?;
        if bytes >= max_bytes && !events.is_empty() {
            stopped_early = true;
            break;
        }
        if exclude_via.is_some() && via.as_deref() == exclude_via {
            next = ev.seq;
            continue;
        }
        if ev.kind == "file_put" && !attach_bytes(brain, space, &conn, &mut ev, chunks)? {
            next = ev.seq;
            withheld.push((ev.seq, ev.eid));
            continue;
        }
        // The retirement of a big file a peer without chunks never got goes with it, unless it erased the bytes: then
        // the file's record travels without them, to every node.
        if ev.kind == "file_retired"
            && !chunks
            && ev.payload.get("erased").and_then(Value::as_bool) != Some(true)
            && big_file(&conn, ev.payload["id"].as_str().unwrap_or_default())?
        {
            next = ev.seq;
            withheld.push((ev.seq, ev.eid));
            continue;
        }
        // An event that would take the page past its size goes first in the next one: a page with a file stays under
        // what a node reads in one request (see `crate::files::MAX_BYTES`).
        let size = ev.payload.to_string().len();
        if !events.is_empty() && bytes + size > max_bytes {
            stopped_early = true;
            break;
        }
        next = ev.seq;
        bytes += size;
        events.push(ev);
    }
    Ok(Page {
        events,
        next,
        head,
        more: stopped_early || next < head,
        withheld,
    })
}

/// Whether the file is bigger than travels whole.
fn big_file(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT size FROM files WHERE id = ?1", [id], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .is_some_and(|n| n > crate::files::MAX_INLINE as i64))
}

/// The bytes of a file travel inside its event, unless they are gone: erased here (a transcribed recording, never sent
/// again from any node), or not on this node; then the record travels alone, marked `erased`. A file bigger than
/// [`crate::files::MAX_INLINE`] travels as the list of its chunks to a peer that takes them (`chunks`), and not at all
/// to one that does not: `false` then, the event is left out.
fn attach_bytes(
    brain: &Brain,
    space: &Space,
    conn: &Connection,
    ev: &mut Event,
    chunks: bool,
) -> Result<bool> {
    let sha = ev.payload["sha256"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let id = ev.payload["id"].as_str().unwrap_or_default().to_string();
    let erased = conn
        .query_row(
            "SELECT retired_reason FROM files WHERE id = ?1",
            [&id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .is_some_and(|r| crate::files::is_erased_reason(&r));
    let big = ev.payload["size"]
        .as_i64()
        .is_some_and(|n| n > crate::files::MAX_INLINE as i64);
    // A log row written by an earlier version may still hold the bytes it received: they are never sent from there.
    let listed = ev.payload.get("chunks").cloned();
    if let Some(o) = ev.payload.as_object_mut() {
        o.remove("data");
        o.remove("erased");
        o.remove("chunks");
        o.remove("chunk_size");
    }
    if erased {
        ev.payload["erased"] = json!(true);
        return Ok(true);
    }
    if big {
        if !chunks {
            return Ok(false);
        }
        let list = match listed.filter(Value::is_array) {
            Some(l) => Some(l),
            None => chunk_list(brain, space, &sha)?.map(|l| json!(l)),
        };
        match list {
            Some(l) => {
                ev.payload["chunk_size"] = json!(crate::chunks::CHUNK_BYTES);
                ev.payload["chunks"] = l;
            }
            None => ev.payload["erased"] = json!(true),
        }
        return Ok(true);
    }
    match crate::files::read_blob(brain, space, &sha)? {
        Some(b) => {
            ev.payload["data"] = json!(base64::engine::general_purpose::STANDARD.encode(b));
        }
        None => ev.payload["erased"] = json!(true),
    }
    Ok(true)
}

/// Facts in dispute: the fact and what was done to it differently (the ids of its replacements, `retired`). Only the
/// disputes that still hold: at least two alternatives still stand.
pub fn disputes(conn: &Connection) -> Result<Vec<(String, Vec<String>)>> {
    Ok(open_disputes(conn)?
        .into_iter()
        .map(|d| (d.fact, d.alternatives.into_iter().map(|a| a.id).collect()))
        .collect())
}

/// The reason written on an alternative that lost when a person resolved a dispute, before the alternative kept.
pub const RESOLVED_REASON: &str = "dispute resolved: kept";

/// One way a fact in dispute was changed on some node: a replacement, or its retirement.
#[derive(Clone, Debug, PartialEq)]
pub struct Alternative {
    /// The id of the replacement, or `retired`.
    pub id: String,
    /// The newest version of that replacement when it was corrected since; `None` when it is the replacement itself or
    /// for `retired`.
    pub current: Option<String>,
    /// The text of the newest version (`None` for `retired`, or a replacement this replica does not hold).
    pub text: Option<String>,
    /// Who wrote the replacement, or who retired the fact.
    pub author: Option<String>,
    /// When (seconds since the epoch).
    pub at: Option<i64>,
    /// The reason given for the retirement.
    pub reason: Option<String>,
}

impl Alternative {
    /// `{id, kind: "replacement" | "retired", current, text, author, at, reason}`, dates in ISO 8601.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": if self.id == "retired" { "retired" } else { "replacement" },
            "current": self.current,
            "text": self.text,
            "author": self.author,
            "at": self.at.map(util::iso),
            "reason": self.reason,
        })
    }
}

/// A fact two nodes changed differently, and the alternatives that still stand.
#[derive(Clone, Debug, PartialEq)]
pub struct Dispute {
    /// The fact both nodes changed.
    pub fact: String,
    /// When this node first saw the dispute.
    pub since: i64,
    /// At least two; ordered by id, `retired` among them.
    pub alternatives: Vec<Alternative>,
}

/// The newest version of a fact along its replacements, and its status. `None` when this replica does not hold it.
fn chain_head(conn: &Connection, id: &str) -> rusqlite::Result<Option<(String, String)>> {
    let mut cur = id.to_string();
    for _ in 0..1000 {
        let Some((status, by)) = fact_status(conn, &cur)? else {
            return Ok(None);
        };
        match by {
            Some(next) if status == "replaced" && exists(conn, "facts", &next)? => cur = next,
            _ => return Ok(Some((cur, status))),
        }
    }
    Ok(None)
}

/// Whether an alternative still stands: the retirement always does (only a resolution sets it aside); a replacement
/// while the newest version of it is active or proposed. A replacement this replica does not hold is counted as
/// standing: a dispute is never hidden for want of a note.
fn stands(conn: &Connection, alt: &str) -> rusqlite::Result<bool> {
    if alt == "retired" {
        return Ok(true);
    }
    Ok(match chain_head(conn, alt)? {
        None => true,
        Some((_, status)) => status == "active" || status == "proposed",
    })
}

/// The marks of each fact in dispute: the alternatives and when each was first seen here.
type DisputeRows = Vec<(String, Vec<(String, i64)>)>;

fn dispute_rows(conn: &Connection) -> rusqlite::Result<DisputeRows> {
    let mut st =
        conn.prepare("SELECT fact_id, alt, at FROM sync_disputes ORDER BY fact_id, alt")?;
    let rows: Vec<(String, String, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out: DisputeRows = Vec::new();
    for (f, a, at) in rows {
        match out.last_mut() {
            Some((id, alts)) if *id == f => alts.push((a, at)),
            _ => out.push((f, vec![(a, at)])),
        }
    }
    Ok(out)
}

/// The disputes that hold here, each with what its alternatives say. Read-only.
pub fn open_disputes(conn: &Connection) -> Result<Vec<Dispute>> {
    let mut out = Vec::new();
    for (fact, rows) in dispute_rows(conn)? {
        let mut alternatives = Vec::new();
        for (alt, _) in &rows {
            if stands(conn, alt)? {
                alternatives.push(describe(conn, &fact, alt)?);
            }
        }
        if alternatives.len() >= 2 {
            let since = rows.iter().map(|(_, at)| *at).min().unwrap_or(0);
            out.push(Dispute {
                fact,
                since,
                alternatives,
            });
        }
    }
    Ok(out)
}

fn describe(conn: &Connection, fact: &str, alt: &str) -> Result<Alternative> {
    if alt == "retired" {
        // Who retired it and why: the retirement as the log keeps it.
        let p: Option<String> = conn
            .query_row(
                "SELECT payload FROM sync_log WHERE eid = ?1",
                [format!("r:{fact}")],
                |r| r.get(0),
            )
            .optional()?;
        let p: Value = p
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        return Ok(Alternative {
            id: alt.into(),
            current: None,
            text: None,
            author: p["actor"].as_str().map(str::to_string),
            at: p["at"].as_i64(),
            reason: p["reason"].as_str().map(str::to_string),
        });
    }
    let head = chain_head(conn, alt)?.map(|(h, _)| h);
    let shown = head.clone().unwrap_or_else(|| alt.to_string());
    let row: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT text, author, created_at FROM facts WHERE id = ?1",
            [&shown],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(Alternative {
        id: alt.into(),
        current: head.filter(|h| h != alt),
        text: row.as_ref().map(|r| r.0.clone()),
        author: row.as_ref().map(|r| r.1.clone()),
        at: row.map(|r| r.2),
        reason: None,
    })
}

/// The rule that ends a dispute, the same on every node because it reads only what the events left: an alternative
/// that no longer stands is set aside, and a fact with at most one alternative left is no longer in dispute. A fact
/// replaced several ways shows the smallest id among the replacements that stand. Applied after each change to a fact
/// that came from a peer or was made here, and after a resolution. Nothing is erased but these marks.
pub(crate) fn settle_disputes(conn: &Connection) -> rusqlite::Result<()> {
    for (fact, rows) in dispute_rows(conn)? {
        let mut standing: Vec<String> = Vec::new();
        for (alt, _) in &rows {
            if stands(conn, alt)? {
                standing.push(alt.clone());
            } else {
                conn.execute(
                    "DELETE FROM sync_disputes WHERE fact_id = ?1 AND alt = ?2",
                    params![fact, alt],
                )?;
            }
        }
        if standing.len() <= 1 {
            conn.execute("DELETE FROM sync_disputes WHERE fact_id = ?1", [&fact])?;
        }
        let mut held: Vec<&String> = Vec::new();
        for a in standing.iter().filter(|a| *a != "retired") {
            if exists(conn, "facts", a)? {
                held.push(a);
            }
        }
        if let (Some((status, shown)), Some(first)) = (fact_status(conn, &fact)?, held.first())
            && status == "replaced"
            && !shown.as_ref().is_some_and(|s| held.contains(&s))
        {
            conn.execute(
                "UPDATE facts SET replaced_by = ?2 WHERE id = ?1",
                params![fact, first],
            )?;
        }
    }
    Ok(())
}

/// What a resolution did.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    /// The fact that was in dispute.
    pub fact: String,
    /// The alternative kept: a replacement's id, or `retired`.
    pub kept: String,
    /// The versions retired because they lost.
    pub retired: Vec<String>,
}

/// Resolves the dispute on `fact` by keeping one alternative (`keep`: the id of a replacement or of its newest
/// version, or `retired`). Each other replacement that stands is retired, with the reason « dispute resolved: kept
/// … », and a `dispute_resolved` event is logged: ordinary events, so every node reaches the same state through the
/// log. Nothing is erased. Who may do it is the caller's business (an act of the person).
///
/// # Errors
/// [`Error::Invalid`] if `fact` is not in dispute here, or `keep` is not one of its alternatives that stand.
pub fn resolve(conn: &Connection, fact: &str, keep: &str, actor: &str) -> Result<Resolved> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let Some(d) = open_disputes(&tx)?.into_iter().find(|d| d.fact == fact) else {
        return Err(Error::Invalid(format!("fact {fact} is not in dispute")));
    };
    let Some(kept) = d
        .alternatives
        .iter()
        .find(|a| a.id == keep || a.current.as_deref() == Some(keep))
        .map(|a| a.id.clone())
    else {
        let ids: Vec<&str> = d.alternatives.iter().map(|a| a.id.as_str()).collect();
        return Err(Error::Invalid(format!(
            "{keep} is not an alternative of this dispute: keep one of {}",
            ids.join(", ")
        )));
    };
    let reason = format!("{RESOLVED_REASON} {kept}");
    let at = util::now();
    let mut retired = Vec::new();
    for a in d
        .alternatives
        .iter()
        .filter(|a| a.id != kept && a.id != "retired")
    {
        let Some((head, status)) = chain_head(&tx, &a.id)? else {
            continue;
        };
        if status != "active" && status != "proposed" {
            continue;
        }
        tx.execute(
            "UPDATE facts SET status = 'retired', status_at = ?2, status_by = ?3, status_reason = ?4 WHERE id = ?1",
            params![head, at, actor, reason],
        )?;
        facts::event(&tx, &head, actor, "retire", json!({ "reason": reason }))?;
        record_fact_retired(&tx, &head, actor, &reason)?;
        retired.push(head);
    }
    let hlc = stamp(&tx)?;
    let origin = replica(&tx)?;
    let p = json!({ "fact": fact, "kept": kept, "actor": actor, "at": at });
    record_raw(
        &tx,
        &format!("d:{fact}:{hlc}"),
        &hlc,
        &origin,
        None,
        "dispute_resolved",
        at,
        &p,
    )?;
    resolution(&tx, fact, &kept, actor, None)?;
    tx.commit()?;
    Ok(Resolved {
        fact: fact.into(),
        kept,
        retired,
    })
}

/// What a resolution changes beyond the retirements it comes with, here or on a node that receives it: keeping a
/// replacement sets the retirement aside; the history of the fact says it; then the rule of [`settle_disputes`].
fn resolution(
    conn: &Connection,
    fact: &str,
    kept: &str,
    actor: &str,
    via: Option<&str>,
) -> Result<()> {
    if kept != "retired" {
        conn.execute(
            "DELETE FROM sync_disputes WHERE fact_id = ?1 AND alt = 'retired'",
            [fact],
        )?;
    }
    // The history of the fact says it, as a repointing: the fact now leads to the alternative kept.
    facts::event(
        conn,
        fact,
        actor,
        "repoint",
        json!({ "dispute_resolved": { "kept": kept }, "synced_from": via }),
    )?;
    settle_disputes(conn)?;
    Ok(())
}

/// Each fact that takes part in a dispute here (the fact in dispute, one of its replacements that stand, or their newest
/// version), with the fact in dispute and the ids of its alternatives.
pub fn dispute_marks(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, (String, Vec<String>)>> {
    let mut out = std::collections::HashMap::new();
    for d in open_disputes(conn)? {
        let alts: Vec<String> = d.alternatives.iter().map(|a| a.id.clone()).collect();
        out.insert(d.fact.clone(), (d.fact.clone(), alts.clone()));
        for a in &d.alternatives {
            if a.id != "retired" {
                out.insert(a.id.clone(), (d.fact.clone(), alts.clone()));
            }
            if let Some(c) = &a.current {
                out.insert(c.clone(), (d.fact.clone(), alts.clone()));
            }
        }
    }
    Ok(out)
}

/// How many disputes hold in a space.
pub fn dispute_count(conn: &Connection) -> Result<usize> {
    let any: bool = conn
        .query_row("SELECT 1 FROM sync_disputes LIMIT 1", [], |_| Ok(()))
        .optional()?
        .is_some();
    Ok(if any { open_disputes(conn)?.len() } else { 0 })
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
    /// A layout change that names a parent space this node does not hold yet: kept aside, not logged, and tried again
    /// at each exchange until it applies (see `sync_deferred`).
    Deferred,
    /// A recording erased at its origin: the record says so here too, and these bytes go once the change is committed.
    Erased(String),
}

/// A problem with one event.
enum Fault {
    /// The event is malformed: refused, not kept.
    Bad(String),
    /// It needs an event that has not arrived (a `fact` or a `file`, by id): it and the ones after wait.
    Missing {
        kind: &'static str,
        id: String,
    },
    /// A big file whose chunks are not all here: it and the ones after wait until they are fetched.
    Chunks(Wanted),
    Fatal(Error),
}

fn missing(kind: &'static str, id: &str) -> Fault {
    Fault::Missing {
        kind,
        id: id.to_string(),
    }
}

/// The eid of the event that makes a `fact` or a `file`.
fn creation_eid(kind: &str, id: &str) -> String {
    if kind == "file" {
        format!("f:{id}")
    } else {
        format!("c:{id}")
    }
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
        "dispute_resolved" => format!("d:{}:{}", id_of(p, "fact")?, ev.hlc),
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
    facts::check_kind(Some(kind)).map_err(|e| Fault::Bad(e.to_string()))?;
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
            return Err(missing("fact", r));
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
        return Err(missing("fact", old));
    };
    if !exists(conn, "facts", by)? {
        if !gone(conn, "fact", by)? {
            return Err(missing("fact", by));
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
        return Err(missing("fact", id));
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
            return Err(missing("fact", f));
        }
    }
    // The name is cleaned as it is when an assistant puts a file: no path, no control character.
    let clean = crate::files::clean_name(req(p, "name")?);
    if clean.is_empty() {
        return Err(Fault::Bad("a file needs a name".into()));
    }
    let chunked = p.get("chunks").filter(|c| c.is_array());
    let mime = if p.get("erased").and_then(Value::as_bool) == Some(true) {
        // Erased at the origin: any chunks of it kept aside here go.
        crate::chunks::drop_staged(brain, space, sha);
        let claimed = req(p, "mime")?;
        if crate::files::is_kept_type(claimed) {
            claimed.to_string()
        } else {
            "application/octet-stream".into()
        }
    } else if let Some(list) = chunked {
        if p.get("chunk_size").and_then(Value::as_u64) != Some(crate::chunks::CHUNK_BYTES as u64) {
            return Err(Fault::Bad(
                "the chunks are not of the size this node takes".into(),
            ));
        }
        if size <= 0 || size > crate::files::MAX_BYTES as i64 {
            return Err(Fault::Bad("the file is bigger than a space keeps".into()));
        }
        let hashes: Vec<String> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                h.as_str()
                    .filter(|h| crate::chunks::is_digest(h))
                    .map(String::from)
            })
            .collect();
        if hashes.len() != list.as_array().map_or(0, Vec::len)
            || hashes.len() != crate::chunks::count(size.unsigned_abs())
        {
            return Err(Fault::Bad(
                "the list of chunks does not fit the size".into(),
            ));
        }
        if !crate::files::hold_or_link(brain, space, sha, size.unsigned_abs())
            .map_err(Fault::from)?
            && !crate::chunks::assemble(brain, space, sha, size.unsigned_abs(), &hashes)
                .map_err(Fault::from)?
        {
            let missing = crate::chunks::missing(brain, space, sha, &hashes);
            crate::chunks::want(brain, space, sha).map_err(Fault::from)?;
            return Err(Fault::Chunks(Wanted {
                file: sha.to_string(),
                chunks: hashes,
                missing,
            }));
        }
        // The type is what the bytes say, read from the first of them.
        let mut head = vec![0u8; 64 * 1024];
        let n = std::io::Read::read(
            &mut std::fs::File::open(crate::files::blob_path(brain, space, sha))
                .map_err(|e| Fault::Fatal(e.into()))?,
            &mut head,
        )
        .map_err(|e| Fault::Fatal(e.into()))?;
        head.truncate(n);
        match crate::files::sniff_head(&head, &clean) {
            Some(m) => m.to_string(),
            None => {
                let _ = crate::files::remove_blob(brain, space, sha);
                return Err(Fault::Bad("this kind of file is not kept".into()));
            }
        }
    } else {
        let data = req(p, "data")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| Fault::Bad("data is not base64".into()))?;
        if hex(&Sha256::digest(&bytes)) != sha || i64::try_from(bytes.len()).ok() != Some(size) {
            return Err(Fault::Bad("the bytes do not match their digest".into()));
        }
        // The type is what the bytes say, never what the sender claims; a kind that is not kept is refused.
        let sniffed = crate::files::sniff(&bytes, &clean)
            .ok_or_else(|| Fault::Bad("this kind of file is not kept".into()))?;
        crate::files::store_blob(brain, space, sha, &bytes).map_err(Fault::from)?;
        sniffed.to_string()
    };
    // The same name with other content: both stay, and the later one by the clock takes the name with its
    // node and time, on every node alike.
    let mut name = clean;
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
            mime,
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
    let reason = opt(p, "reason").unwrap_or_default();
    // Erased at the origin: the flag, or the reason a node of an earlier version gives.
    let erase = p.get("erased").and_then(Value::as_bool) == Some(true)
        || crate::files::is_erased_reason(reason);
    let row: Option<(String, Option<String>, String)> = conn
        .query_row(
            "SELECT status, retired_reason, sha256 FROM files WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((status, was, sha)) = row else {
        if gone(conn, "file", id)? {
            return Ok(Step::Dropped);
        }
        return Err(missing("file", id));
    };
    let already_erased = was.as_deref().is_some_and(crate::files::is_erased_reason);
    if status == "retired" && (!erase || already_erased) {
        return Ok(Step::Skipped);
    }
    conn.execute(
        "UPDATE files SET status = 'retired', retired_reason = ?2 WHERE id = ?1",
        params![id, reason],
    )?;
    Ok(if erase {
        Step::Erased(sha)
    } else {
        Step::Applied
    })
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
                // The parent is made here by a later exchange (or brought back): the move waits for it, aside.
                if known.is_none() {
                    return Ok(Step::Deferred);
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

/// The biggest text of one fact a peer may send (the biggest any node writes), and how many tags it may carry.
const MAX_EVENT_TEXT: usize = crate::facts::MAX_TEXT_BYTES;
const MAX_EVENT_TAGS: usize = 64;

/// What every event must look like before anything is read from it: short plain identifiers for who and when, and
/// bounded text, so that a peer cannot fill the store or put control characters where a name is shown.
fn check_shape(ev: &Event) -> std::result::Result<(), Fault> {
    if !plain_id(&ev.origin) {
        return Err(Fault::Bad("the origin is not a plain identifier".into()));
    }
    let replica = ev.hlc.rsplit('.').next().unwrap_or("");
    if !plain_id(replica) {
        return Err(Fault::Bad("the clock stamp is malformed".into()));
    }
    let p = &ev.payload;
    let small = |k: &str, max: usize| match opt(p, k) {
        Some(v) if v.chars().count() > max || v.chars().any(|c| c == '\0') => {
            Err(Fault::Bad(format!("{k} is too long")))
        }
        _ => Ok(()),
    };
    for k in ["author", "actor", "source_agent", "source_kind", "type"] {
        small(k, 200)?;
    }
    for k in [
        "source_path",
        "source_hash",
        "source_range",
        "source_ref",
        "reason",
        "note",
    ] {
        small(k, 4096)?;
    }
    small("fields", 64 * 1024)?;
    if opt(p, "text").is_some_and(|t| t.len() > MAX_EVENT_TEXT) {
        return Err(Fault::Bad("text is too long".into()));
    }
    if p.get("tags")
        .and_then(Value::as_array)
        .is_some_and(|t| t.len() > MAX_EVENT_TAGS)
    {
        return Err(Fault::Bad("too many tags".into()));
    }
    Ok(())
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
    check_shape(ev)?;
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
    let step = match ev.kind.as_str() {
        "fact_created" => apply_created(conn, space, p, via),
        "fact_superseded" => apply_superseded(conn, p, ev.at, via),
        "fact_retired" => apply_retired(conn, p, ev.at, via),
        "dispute_resolved" => apply_resolved(conn, p, via),
        "file_put" => apply_file_put(brain, space, conn, ev),
        "file_retired" => apply_file_retired(conn, p),
        _ => apply_layout(brain, space, conn, ev),
    }?;
    // A dispute ends by the same rule on every node, as soon as the events that end it are here.
    if matches!(ev.kind.as_str(), "fact_superseded" | "fact_retired")
        && matches!(step, Step::Applied | Step::Disputed)
    {
        settle_disputes(conn)?;
    }
    Ok(step)
}

/// A person resolved a dispute on another node: the retirements came before it; here the retirement it set aside
/// and the rule that ends the dispute.
fn apply_resolved(conn: &Connection, p: &Value, via: &str) -> Outcome {
    let fact = id_of(p, "fact")?;
    let kept = id_of(p, "kept")?;
    let actor = opt(p, "actor").unwrap_or("peer");
    if !exists(conn, "facts", fact)? {
        if gone(conn, "fact", fact)? {
            return Ok(Step::Dropped);
        }
        return Err(missing("fact", fact));
    }
    resolution(conn, fact, kept, actor, Some(via))?;
    Ok(Step::Applied)
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
    let mut erased: Vec<String> = Vec::new();
    // The bytes of a received file are written before its record commits: the sweep of a starting node waits.
    let writing = crate::recover::writing(brain)?;
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    retry_deferred(brain, space, &tx, &mut erased)?;
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
        let mut outcome = apply_one(brain, space, &tx, ev, via, cutoff);
        if let Err(Fault::Missing { kind, id }) = &outcome {
            let (kind, id) = (*kind, id.clone());
            let needs = creation_eid(kind, &id);
            let refused: Option<String> = tx
                .query_row(
                    "SELECT why FROM sync_rejected WHERE eid = ?1",
                    [&needs],
                    |r| r.get(0),
                )
                .optional()?;
            let waited: i64 = tx
                .query_row(
                    "SELECT stops FROM sync_waiting WHERE needs = ?1",
                    [&needs],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            if let Some(why) = refused {
                // What it needs was refused here: it can never apply, so it is refused too instead of waiting for ever.
                outcome = Err(Fault::Bad(format!(
                    "it needs {kind} {id}, which this node refused ({why})"
                )));
            } else if waited >= MAX_WAITS {
                // It waited at every exchange for an event no peer has (pruned or left out where it comes from): it
                // goes on as if that note or file were outside this replica's window, and the pass says so.
                tx.execute_batch("ROLLBACK TO ev")?;
                mark_gone(&tx, kind, &id)?;
                tx.execute("DELETE FROM sync_waiting WHERE needs = ?1", [&needs])?;
                out.gave_up.push(format!(
                    "{kind} {id} never arrived after {MAX_WAITS} exchanges: what needed it went on without it"
                ));
                outcome = apply_one(brain, space, &tx, ev, via, cutoff);
            }
        }
        match outcome {
            Ok(step) => {
                observe(&tx, &ev.hlc)?;
                match &step {
                    Step::Dropped => {}
                    Step::Deferred => {
                        tx.execute(
                            "INSERT INTO sync_deferred (eid, via, event, at) VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT(eid) DO UPDATE SET via = excluded.via, event = excluded.event",
                            params![ev.eid, via, ev.to_json().to_string(), util::now()],
                        )?;
                    }
                    _ => {
                        record_raw(
                            &tx,
                            &ev.eid,
                            &ev.hlc,
                            &ev.origin,
                            Some(via),
                            &ev.kind,
                            ev.at,
                            &logged(&ev.payload),
                        )?;
                    }
                }
                // Arrived after all: nothing waits for it, and an earlier refusal of it no longer holds.
                tx.execute("DELETE FROM sync_rejected WHERE eid = ?1", [&ev.eid])?;
                tx.execute("DELETE FROM sync_waiting WHERE needs = ?1", [&ev.eid])?;
                tx.execute_batch("RELEASE ev")?;
                match step {
                    Step::Applied => out.applied += 1,
                    Step::Erased(sha) => {
                        out.applied += 1;
                        erased.push(sha);
                    }
                    Step::Skipped | Step::Deferred => out.skipped += 1,
                    Step::Dropped => {
                        out.skipped += 1;
                        out.not_kept.push(ev.eid.clone());
                    }
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
                // Remembered only when the eid is the one its content gives (a malformed event cannot name another), and
                // not for a stamp ahead of this node's clock: that says nothing of the change, which may come again.
                let ahead = parse_hlc(&ev.hlc)
                    .is_some_and(|(ms, _)| ms > util::now_ms().saturating_add(MAX_DRIFT_MS));
                if !ahead && expected_eid(ev).is_ok_and(|e| e == ev.eid) {
                    tx.execute(
                        "INSERT INTO sync_rejected (eid, why, at) VALUES (?1, ?2, ?3)
                         ON CONFLICT(eid) DO UPDATE SET why = excluded.why, at = excluded.at",
                        params![ev.eid, why, util::now()],
                    )?;
                }
                out.rejected.push((ev.eid.clone(), why));
                out.not_kept.push(ev.eid.clone());
                out.consumed += 1;
            }
            Err(Fault::Missing { kind, id }) => {
                tx.execute_batch("ROLLBACK TO ev; RELEASE ev")?;
                tx.execute(
                    "INSERT INTO sync_waiting (needs, stops, since) VALUES (?1, 1, ?2)
                     ON CONFLICT(needs) DO UPDATE SET stops = stops + 1",
                    params![creation_eid(kind, &id), util::now()],
                )?;
                out.stopped = Some(format!("{kind} {id} has not arrived"));
                break;
            }
            Err(Fault::Chunks(w)) => {
                tx.execute_batch("ROLLBACK TO ev; RELEASE ev")?;
                out.stopped = Some(format!(
                    "file {} waits for {} of its {} chunks",
                    w.file,
                    w.missing.len(),
                    w.chunks.len()
                ));
                out.wants = Some(w);
                break;
            }
            Err(Fault::Fatal(e)) => return Err(e),
        }
    }
    tx.commit()?;
    drop(writing);
    crate::failpoint::hit(crate::failpoint::at::SYNC_APPLY_BYTES)?;
    remove_erased(brain, space, &conn, &erased)?;
    Ok(out)
}

/// A received event as the log keeps it: without the bytes of a file, which live once, in the space's files, and are
/// read from there when the event is sent on (an erased recording then travels without them).
fn logged(payload: &Value) -> Value {
    let mut p = payload.clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("data");
        o.remove("erased");
    }
    p
}

/// How many exchanges an event may stop on one that has not arrived before it goes on without it.
const MAX_WAITS: i64 = 10;

/// The layout changes kept aside because their parent space was not here: each is tried again, logged once it applies,
/// and dropped once it is refused. In the caller's transaction.
fn retry_deferred(
    brain: &Brain,
    space: &Space,
    tx: &Connection,
    erased: &mut Vec<String>,
) -> Result<()> {
    let rows: Vec<(String, String, String)> = tx
        .prepare("SELECT eid, via, event FROM sync_deferred ORDER BY at, eid")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (eid, via, event) in rows {
        let Some(ev) = serde_json::from_str::<Value>(&event)
            .ok()
            .and_then(|v| Event::from_json(&v).ok())
        else {
            tx.execute("DELETE FROM sync_deferred WHERE eid = ?1", [&eid])?;
            continue;
        };
        tx.execute_batch("SAVEPOINT deferred")?;
        match apply_one(brain, space, tx, &ev, &via, None) {
            Ok(Step::Deferred) => tx.execute_batch("RELEASE deferred")?,
            Ok(step) => {
                record_raw(
                    tx,
                    &ev.eid,
                    &ev.hlc,
                    &ev.origin,
                    Some(&via),
                    &ev.kind,
                    ev.at,
                    &logged(&ev.payload),
                )?;
                if let Step::Erased(sha) = step {
                    erased.push(sha);
                }
                tx.execute("DELETE FROM sync_deferred WHERE eid = ?1", [&eid])?;
                tx.execute_batch("RELEASE deferred")?;
            }
            Err(Fault::Fatal(e)) => return Err(e),
            Err(_) => {
                tx.execute_batch("ROLLBACK TO deferred; RELEASE deferred")?;
                tx.execute("DELETE FROM sync_deferred WHERE eid = ?1", [&eid])?;
            }
        }
    }
    Ok(())
}

/// The bytes of recordings erased at their origin go here too, once the change is committed, unless another record of
/// the space still holds them. A crash before leaves them to the sweep of the next start (`crate::recover`).
fn remove_erased(brain: &Brain, space: &Space, conn: &Connection, shas: &[String]) -> Result<()> {
    for sha in shas {
        crate::recover::drop_unless_held(brain, space, conn, sha)?;
    }
    Ok(())
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
    /// The position read from the peer's log: where to resume pulling.
    Pulled,
    /// The position of this node's log the peer has acknowledged: where to resume pushing.
    Pushed,
}

/// Moves one cursor of `peer` on `space` to `to` (created at 0 on first use). The caller moves a cursor
/// only after the events before it were applied or acknowledged, so a failure re-sends instead of losing.
/// Effect: one row of `sync_state` in the control database.
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

const PASS_KEY: &str = "sync_pass:";

/// Keeps what the latest pass with `peer` could not do (one line each; none when all went well), so the page and the
/// apps show it: a sync started in the background has no one reading its output. Effect: one row of the control `meta`.
pub fn note_pass(brain: &Brain, peer: &str, at: i64, problems: &[String]) -> Result<()> {
    brain.open_control()?.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![
            format!("{PASS_KEY}{peer}"),
            json!({ "at": at, "problems": problems }).to_string()
        ],
    )?;
    Ok(())
}

/// The latest pass with `peer` as [`note_pass`] kept it: `{at, problems}`; `None` before the first.
pub fn last_pass(ctl: &Connection, peer: &str) -> Option<Value> {
    ctl.query_row(
        "SELECT v FROM meta WHERE k = ?1",
        [format!("{PASS_KEY}{peer}")],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|v| serde_json::from_str(&v).ok())
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

/// How far one space has got in the transfer with a peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceProgress {
    /// Space name.
    pub space: String,
    /// Events moved.
    pub done: i64,
    /// Events to move in all.
    pub total: i64,
    /// `done` over `total`, 0 to 100.
    pub percent: i64,
}

/// One peer as shown on the page and in the `peers` tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStatus {
    /// The connection's name, `peer-<device>-xxxx`.
    pub peer: String,
    /// The machine's name as the person gave it.
    pub name: String,
    /// `sending`, `receiving` or `idle`.
    pub direction: String,
    /// Progress per space.
    pub spaces: Vec<SpaceProgress>,
    /// When the peer last moved events (seconds), if ever.
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
        .prepare("SELECT agent FROM connections WHERE kind = 'peer' AND revoked_at IS NULL")?
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
    /// The JSON shape served to clients: `peer`, `name`, `direction`, `spaces` and `last_activity`.
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
    /// Space pruned.
    pub space: String,
    /// Notes removed, whatever their status.
    pub facts: usize,
    /// Tag rows removed with the notes.
    pub tags: usize,
    /// Files removed with the notes (the bytes too, when nothing else holds them).
    pub files: usize,
    /// Notes outside the window that stay because a peer has not acknowledged them yet.
    pub kept_unacknowledged: usize,
    /// Notes outside the window that stay because they are pinned (or open board notes, or waiting for approval).
    pub kept_pinned: usize,
}

impl Pruned {
    /// The JSON shape: `space`, `facts`, `tags`, `files`, `kept_unacknowledged` and `kept_pinned`.
    pub fn to_json(&self) -> Value {
        json!({
            "space": self.space, "facts": self.facts, "tags": self.tags, "files": self.files,
            "kept_unacknowledged": self.kept_unacknowledged, "kept_pinned": self.kept_pinned,
        })
    }
}

type LogRows = Vec<(i64, Option<String>, String)>;

/// Whether every peer of the space holds the log rows named: for each row, the peer sent it here, or this node has
/// pushed past it and the peer kept it (`unheld` lists the `(peer, eid)` a peer took without keeping: refused, or left
/// out of its own window). No peer, or no row, is never enough.
fn acknowledged(
    rows: &LogRows,
    peers: &[(String, i64)],
    unheld: &BTreeSet<(String, String)>,
) -> bool {
    !peers.is_empty()
        && !rows.is_empty()
        && rows.iter().all(|(seq, via, eid)| {
            peers.iter().all(|(peer, pushed)| {
                via.as_deref() == Some(peer.as_str())
                    || (pushed >= seq && !unheld.contains(&(peer.clone(), eid.clone())))
            })
        })
}

fn log_rows(conn: &Connection, eid: &str, prefix: bool) -> Result<LogRows> {
    let sql = if prefix {
        "SELECT seq, via, eid FROM sync_log WHERE substr(eid, 1, length(?1)) = ?1"
    } else {
        "SELECT seq, via, eid FROM sync_log WHERE eid = ?1"
    };
    Ok(conn
        .prepare(sql)?
        .query_map([eid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// `peer` took these events of `space` without keeping them (it refused them, or left them out of its window): they
/// never count as held by it, so the window of this replica keeps what they are about. Effect: rows of
/// `sync_unheld` in the space.
pub fn note_unheld(brain: &Brain, peer: &str, space: &Space, eids: &[String]) -> Result<()> {
    if eids.is_empty() {
        return Ok(());
    }
    let conn = brain.open_space(space)?;
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    for eid in eids {
        tx.execute(
            "INSERT OR IGNORE INTO sync_unheld (peer, eid, at) VALUES (?1, ?2, ?3)",
            params![peer, eid, util::now()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// `peer` now keeps these events of `space` (a later push it took): an earlier « not kept » of them no longer holds.
pub fn note_held(brain: &Brain, peer: &str, space: &Space, eids: &[String]) -> Result<()> {
    if eids.is_empty() {
        return Ok(());
    }
    let conn = brain.open_space(space)?;
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    for eid in eids {
        tx.execute(
            "DELETE FROM sync_unheld WHERE peer = ?1 AND eid = ?2",
            params![peer, eid],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn withheld_key(who: &str) -> String {
    format!("withheld:{who}")
}

/// Events of `space` from position `seq` on were left out for `who` because it takes no chunks (see
/// [`Page::withheld`]): kept, the earliest only, so that once `who` takes chunks the exchange goes back there.
pub fn note_withheld(brain: &Brain, space: &Space, who: &str, seq: i64) -> Result<()> {
    let conn = brain.open_space(space)?;
    let key = withheld_key(who);
    let earliest = meta_get(&conn, &key)?
        .and_then(|v| v.parse::<i64>().ok())
        .map_or(seq, |was| was.min(seq));
    meta_set(&conn, &key, &earliest.to_string())
}

/// The earliest position left out for `who` (see [`note_withheld`]). Read only: the mark stays until
/// [`clear_withheld`], once what it says is durably taken care of.
///
/// For a node that now takes chunks, the rule is the same on both sides of an exchange: while its cursor (what it
/// pulled, or what was pushed to it) is at or past the mark, the cursor goes back before it first, durably; once the
/// cursor is before the mark, the exchange that follows sends what was left out, and the mark is cleared. A crash
/// between two steps repeats a step, never skips one.
pub fn withheld(brain: &Brain, space: &Space, who: &str) -> Result<Option<i64>> {
    let conn = brain.open_space(space)?;
    Ok(meta_get(&conn, &withheld_key(who))?.and_then(|v| v.parse::<i64>().ok()))
}

/// Clears the mark of [`withheld`] if it is still `seq` (a later [`note_withheld`] of an earlier position keeps it).
pub fn clear_withheld(brain: &Brain, space: &Space, who: &str, seq: i64) -> Result<()> {
    let conn = brain.open_space(space)?;
    conn.execute(
        "DELETE FROM meta WHERE k = ?1 AND v = ?2",
        params![withheld_key(who), seq.to_string()],
    )?;
    Ok(())
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
    let peers: Vec<(String, i64)> = ctl
        .prepare("SELECT peer, pushed FROM sync_state WHERE space = ?1")?
        .query_map([space.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // A machine this node sends nothing of this space to (its policy is `receive` or `off`) holds nothing for it:
    // it is neither a holder nor a reason to wait.
    let gives = |peer: &str| {
        crate::policy::of(&ctl, peer, space.as_str()).map(crate::policy::Policy::gives)
    };
    let mut kept = Vec::with_capacity(peers.len());
    for p in peers {
        if gives(&p.0)? {
            kept.push(p);
        }
    }
    let mut peers = kept;
    // A peer that holds a connection here pulls from this node: its progress is its own, so it counts as unknown.
    let blind: Vec<String> = ctl
        .prepare(
            "SELECT c.agent FROM connections c WHERE c.kind = 'peer' AND c.revoked_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM sync_state s WHERE s.peer = c.agent AND s.space = ?1)",
        )?
        .query_map([space.as_str()], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut blind_given = false;
    for b in &blind {
        blind_given |= gives(b)?;
    }
    if blind_given {
        peers.clear();
    }

    let unheld: BTreeSet<(String, String)> = conn
        .prepare("SELECT peer, eid FROM sync_unheld")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
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
        rows.extend(log_rows(&conn, &format!("d:{id}:"), true)?);
        for fid in strings(&conn, "SELECT id FROM files WHERE fact_id = ?1", id)? {
            rows.extend(file_rows(&fid)?);
        }
        if !created.is_empty() && acknowledged(&rows, &peers, &unheld) {
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
        if acknowledged(&file_rows(&fid)?, &peers, &unheld) {
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
        tx.execute(
            "DELETE FROM sync_log WHERE substr(eid, 1, length(?1)) = ?1",
            [format!("d:{id}:")],
        )?;
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
    crate::failpoint::hit(crate::failpoint::at::SYNC_PRUNE_BYTES)?;
    // The bytes go unless another record of this space still holds them; a crash before leaves them to the sweep of
    // the next start (`crate::recover`).
    for sha in blobs {
        crate::recover::drop_unless_held(brain, space, &conn, &sha)?;
    }
    Ok(out)
}

/// What the last pruning of this replica did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastPrune {
    /// When the pruning ran (seconds since the epoch).
    pub at: i64,
    /// Notes removed.
    pub facts: usize,
    /// Files removed.
    pub files: usize,
    /// Notes kept because a peer has not acknowledged them.
    pub kept_unacknowledged: usize,
}

/// The window of this replica and what was pruned last, for the page and the apps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowStatus {
    /// Size of the window in days; `None` keeps everything.
    pub days: Option<u32>,
    /// What the last pruning did, if one ran.
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

    /// The JSON shape: `days`, `label` and `last_prune` (or `null`).
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

/// The window of this replica and what the last pruning did. Read-only.
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
