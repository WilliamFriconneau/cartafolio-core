//! Facts: write, replace, retire, read the history, search.
//!
//! Rules: a fact is never deleted nor its text changed; correcting = `supersede`;
//! retiring leaves a tombstone; an exact duplicate is dropped; a near-duplicate
//! is not written without a decision.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::space::Space;
use crate::text;
use crate::util;

/// Above this stem Jaccard index, a close fact is a duplicate candidate.
pub const CANDIDATE_THRESHOLD: f64 = 0.5;
const MAX_CANDIDATES: usize = 5;

/// A fact as stored and as returned: one short dated statement with its source and status.
///
/// Immutable once written: the text never changes. The only fields that change afterwards are
/// `status`, `status_*`, `replaced_by`, `valid_to` bookkeeping and `tags` (see `supersede` and `retire`).
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    /// Opaque identifier, unique in the whole installation (random, never reused).
    pub id: String,
    /// The space that holds the fact.
    pub space: Space,
    /// `fact`, `decision`, `context` or `passage` (a passage of an imported note).
    pub kind: String,
    /// Sub-type label: `file` for the fact that describes a stored file, `None` otherwise.
    pub typ: Option<String>,
    /// The text, normalised (NFC, single spaces).
    pub text: String,
    /// `agent` for a fact written by a connection, `note` for a passage of an imported note.
    pub source_kind: String,
    /// Path of the imported note (`note` sources only).
    pub source_path: Option<String>,
    /// Reference of the passage inside the note (heading path), unique among active facts.
    pub source_ref: Option<String>,
    /// Name of the connection that wrote it (`agent` sources only).
    pub source_agent: Option<String>,
    /// Who wrote it: the identity of the connection, never an argument of the request.
    pub author: String,
    /// When it was written (seconds since the epoch).
    pub created_at: i64,
    /// Start of the period the statement is true for (defaults to `created_at`).
    pub valid_from: i64,
    /// End of the period, if the statement expires; an expired fact is hidden unless asked for.
    pub valid_to: Option<i64>,
    /// Id of the version this one corrects.
    pub replaces: Option<String>,
    /// Id of the version that corrected this one (set when `status` is `replaced`).
    pub replaced_by: Option<String>,
    /// `active`, `proposed` (made by a model, never served by default), `replaced` or `retired`.
    pub status: String,
    /// When the status last changed.
    pub status_at: Option<i64>,
    /// Who changed the status.
    pub status_by: Option<String>,
    /// Why: always set for `retired`.
    pub status_reason: Option<String>,
    /// Normalised tags (`work/dev`), sorted; `@space` means "also concerns that space".
    pub tags: Vec<String>,
}

impl Fact {
    /// The JSON shape served to clients: `id`, `space`, `kind`, `type`, `text`, `status`, a `source`
    /// object (`kind`, `path`, `ref`, `agent`), `author`, the dates, `replaces`, `replaced_by`,
    /// `status_at`, `status_by`, `status_reason` and `tags`. Pure.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "space": self.space.as_str(),
            "kind": self.kind,
            "type": self.typ,
            "text": self.text,
            "status": self.status,
            "source": {
                "kind": self.source_kind,
                "path": self.source_path,
                "ref": self.source_ref,
                "agent": self.source_agent,
            },
            "author": self.author,
            "created_at": self.created_at,
            "valid_from": self.valid_from,
            "valid_to": self.valid_to,
            "replaces": self.replaces,
            "replaced_by": self.replaced_by,
            "status_at": self.status_at,
            "status_by": self.status_by,
            "status_reason": self.status_reason,
            "tags": self.tags,
        })
    }
}

/// Where a fact comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Written by an agent (`agent:claude-code`…).
    Agent(String),
    /// Passage of an imported note.
    Note {
        /// Path of the note.
        path: String,
        /// Hash of the note text the passage was taken from.
        hash: String,
        /// Character range of the passage in the note, if known.
        range: Option<String>,
        /// Stable reference of the passage in the note (unique among active facts).
        reference: Option<String>,
    },
}

/// What a caller gives to write a fact. The author and the space are not in it: the writer's
/// identity and the target space come from the call.
#[derive(Clone, Debug)]
pub struct NewFact {
    /// `fact`, `decision`, `context` or `passage`; anything else is refused.
    pub kind: String,
    /// The statement. Not empty, no control characters; normalised when stored.
    pub text: String,
    /// Where it comes from.
    pub source: Source,
    /// Start of validity (seconds); `None` is now.
    pub valid_from: Option<i64>,
    /// End of validity; must not be before `valid_from`.
    pub valid_to: Option<i64>,
    /// `proposed`: output of a model, never served by default.
    pub proposed: bool,
    /// Hierarchical labels (`work/dev`), checked by [`normalize_tags`] when written.
    pub tags: Vec<String>,
}

impl NewFact {
    /// A plain `fact` written by `agent`, with no tags and no dates; the other fields are changed on the value.
    pub fn agent(text: &str, agent: &str) -> NewFact {
        NewFact {
            kind: "fact".into(),
            text: text.into(),
            source: Source::Agent(agent.into()),
            valid_from: None,
            valid_to: None,
            proposed: false,
            tags: vec![],
        }
    }
}

/// An existing active fact close to the one being written.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Id of the existing fact.
    pub id: String,
    /// Its text.
    pub text: String,
    /// Jaccard index of the stems, between [`CANDIDATE_THRESHOLD`] and 1.
    pub similarity: f64,
}

/// What `remember` did. Only `Created` changes the memory.
#[derive(Clone, Debug)]
pub enum Remembered {
    /// The fact was written.
    Created(Fact),
    /// The same exact text is already active: nothing is written.
    Duplicate(Fact),
    /// Close facts exist: nothing is written until the caller has chosen
    /// (`supersede` one candidate, or `distinct_from` to confirm).
    NeedsDecision(Vec<Candidate>),
}

/// One search result.
#[derive(Clone, Debug)]
pub struct Hit {
    /// The fact, whatever its status (the options decide which statuses are searched).
    pub fact: Fact,
    /// Relevance (BM25, higher is better). In a listing (no words) the order is newest first and the score carries no meaning.
    pub score: f64,
}

/// How to search. The default searches active facts only, 10 results, with no tag filter.
#[derive(Clone, Debug, Default)]
pub struct RecallOptions {
    /// Maximum hits; `0` means 10.
    pub limit: usize,
    /// Also search replaced and retired facts.
    pub include_retired: bool,
    /// Also search facts whose `valid_to` is past.
    pub include_expired: bool,
    /// Also search `proposed` facts (a model's output).
    pub include_proposed: bool,
    /// What was true at that date (Unix seconds).
    pub as_of: Option<i64>,
    /// Every one of these tags must match (a tag matches itself and its descendants).
    pub tags: Vec<String>,
    /// When not empty, only facts filed under at least one of these paths: what a connection
    /// limited to paths inside the vault may read.
    pub within: Vec<String>,
}

const COLS: &str = "id, space, kind, type, text, source_kind, source_path, source_ref, source_agent, \
author, created_at, valid_from, valid_to, replaces, replaced_by, status, status_at, status_by, status_reason";

fn fact_of(r: &Row) -> rusqlite::Result<Fact> {
    let space: String = r.get(1)?;
    Ok(Fact {
        id: r.get(0)?,
        space: Space::new(&space).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                e.to_string().into(),
            )
        })?,
        kind: r.get(2)?,
        typ: r.get(3)?,
        text: r.get(4)?,
        source_kind: r.get(5)?,
        source_path: r.get(6)?,
        source_ref: r.get(7)?,
        source_agent: r.get(8)?,
        author: r.get(9)?,
        created_at: r.get(10)?,
        valid_from: r.get(11)?,
        valid_to: r.get(12)?,
        replaces: r.get(13)?,
        replaced_by: r.get(14)?,
        status: r.get(15)?,
        status_at: r.get(16)?,
        status_by: r.get(17)?,
        status_reason: r.get(18)?,
        tags: vec![],
    })
}

/// Reads one fact by id, whatever its status, tags included. `Ok(None)` if the id is unknown in
/// this database (another space's fact is unknown here). Read-only.
pub fn read_fact(conn: &Connection, id: &str) -> Result<Option<Fact>> {
    let found = conn
        .query_row(
            &format!("SELECT {COLS} FROM facts WHERE id = ?1"),
            [id],
            fact_of,
        )
        .optional()?;
    match found {
        Some(mut f) => {
            attach_tags(conn, &mut f)?;
            Ok(Some(f))
        }
        None => Ok(None),
    }
}

// --- tags -------------------------------------------------------------------------------------

/// The tag of a quick note kept to be processed later: the inbox. Notes under it are written
/// without the "close facts" question and are not found by a plain `recall`; they are asked for
/// by this tag, and each connection is told how many wait.
pub const INBOX_TAG: &str = "inbox";

/// The tag raw records from apps are filed under (`sources/<app>`): the vault's raw layer.
pub const SOURCES_TAG: &str = "sources";

fn in_inbox(tag: &str) -> bool {
    tag == INBOX_TAG || tag.starts_with("inbox/")
}

/// The raw layers (the inbox and the sources) are left out of a plain recall: what an assistant
/// reads first is the sorted memory, and it is told how much raw there is, and how to read it.
fn is_cold(tag: &str) -> bool {
    in_inbox(tag) || tag == SOURCES_TAG || tag.starts_with("sources/")
}

/// How many active notes wait in the inbox, and when the oldest was written.
pub fn inbox(conn: &Connection) -> Result<(i64, Option<i64>)> {
    Ok(conn.query_row(
        "SELECT count(DISTINCT f.id), min(f.created_at) FROM facts f JOIN fact_tags t ON t.fact_id = f.id
         WHERE f.status = 'active' AND (t.tag = 'inbox' OR substr(t.tag, 1, 6) = 'inbox/')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Tags on one note, at most.
pub const MAX_TAGS: usize = 64;

/// A tag in its one accepted form, or an error that says what is accepted (an assistant
/// reads it and tries again). Lowercase, accents and spaces tolerated and normalised:
/// `Évaluation / Maths` becomes `evaluation/maths`. Segments of letters, digits, `-` and
/// `_`, separated by `/`, at most four deep. `@name` alone says "also concerns this space".
pub fn normalize_tag(raw: &str) -> Result<String> {
    let bad = |why: &str| {
        Error::Invalid(format!(
            "tag {raw:?}: {why}. A tag is lowercase segments of letters, digits, - or _, separated by / (work/dev/ci); @name means the fact also concerns that space."
        ))
    };
    let folded = text::fold(raw);
    let t: String = folded
        .split('/')
        .map(|seg| seg.split_whitespace().collect::<Vec<_>>().join("-"))
        .collect::<Vec<_>>()
        .join("/");
    if t.is_empty() {
        return Err(bad("empty"));
    }
    if let Some(name) = t.strip_prefix('@') {
        return Space::new(name)
            .map(|s| format!("@{}", s.as_str()))
            .map_err(|_| bad("after @ comes the name of a space"));
    }
    let segments: Vec<&str> = t.split('/').collect();
    for seg in &segments {
        let ok = !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !ok {
            return Err(bad("a segment is empty or has a forbidden character"));
        }
    }
    Ok(t)
}

/// The tags of a fact, each normalised, without repeats, in order.
pub fn normalize_tags(raw: &[String]) -> Result<Vec<String>> {
    if raw.len() > MAX_TAGS {
        return Err(Error::Invalid(format!(
            "at most {MAX_TAGS} tags on one note"
        )));
    }
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let t = normalize_tag(r)?;
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out.sort();
    Ok(out)
}

fn tags_of(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT tag FROM fact_tags WHERE fact_id = ?1 ORDER BY tag")?;
    let rows = st.query_map([id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn attach_tags(conn: &Connection, fact: &mut Fact) -> Result<()> {
    fact.tags = tags_of(conn, &fact.id)?;
    Ok(())
}

/// The tags in use on active facts, with how many facts each covers. A fact counts for
/// every level of its tags (`work/dev/ci` counts for `work`, `work/dev` and itself), so the
/// list reads as a tree of folders. Concern tags (`@space`) are not part of it. Most used
/// first, at most 200.
pub fn tag_counts(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut st = conn.prepare(
        "SELECT t.fact_id, t.tag FROM fact_tags t JOIN facts f ON f.id = t.fact_id
         WHERE f.status = 'active' AND substr(t.tag, 1, 1) <> '@' ORDER BY t.fact_id",
    )?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut current: Option<String> = None;
    let mut levels: std::collections::BTreeSet<String> = Default::default();
    let mut flush = |levels: &mut std::collections::BTreeSet<String>| {
        for l in std::mem::take(levels) {
            *counts.entry(l).or_insert(0) += 1;
        }
    };
    for row in rows {
        let (id, tag) = row?;
        if current.as_deref() != Some(id.as_str()) {
            flush(&mut levels);
            current = Some(id);
        }
        let mut path = String::new();
        for seg in tag.split('/') {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(seg);
            levels.insert(path.clone());
        }
    }
    flush(&mut levels);
    let mut out: Vec<(String, i64)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.truncate(200);
    Ok(out)
}

pub(crate) fn active_by_hash(conn: &Connection, hash: &str) -> Result<Option<Fact>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM facts WHERE text_hash = ?1 AND status = 'active'"),
            [hash],
            fact_of,
        )
        .optional()?)
}

pub(crate) fn check_text(text: &str) -> Result<String> {
    let t = text::normalize(text);
    let n = t.chars().count();
    if n == 0 {
        return Err(Error::Invalid("the text is empty".into()));
    }
    if t.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "the text contains control characters".into(),
        ));
    }
    Ok(t)
}

pub(crate) fn check_kind(kind: &str) -> Result<()> {
    if ["fact", "decision", "context", "passage"].contains(&kind) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "unknown kind {kind:?} (fact, decision, context, passage)"
        )))
    }
}

pub(crate) fn event(
    conn: &Connection,
    fact_id: &str,
    actor: &str,
    action: &str,
    detail: Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO fact_events (fact_id, at, actor, action, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![fact_id, util::now(), actor, action, detail.to_string()],
    )?;
    Ok(())
}

/// Inserts a fact and its index entry, in the caller's transaction, and logs it for the peers.
pub(crate) fn insert(
    conn: &Connection,
    space: &Space,
    author: &str,
    new: &NewFact,
    text: &str,
    replaces: Option<&str>,
) -> Result<Fact> {
    let fact = insert_unlogged(conn, space, author, new, text, replaces)?;
    crate::sync::record_fact_created(conn, &fact.id)?;
    Ok(fact)
}

/// [`insert`] without the entry in the peers' log: the caller logs it once the fact is complete.
fn insert_unlogged(
    conn: &Connection,
    space: &Space,
    author: &str,
    new: &NewFact,
    text: &str,
    replaces: Option<&str>,
) -> Result<Fact> {
    let id = util::new_fact_id();
    let now = util::now();
    let valid_from = new.valid_from.unwrap_or(now);
    let (source_kind, path, hash, range, reference, agent) = match &new.source {
        Source::Agent(a) => ("agent", None, None, None, None, Some(a.as_str())),
        Source::Note {
            path,
            hash,
            range,
            reference,
        } => (
            "note",
            Some(path.as_str()),
            Some(hash.as_str()),
            range.as_deref(),
            reference.as_deref(),
            None,
        ),
    };
    let status = if new.proposed { "proposed" } else { "active" };
    conn.execute(
        "INSERT INTO facts (id, space, kind, text, text_hash, source_kind, source_path, source_hash,
            source_range, source_ref, source_agent, author, created_at, valid_from, valid_to, replaces, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            id,
            space.as_str(),
            new.kind,
            text,
            text::text_hash(text),
            source_kind,
            path,
            hash,
            range,
            reference,
            agent,
            author,
            now,
            valid_from,
            new.valid_to,
            replaces,
            status
        ],
    )?;
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO facts_fts (rowid, stem) VALUES (?1, ?2)",
        params![rowid, text::stems(text).join(" ")],
    )?;
    for tag in &new.tags {
        conn.execute(
            "INSERT OR IGNORE INTO fact_tags (fact_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    event(conn, &id, author, "create", json!({ "replaces": replaces }))?;
    read_fact(conn, &id)?.ok_or_else(|| Error::Schema("fact not found after writing".into()))
}

/// The most distinct words one search looks for: an OR of thousands of terms is a slow query, not a better one.
const MAX_SEARCH_TERMS: usize = 64;

/// How many terms the search of `text` would use (see `MAX_SEARCH_TERMS`).
pub fn fts_terms(text: &str) -> usize {
    fts_query(&text::stems(text)).map_or(0, |q| q.matches(" OR ").count() + 1)
}

fn fts_query(stems: &[String]) -> Option<String> {
    let mut seen = std::collections::BTreeSet::new();
    let terms: Vec<String> = stems
        .iter()
        .filter(|s| seen.insert((*s).clone()))
        .take(MAX_SEARCH_TERMS)
        .map(|s| format!("\"{}\"", s.replace('"', "")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Active facts close to the text, closest first.
fn candidates_for(conn: &Connection, text: &str, ignore: &[&str]) -> Result<Vec<Candidate>> {
    close_facts(conn, text, ignore, CANDIDATE_THRESHOLD)
}

fn close_facts(
    conn: &Connection,
    text: &str,
    ignore: &[&str],
    least: f64,
) -> Result<Vec<Candidate>> {
    let stems = text::stems(text);
    let Some(q) = fts_query(&stems) else {
        return Ok(vec![]);
    };
    let mut st = conn.prepare(
        "SELECT f.id, f.text FROM facts_fts JOIN facts f ON f.rowid = facts_fts.rowid
         WHERE facts_fts MATCH ?1 AND f.status = 'active' ORDER BY bm25(facts_fts) LIMIT 25",
    )?;
    let rows = st.query_map([q], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = vec![];
    for row in rows {
        let (id, t) = row?;
        if ignore.contains(&id.as_str()) {
            continue;
        }
        let sim = text::similarity(&stems, &text::stems(&t));
        if sim >= least {
            out.push(Candidate {
                id,
                text: t,
                similarity: sim,
            });
        }
    }
    out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    out.truncate(MAX_CANDIDATES);
    Ok(out)
}

/// Writes a new fact in `space` on behalf of `author`.
///
/// Arguments: the fact to write (`new`) and `distinct_from`, the ids of close facts the caller has
/// looked at and judges different (they are then not counted as candidates).
///
/// Returns [`Remembered::Created`] with the stored fact (status `active`, or `proposed` when
/// `new.proposed`), [`Remembered::Duplicate`] with the existing fact when the exact same normalised
/// text is already active (nothing is written), or [`Remembered::NeedsDecision`] with up to five
/// close active facts (stem similarity at least [`CANDIDATE_THRESHOLD`]; nothing is written until
/// the caller `supersede`s one of them or repeats the call with `distinct_from`). A fact tagged
/// `inbox` skips the closeness check: a quick note is taken as it comes.
///
/// # Errors
/// [`Error::Invalid`] for an empty text, control characters, an unknown kind, a bad tag, or
/// `valid_to` before `valid_from`.
///
/// # Effects
/// One transaction: a row in `facts` and `fact_tags`, an event `remember` in the fact's history and
/// an entry in the sync log. Nothing already stored is touched.
pub fn remember(
    conn: &Connection,
    space: &Space,
    author: &str,
    mut new: NewFact,
    distinct_from: &[String],
) -> Result<Remembered> {
    new.tags = normalize_tags(&new.tags)?;
    check_kind(&new.kind)?;
    let t = check_text(&new.text)?;
    if let (Some(from), Some(to)) = (new.valid_from, new.valid_to)
        && to < from
    {
        return Err(Error::Invalid("valid_to is before valid_from".into()));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if let Some(existing) = active_by_hash(&tx, &text::text_hash(&t))? {
        return Ok(Remembered::Duplicate(existing));
    }
    let ignore: Vec<&str> = distinct_from.iter().map(String::as_str).collect();
    // A quick note for the inbox is taken as it comes: sorting it out is for later.
    let cands = if new.tags.iter().any(|g| in_inbox(g)) {
        vec![]
    } else {
        candidates_for(&tx, &t, &ignore)?
    };
    if !cands.is_empty() {
        return Ok(Remembered::NeedsDecision(cands));
    }
    let fact = insert(&tx, space, author, &new, &t, None)?;
    tx.commit()?;
    Ok(Remembered::Created(fact))
}

/// Close enough to be read as the same fact in a report.
pub const SAME_FACT_THRESHOLD: f64 = 0.8;

/// A pair of active facts of one space that say nearly the same thing.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosePair {
    /// Id and text of the first fact.
    pub first: (String, String),
    /// Id and text of the second fact.
    pub second: (String, String),
    /// Stem similarity of the two texts, from the threshold to 1.
    pub similarity: f64,
}

/// Every active fact of a space: id, text digest, text. Read only.
pub fn active_digests(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut st = conn.prepare("SELECT id, text_hash, text FROM facts WHERE status = 'active'")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The pairs of active facts at or above `least`, most alike first, each pair
/// once. Read only: a report for a person to decide on.
pub fn close_pairs(conn: &Connection, least: f64) -> Result<Vec<ClosePair>> {
    let mut out = vec![];
    for (id, _, text) in active_digests(conn)? {
        for c in close_facts(conn, &text, &[&id], least)? {
            if id < c.id {
                out.push(ClosePair {
                    first: (id.clone(), text.clone()),
                    second: (c.id, c.text),
                    similarity: c.similarity,
                });
            }
        }
    }
    out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    Ok(out)
}

/// A fact longer than this is probably several facts.
const LONG_FACT_CHARS: usize = 400;
/// Close enough to be worth a look, not enough to block the write.
const NEIGHBOUR_THRESHOLD: f64 = 0.3;

/// The tidying that goes with each write (5S): what the memory noticed around a
/// fact just written. It is computed, never applied: nothing changes here, the
/// agent or the person decides with `supersede` or `retire`. Empty when all is in
/// order. `id` and `text` are the fact written.
pub fn tidy_after(conn: &Connection, id: &str, text: &str, now: i64) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    // Sort: what says nearly the same thing.
    let close = close_facts(conn, text, &[id], NEIGHBOUR_THRESHOLD)?;
    if !close.is_empty() {
        let list: Vec<Value> = close
            .iter()
            .take(3)
            .map(|c| json!({ "id": c.id, "text": c.text }))
            .collect();
        out.insert("close".into(), Value::Array(list));
    }
    // Shine: what is past its date and still active.
    let mut st = conn.prepare(
        "SELECT id, text FROM facts WHERE status = 'active' AND valid_to IS NOT NULL AND valid_to < ?1
         ORDER BY valid_to LIMIT 3",
    )?;
    let expired: Vec<Value> = st
        .query_map([now], |r| {
            Ok(json!({ "id": r.get::<_, String>(0)?, "text": r.get::<_, String>(1)? }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if !expired.is_empty() {
        out.insert("expired".into(), Value::Array(expired));
    }
    // Standardize: one fact, short.
    if text.chars().count() > LONG_FACT_CHARS {
        out.insert(
            "too_long".into(),
            json!("Probably several facts: split it."),
        );
    }
    Ok(out)
}

/// Replaces the active fact `old_id` with a corrected version, written by `author`.
///
/// The old fact becomes `replaced` (never erased: its text stays readable through [`history`]) and
/// points to the new one. The new fact keeps the old fact's tags unless `new` brings its own;
/// the same text is accepted only when it is filed under different tags (sorting an inbox note).
/// `reason` is stored in the history and the old fact's `status_reason`.
///
/// # Errors
/// [`Error::UnknownFact`] if `old_id` is not in `space`; [`Error::Invalid`] if the old fact is not
/// `active`, the new text is identical (same tags) or already active as another fact, or `new` breaks
/// the rules of [`remember`] (text, kind, tags).
///
/// # Effects
/// One transaction: the old row is updated, a new row is inserted, an event `supersede` is
/// appended to the history, and the sync log records the replacement.
pub fn supersede(
    conn: &Connection,
    space: &Space,
    author: &str,
    old_id: &str,
    new: NewFact,
    reason: Option<&str>,
) -> Result<Fact> {
    supersede_as(conn, space, author, old_id, new, reason, "supersede")
}

/// `supersede` with the action written in the history (the importer writes `import_replace`).
pub(crate) fn supersede_as(
    conn: &Connection,
    space: &Space,
    author: &str,
    old_id: &str,
    new: NewFact,
    reason: Option<&str>,
    action: &str,
) -> Result<Fact> {
    check_kind(&new.kind)?;
    let t = check_text(&new.text)?;
    let given_tags = normalize_tags(&new.tags)?;
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let old = read_fact(&tx, old_id)?.ok_or_else(|| Error::UnknownFact(old_id.into()))?;
    if old.status != "active" {
        return Err(Error::Invalid(format!(
            "fact {old_id} is {}: only an active fact can be replaced",
            old.status
        )));
    }
    let hash = text::text_hash(&t);
    // The same text is a change only when it is filed elsewhere: sorting a note of the inbox under its real tags.
    let same_text = text::text_hash(&old.text) == hash;
    if same_text {
        let mut given_sorted = given_tags.clone();
        given_sorted.sort();
        let mut old_sorted = old.tags.clone();
        old_sorted.sort();
        if given_tags.is_empty() || given_sorted == old_sorted {
            return Err(Error::Invalid(
                "the new text is identical to the old one".into(),
            ));
        }
    }
    if let Some(other) = active_by_hash(&tx, &hash)?.filter(|o| o.id != old_id) {
        return Err(Error::Invalid(format!(
            "this text already exists, active, as {}",
            other.id
        )));
    }
    // The source reference is unique among active facts: the new one only takes it
    // once the old one is marked as replaced.
    let mut staged = new.clone();
    // A correction keeps the tags of what it corrects, unless it brings its own.
    staged.tags = if given_tags.is_empty() {
        old.tags.clone()
    } else {
        given_tags
    };
    let deferred_ref = match &mut staged.source {
        Source::Note { reference, .. } => reference.take(),
        Source::Agent(_) => None,
    };
    // The old one leaves the active set first (one text is active once), then the new one comes, then they are linked:
    // the link names a fact that does not exist yet, so the foreign key is checked at the commit.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    let changed = tx.execute(
        "UPDATE facts SET status = 'replaced', replaced_by = ?1, status_at = ?2, status_by = ?3, status_reason = ?4
         WHERE id = ?1 AND status = 'active'",
        params![old_id, util::now(), author, reason],
    )?;
    if changed != 1 {
        // Another replacement got through meanwhile: only one wins.
        return Err(Error::Invalid(format!(
            "fact {old_id} was just replaced or retired by another call"
        )));
    }
    let fact = insert_unlogged(&tx, space, author, &staged, &t, Some(old_id))?;
    tx.execute(
        "UPDATE facts SET replaced_by = ?2 WHERE id = ?1",
        params![old_id, fact.id],
    )?;
    if let Some(r) = deferred_ref {
        tx.execute(
            "UPDATE facts SET source_ref = ?2 WHERE id = ?1",
            params![fact.id, r],
        )?;
    }
    crate::sync::record_fact_created(&tx, &fact.id)?;
    crate::sync::record_fact_superseded(&tx, old_id, &fact.id, author, reason)?;
    event(
        &tx,
        old_id,
        author,
        action,
        json!({ "by": fact.id, "reason": reason }),
    )?;
    tx.commit()?;
    read_fact(conn, &fact.id)?.ok_or_else(|| Error::UnknownFact(fact.id))
}

/// Retires an `active` or `proposed` fact: it leaves default searches but its text stays and can
/// be read by id and in [`history`]. Retiring is how a wrong or obsolete fact is removed; nothing is
/// ever erased.
///
/// # Errors
/// [`Error::Invalid`] if `reason` is empty, or the fact is already replaced or retired;
/// [`Error::UnknownFact`] if `id` is not in this database.
///
/// # Effects
/// The row gets status `retired`, `status_at`, `status_by = author` and `status_reason = reason`;
/// an event `retire` is appended to the history and the sync log records it.
pub fn retire(conn: &Connection, author: &str, id: &str, reason: &str) -> Result<Fact> {
    retire_as(conn, author, id, reason, "retire")
}

/// `retire` with the action written in the history (the importer writes `import_retire`).
pub(crate) fn retire_as(
    conn: &Connection,
    author: &str,
    id: &str,
    reason: &str,
    action: &str,
) -> Result<Fact> {
    if reason.trim().is_empty() {
        return Err(Error::Invalid("retiring needs a reason".into()));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let f = read_fact(&tx, id)?.ok_or_else(|| Error::UnknownFact(id.into()))?;
    if f.status != "active" && f.status != "proposed" {
        return Err(Error::Invalid(format!(
            "fact {id} is {}: only an active or proposed fact can be retired",
            f.status
        )));
    }
    tx.execute(
        "UPDATE facts SET status = 'retired', status_at = ?2, status_by = ?3, status_reason = ?4 WHERE id = ?1",
        params![id, util::now(), author, reason],
    )?;
    event(&tx, id, author, action, json!({ "reason": reason }))?;
    crate::sync::record_fact_retired(&tx, id, author, reason)?;
    tx.commit()?;
    read_fact(conn, id)?.ok_or_else(|| Error::UnknownFact(id.into()))
}

/// The replacement chain around a fact, from the oldest version to the newest, whatever the status of
/// each version. Read-only.
///
/// # Errors
/// [`Error::UnknownFact`] if `id` is not in this database.
pub fn history(conn: &Connection, id: &str) -> Result<Vec<Fact>> {
    let start = read_fact(conn, id)?.ok_or_else(|| Error::UnknownFact(id.into()))?;
    let mut chain = vec![start.clone()];
    let mut cur = start.clone();
    while let Some(prev) = cur.replaces.clone() {
        let Some(f) = read_fact(conn, &prev)? else {
            break;
        };
        chain.insert(0, f.clone());
        cur = f;
        if chain.len() > 1000 {
            break;
        }
    }
    let mut cur = start;
    while let Some(next) = cur.replaced_by.clone() {
        let Some(f) = read_fact(conn, &next)? else {
            break;
        };
        chain.push(f.clone());
        cur = f;
        if chain.len() > 1000 {
            break;
        }
    }
    Ok(chain)
}

/// Lexical search (FTS5, BM25) on stems. Never a summary: the exact text, its
/// source, its date and its status. Read-only.
///
/// `query` is matched on stems (accents, case and plurals ignored; common words dropped). `opts`
/// chooses the statuses, the date (`as_of`), the tags and the paths (`within`). The raw layers
/// (`inbox`, `sources`) are left out unless one of them is asked for by tag. Returns at most
/// `opts.limit` hits (10 if 0), best first.
///
/// # Errors
/// [`Error::Invalid`] for a malformed tag in `opts`.
///
/// A query with no word to search for (empty, or only words too common to count) and at least one tag
/// is a **listing**: what is filed under those tags, newest first. It is how an inbox is read.
pub fn recall(conn: &Connection, query: &str, opts: &RecallOptions, now: i64) -> Result<Vec<Hit>> {
    let fts = fts_query(&text::stems(query));
    let listing = fts.is_none();
    if listing && normalize_tags(&opts.tags)?.is_empty() {
        return Ok(vec![]);
    }
    let q = fts.unwrap_or_default();
    let limit = if opts.limit == 0 { 10 } else { opts.limit };
    let wanted_tags = normalize_tags(&opts.tags)?;
    let within = normalize_tags(&opts.within)?;
    // Each required tag: the fact has it, or has a descendant of it.
    // The raw layers are left out unless one of them is asked for by its tag.
    let inbox_clause = if wanted_tags.iter().chain(within.iter()).any(|g| is_cold(g)) {
        ""
    } else {
        " AND NOT EXISTS (SELECT 1 FROM fact_tags i WHERE i.fact_id = f.id AND (i.tag IN ('inbox', 'sources') OR substr(i.tag, 1, 6) = 'inbox/' OR substr(i.tag, 1, 8) = 'sources/'))"
    };
    let tag_clauses: String = (0..wanted_tags.len())
        .map(|i| {
            let n = i + 2;
            format!(
                " AND EXISTS (SELECT 1 FROM fact_tags t WHERE t.fact_id = f.id AND (t.tag = ?{n} OR substr(t.tag, 1, length(?{n}) + 1) = ?{n} || '/'))"
            )
        })
        .collect();
    // Within the allowed paths: the fact has one of them, or a descendant of one.
    let within_clause: String = if within.is_empty() {
        String::new()
    } else {
        let any: Vec<String> = (0..within.len())
            .map(|j| {
                let n = wanted_tags.len() + j + 2;
                format!(
                    "EXISTS (SELECT 1 FROM fact_tags w WHERE w.fact_id = f.id AND (w.tag = ?{n} OR substr(w.tag, 1, length(?{n}) + 1) = ?{n} || '/'))"
                )
            })
            .collect();
        format!(" AND ({})", any.join(" OR "))
    };
    let cols = COLS
        .split(", ")
        .map(|c| format!("f.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = if listing {
        // `?1 IS NOT NULL` only keeps the first parameter in use: the tags start at `?2` either way.
        // The status is filtered here, not only after: sorted notes keep their tag, and a window of the newest
        // rows taken before filtering would hide an old note still waiting behind them.
        let status_clause = if opts.as_of.is_none() && !opts.include_retired {
            if opts.include_proposed {
                " AND f.status IN ('active', 'proposed')"
            } else {
                " AND f.status = 'active'"
            }
        } else {
            ""
        };
        format!(
            "SELECT {cols}, 0.0 AS rank FROM facts f WHERE ?1 IS NOT NULL{status_clause}{inbox_clause}{tag_clauses}{within_clause}
             ORDER BY f.created_at DESC, f.id DESC LIMIT 200"
        )
    } else {
        format!(
            "SELECT {cols}, bm25(facts_fts) AS rank FROM facts_fts JOIN facts f ON f.rowid = facts_fts.rowid
             WHERE facts_fts MATCH ?1{inbox_clause}{tag_clauses}{within_clause} ORDER BY rank LIMIT 200"
        )
    };
    let mut st = conn.prepare(&sql)?;
    let params: Vec<&str> = std::iter::once(q.as_str())
        .chain(wanted_tags.iter().map(String::as_str))
        .chain(within.iter().map(String::as_str))
        .collect();
    let rows = st.query_map(rusqlite::params_from_iter(params), |r| {
        Ok((fact_of(r)?, r.get::<_, f64>(19)?))
    })?;
    let mut hits = vec![];
    for row in rows {
        let (mut fact, rank) = row?;
        let keep = match opts.as_of {
            Some(t) => {
                fact.created_at <= t
                    && fact.valid_from <= t
                    && fact.valid_to.is_none_or(|v| v > t)
                    && (fact.status == "active"
                        || (matches!(fact.status.as_str(), "replaced" | "retired")
                            && fact.status_at.is_some_and(|s| s > t)))
            }
            None => {
                let status_ok = match fact.status.as_str() {
                    "active" => true,
                    "proposed" => opts.include_proposed,
                    _ => opts.include_retired,
                };
                let expired = fact.valid_to.is_some_and(|v| v <= now);
                status_ok && (opts.include_expired || !expired)
            }
        };
        if keep {
            attach_tags(conn, &mut fact)?;
            hits.push(Hit {
                fact,
                // bm25 returns a negative value: smaller is better.
                score: -rank,
            });
        }
        if hits.len() == limit {
            break;
        }
    }
    Ok(hits)
}

/// Number of facts per status, as a JSON object (`{"active": 12, "retired": 1}`; absent statuses are
/// not listed). Read-only.
pub fn counts(conn: &Connection) -> Result<Value> {
    let mut st = conn.prepare("SELECT status, count(*) FROM facts GROUP BY status")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut m = serde_json::Map::new();
    for row in rows {
        let (s, n) = row?;
        m.insert(s, json!(n));
    }
    Ok(Value::Object(m))
}

/// Appends one row to the space audit log: who (`client`) called which `tool`, whether it worked,
/// the ids of the facts touched, a hash of the arguments and the first 300 characters of `detail`.
/// Never the full text: it is in `facts`. The log is append-only.
pub fn audit(
    conn: &Connection,
    client: &str,
    tool: &str,
    ok: bool,
    fact_ids: &[String],
    args: &Value,
    detail: &str,
) -> Result<()> {
    let args_hash = text::text_hash(&args.to_string());
    conn.execute(
        "INSERT INTO audit (ts, client, tool, ok, fact_ids, args_hash, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            util::now(),
            client,
            tool,
            i64::from(ok),
            fact_ids.join(","),
            args_hash,
            detail.chars().take(300).collect::<String>()
        ],
    )?;
    Ok(())
}

/// Active passages of a note, by `source_ref`.
pub(crate) fn active_passages_of(conn: &Connection, path: &str) -> Result<Vec<Fact>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM facts WHERE source_path = ?1 AND kind = 'passage' AND status = 'active' ORDER BY source_ref"
    ))?;
    let rows = st.query_map([path], fact_of)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// The active fact carrying exactly this text, if any.
pub(crate) fn active_with_text(conn: &Connection, text: &str) -> Result<Option<Fact>> {
    active_by_hash(conn, &text::text_hash(text))
}

/// Creates a note passage, without looking for near-duplicates (import is deterministic).
/// `None` if this exact text is already active.
pub(crate) fn create_passage(
    conn: &Connection,
    space: &Space,
    author: &str,
    text_raw: &str,
    source: Source,
    valid_from: Option<i64>,
) -> Result<Option<Fact>> {
    let t = check_text(text_raw)?;
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if active_by_hash(&tx, &text::text_hash(&t))?.is_some() {
        return Ok(None);
    }
    let new = NewFact {
        kind: "passage".into(),
        text: t.clone(),
        source,
        valid_from,
        valid_to: None,
        proposed: false,
        tags: vec![],
    };
    let fact = insert(&tx, space, author, &new, &t, None)?;
    tx.commit()?;
    Ok(Some(fact))
}

/// Moves a passage pointer to another note (the note was moved or renamed).
pub(crate) fn repoint(conn: &Connection, actor: &str, id: &str, source: &Source) -> Result<()> {
    let Source::Note {
        path,
        hash,
        range,
        reference,
    } = source
    else {
        return Err(Error::Invalid("repoint: a note source is expected".into()));
    };
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let old = read_fact(&tx, id)?.ok_or_else(|| Error::UnknownFact(id.into()))?;
    tx.execute(
        "UPDATE facts SET source_path = ?2, source_hash = ?3, source_range = ?4, source_ref = ?5 WHERE id = ?1",
        params![id, path, hash, range, reference],
    )?;
    event(
        &tx,
        id,
        actor,
        "repoint",
        json!({ "from_path": old.source_path, "from_ref": old.source_ref, "to_path": path, "to_ref": reference }),
    )?;
    tx.commit()?;
    Ok(())
}
