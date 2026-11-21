//! Facts that an app deposits under its own record ids, and the point to resume from.
//!
//! A source (a health app, a sensor hub) gives each record the id it has for it, and the
//! instant of the data point. The vault then does three things and nothing more:
//!
//! - the same app sending the same id again changes nothing when the text is the same, and
//!   otherwise **merges** into the app's own earlier version instead of replacing it (the
//!   history keeps both), so a day's summary can be completed as the day goes on without
//!   piling up copies and without a thinner reading erasing a fuller one. Raw records
//!   (`sources/<app>`): two JSON objects merge field by field (a new field is added, a
//!   non-null value updates, a null, empty or absent value erases nothing, nested objects
//!   merge the same way); any other text is appended to what is held (nothing is dropped,
//!   and a text already contained in the held one changes nothing). The dossier layer, the
//!   sorted memory of a reader, is still rewritten whole;
//! - every such fact is filed under `sources/<app>`, whatever tags it asked for;
//! - the latest `at` it holds for the app, and how many facts, come back as a cursor: the
//!   app resumes from there instead of keeping its own account of what was delivered.
//!
//! Two different apps that send the same measure are two sources: both are kept. Choosing
//! one per measure is the reader's business.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::facts::{self, Fact, NewFact};
use crate::space::Space;
use crate::util;

/// Which layer a deposit belongs to: the raw records of an app (`sources/<app>`, left out of a
/// plain recall), or the dossier, the sorted memory an expert reader writes after reconciling the
/// sources (`dossier/<app>`, read first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// The raw records of an app, tagged `sources/<app>` and left out of a plain recall.
    Source,
    /// The sorted memory an expert reader writes after reconciling sources, tagged `dossier/<app>`.
    Dossier,
}

impl Layer {
    /// Reads `"source"` or `"dossier"`; [`Error::Invalid`] for anything else.
    pub fn parse(s: &str) -> Result<Layer> {
        match s {
            "source" => Ok(Layer::Source),
            "dossier" => Ok(Layer::Dossier),
            _ => Err(Error::Invalid(
                "origin.layer: \"source\" (the default) or \"dossier\"".into(),
            )),
        }
    }

    /// `"source"` or `"dossier"`.
    pub fn word(self) -> &'static str {
        match self {
            Layer::Source => "source",
            Layer::Dossier => "dossier",
        }
    }

    fn tag_root(self) -> &'static str {
        match self {
            Layer::Source => "sources",
            Layer::Dossier => "dossier",
        }
    }
}

/// Where a deposited record comes from: the app, its own id for the record, and when the data point happened.
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    /// Lowercase name of the app (letters, digits, `-`, `_`).
    pub app: String,
    /// The app's own identifier of the record (the same id again means the same record).
    pub id: String,
    /// When the data point happened (ISO 8601, starting `YYYY-MM-DD`). Left out: now.
    pub at: Option<String>,
    /// A standard code for what was measured, `system:code` (`loinc:8867-4`). The fact is then
    /// filed under `code/<system>/<code>`, so the same measure from two sources is found
    /// together. The vault carries no table of codes and checks only the shape.
    pub code: Option<String>,
    /// Raw source (the default) or the sorted dossier.
    pub layer: Layer,
}

/// What a deposit did.
#[derive(Clone, Debug)]
pub enum Deposit {
    /// The record is new: the fact was written.
    Created(Fact),
    /// The vault already held exactly this under this id: nothing written.
    Unchanged(Fact),
    /// Exactly this text is held, but not under this app's id (another source, or a record
    /// the app has under another id): nothing written, and the app's cursor does not move.
    /// Naming the source in the text keeps both.
    SameTextHeld(Fact),
    /// The app's earlier version of this record is replaced by its merge with the new one (or by
    /// the new one in the dossier layer); the old one stays in the history.
    Updated {
        /// The merged (or rewritten) fact.
        fact: Fact,
        /// Id of the earlier version it replaced.
        replaced: String,
    },
}

/// The point to resume from for one source.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    /// The app.
    pub app: String,
    /// The latest `at` the memory holds for the app.
    pub last_at: Option<String>,
    /// When the memory last received a fact of this app: always a real instant, unlike `last_at`, which is the app's own
    /// word (an app may date a day at its last minute, so ahead of now).
    pub received_at: Option<String>,
    /// Number of facts the app has deposited.
    pub facts: i64,
}

impl Cursor {
    /// The JSON shape: `app`, `last_at`, `received_at` and `facts`.
    pub fn to_json(&self) -> Value {
        json!({ "app": self.app, "last_at": self.last_at, "received_at": self.received_at, "facts": self.facts })
    }
}

impl Origin {
    /// Checks and normalises what the caller gave.
    pub fn parse(app: &str, id: &str, at: Option<&str>) -> Result<Origin> {
        let app = app.trim().to_lowercase();
        let ok_app = !app.is_empty()
            && app
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !ok_app || !app.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(Error::Invalid(
                "origin.app: lowercase letters, digits, - or _".into(),
            ));
        }
        let id = id.trim().to_string();
        if id.is_empty() || id.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "origin.id: the app's own id for the record, not empty".into(),
            ));
        }
        let at = match at.map(str::trim).filter(|a| !a.is_empty()) {
            None => None,
            Some(a) => {
                let b = a.as_bytes();
                let dated = a.len() >= 10
                    && a.len() <= 40
                    && b[..4].iter().all(u8::is_ascii_digit)
                    && b[4] == b'-'
                    && b[5..7].iter().all(u8::is_ascii_digit)
                    && b[7] == b'-'
                    && b[8..10].iter().all(u8::is_ascii_digit);
                if !dated {
                    return Err(Error::Invalid(
                        "origin.at: an ISO 8601 date or time, like 2026-11-01 or 2026-11-01T07:30:00Z"
                            .into(),
                    ));
                }
                Some(a.to_string())
            }
        };
        Ok(Origin {
            app,
            id,
            at,
            code: None,
            layer: Layer::Source,
        })
    }

    /// Adds a standard code (`loinc:8867-4`): a system of lowercase letters and digits, a colon,
    /// then the code (letters, digits, `-` or `.`).
    pub fn with_code(mut self, code: &str) -> Result<Origin> {
        let bad = || {
            Error::Invalid(
                "origin.code: system:code, like loinc:8867-4 (letters, digits, - or .)".into(),
            )
        };
        let (system, c) = code.trim().split_once(':').ok_or_else(bad)?;
        let system = system.to_lowercase();
        let c = c.to_lowercase().replace('.', "-");
        let word = |t: &str| {
            !t.is_empty()
                && t.chars().count() <= 24
                && t.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        };
        if !word(&system) || !word(&c) {
            return Err(bad());
        }
        self.code = Some(format!("{system}:{c}"));
        Ok(self)
    }

    /// Writes into the dossier layer instead of the raw one.
    pub fn in_layer(mut self, layer: Layer) -> Origin {
        self.layer = layer;
        self
    }

    /// The tag a deposit under this origin is filed under (`sources/<app>` or `dossier/<app>`).
    pub fn scope_tag(&self) -> String {
        self.tag()
    }

    pub(crate) fn versions(&self, conn: &Connection) -> Result<Vec<String>> {
        let mut query = conn.prepare(
            "SELECT fact_id FROM fact_origin WHERE app = ?1 AND ext_id = ?2 AND layer = ?3",
        )?;
        Ok(query
            .query_map(params![self.app, self.id, self.layer.word()], |row| {
                row.get(0)
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn tag(&self) -> String {
        format!("{}/{}", self.layer.tag_root(), self.app)
    }

    fn code_tag(&self) -> Option<String> {
        self.code
            .as_ref()
            .map(|c| format!("code/{}", c.replacen(':', "/", 1)))
    }
}

fn held(conn: &Connection, o: &Origin) -> Result<Option<Fact>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT o.fact_id FROM fact_origin o JOIN facts f ON f.id = o.fact_id
             WHERE o.app = ?1 AND o.ext_id = ?2 AND o.layer = ?3 AND f.status = 'active'",
            params![o.app, o.id, o.layer.word()],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => facts::read_fact(conn, &id),
        None => Ok(None),
    }
}

pub(crate) fn record(conn: &Connection, fact_id: &str, o: &Origin, now: i64) -> Result<()> {
    let at = o.at.clone().unwrap_or_else(|| util::iso(now));
    conn.execute(
        "INSERT OR REPLACE INTO fact_origin (fact_id, app, ext_id, at, layer) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![fact_id, o.app, o.id, at, o.layer.word()],
    )?;
    Ok(())
}

/// Fields that say nothing: a null, an empty text, list or object never erases a held value.
fn empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// `new` completed into `held`: new fields are added, a non-empty value updates, an empty or
/// absent one leaves what is held; objects merge field by field.
fn merge_json(held: &mut serde_json::Map<String, Value>, new: &serde_json::Map<String, Value>) {
    for (k, v) in new {
        if empty(v) {
            continue;
        }
        match (held.get_mut(k), v) {
            (Some(Value::Object(h)), Value::Object(n)) => merge_json(h, n),
            _ => {
                held.insert(k.clone(), v.clone());
            }
        }
    }
}

/// A record's text split into the label an app puts before its JSON object (`[HealthApp] event E1: `)
/// and the object itself; the label is empty for plain JSON. `None` when no object follows.
fn labelled_json(t: &str) -> Option<(&str, serde_json::Map<String, Value>)> {
    let at = t.find('{')?;
    match serde_json::from_str::<Value>(&t[at..]) {
        Ok(Value::Object(o)) => Some((&t[..at], o)),
        _ => None,
    }
}

/// What a raw record becomes when the app sends `new` for an id that holds `old`: the field
/// merge for two JSON objects under the same label, else the old text with the new one appended
/// (nothing if it is already in it). `None` only when there is nothing to merge (never for text).
fn merge_text(old: &str, new: &str) -> Option<String> {
    Some(match (labelled_json(old), labelled_json(new)) {
        (Some((l_old, mut held)), Some((l_new, fresh))) if l_old == l_new => {
            let before = held.clone();
            merge_json(&mut held, &fresh);
            // Nothing changed: keep the text as it was held, byte for byte.
            if held == before {
                return Some(old.to_string());
            }
            format!(
                "{l_new}{}",
                serde_json::to_string(&Value::Object(held)).ok()?
            )
        }
        _ if old.contains(new) => return Some(old.to_string()),
        _ if new.contains(old) => new.to_string(),
        _ => format!("{old}\n\n{new}"),
    })
}

/// Deposits a fact under the app's own id. See the module.
pub fn deposit(
    conn: &Connection,
    space: &Space,
    author: &str,
    mut new: NewFact,
    o: &Origin,
    now: i64,
) -> Result<Deposit> {
    let tag = o.tag();
    if !new.tags.contains(&tag) {
        new.tags.push(tag);
    }
    if let Some(code_tag) = o.code_tag()
        && !new.tags.contains(&code_tag)
    {
        new.tags.push(code_tag);
    }
    new.tags = facts::normalize_tags(&new.tags)?;
    facts::check_kind(new.kind.as_deref())?;
    let text = facts::check_text(&new.text)?;
    facts::check_size(&text)?;
    if let Some(old) = held(conn, o)? {
        if crate::text::text_hash(&old.text) == crate::text::text_hash(&text) {
            return Ok(Deposit::Unchanged(old));
        }
        // Raw records never lose what they held: the new reading is merged into it.
        let (text, new) = if o.layer == Layer::Source {
            match merge_text(&old.text, &text) {
                Some(merged) if merged == old.text => return Ok(Deposit::Unchanged(old)),
                Some(merged) => {
                    let merged = facts::check_text(&merged)?;
                    facts::check_size(&merged)?;
                    (
                        merged.clone(),
                        NewFact {
                            text: merged,
                            ..new
                        },
                    )
                }
                None => (text, new),
            }
        } else {
            (text, new)
        };
        if let Some(same) = facts::active_by_hash(conn, &crate::text::text_hash(&text))?
            && same.id != old.id
        {
            return Ok(Deposit::SameTextHeld(same));
        }
        let reason = format!("updated by {}", o.app);
        let fact = facts::supersede_as(
            conn,
            space,
            author,
            &old.id,
            new,
            Some(&reason),
            "supersede",
        )?;
        record(conn, &fact.id, o, now)?;
        return Ok(Deposit::Updated {
            fact,
            replaced: old.id,
        });
    }
    // A feed sends many alike facts (one per day, one per measure): no "close facts" question.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if let Some(same) = facts::active_by_hash(&tx, &crate::text::text_hash(&text))? {
        return Ok(Deposit::SameTextHeld(same));
    }
    let fact = facts::insert(&tx, space, author, &new, &text, None)?;
    record(&tx, &fact.id, o, now)?;
    tx.commit()?;
    Ok(Deposit::Created(fact))
}

/// The cursor of one source: its latest data point and how many facts of it are active.
pub fn cursor(conn: &Connection, app: &str) -> Result<Cursor> {
    cursor_in(conn, app, Layer::Source)
}

/// The cursor of one app in one layer.
pub fn cursor_in(conn: &Connection, app: &str, layer: Layer) -> Result<Cursor> {
    let (last_at, received, facts): (Option<String>, Option<i64>, i64) = conn.query_row(
        "SELECT max(o.at), max(f.created_at), count(*) FROM fact_origin o JOIN facts f ON f.id = o.fact_id
         WHERE o.app = ?1 AND o.layer = ?2 AND f.status = 'active'",
        params![app, layer.word()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(Cursor {
        app: app.to_string(),
        last_at,
        received_at: received.map(crate::util::iso),
        facts,
    })
}

/// Every source that deposited here, with its cursor.
pub fn sources(conn: &Connection) -> Result<Vec<Cursor>> {
    layer_cursors(conn, Layer::Source)
}

/// Every app that wrote into the dossier layer, with its cursor.
pub fn dossiers(conn: &Connection) -> Result<Vec<Cursor>> {
    layer_cursors(conn, Layer::Dossier)
}

fn layer_cursors(conn: &Connection, layer: Layer) -> Result<Vec<Cursor>> {
    let mut st =
        conn.prepare("SELECT DISTINCT app FROM fact_origin WHERE layer = ?1 ORDER BY app")?;
    let apps: Vec<String> = st
        .query_map([layer.word()], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    apps.iter().map(|a| cursor_in(conn, a, layer)).collect()
}
