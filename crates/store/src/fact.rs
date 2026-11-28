use crate::capture::{origin_fact_id, remember, text_hash};
use crate::tags::{fold_tag, normalize_tags};
use crate::{Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const COLUMNS: &str =
    "id, space, text, tags_json, author, source, origin_app, origin_id,
     created_at, replaces, status, reason, pinned";

#[derive(Clone, Debug)]
pub struct NewFact {
    pub space: String,
    pub text: String,
    pub tags: Vec<String>,
    pub author: String,
    pub source: String,
    pub at: Option<i64>,
    pub origin: Option<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Fact {
    pub id: String,
    pub space: String,
    pub text: String,
    pub tags: Vec<String>,
    pub author: String,
    pub source: String,
    pub origin: Option<(String, String)>,
    pub created_at: i64,
    pub replaces: Option<String>,
    pub status: FactStatus,
    pub reason: Option<String>,
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FactStatus {
    Active,
    Superseded,
    Retired,
}

impl FactStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Retired => "retired",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Created,
    Duplicate,
    Replayed,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Remembered {
    pub fact: Fact,
    pub outcome: Outcome,
}

impl Store {
    pub fn remember(&self, new: NewFact) -> Result<Remembered, StoreError> {
        self.write(|tx| remember(tx, new))
    }

    pub fn remember_many(&self, notes: Vec<NewFact>) -> Result<Vec<Remembered>, StoreError> {
        self.remember_many_checked(notes, |_| Ok(()))
    }

    /// Checks each result before committing the batch, including origin replays.
    pub fn remember_many_checked(
        &self,
        notes: Vec<NewFact>,
        check: impl Fn(&Remembered) -> Result<(), StoreError>,
    ) -> Result<Vec<Remembered>, StoreError> {
        self.write(|tx| {
            notes
                .into_iter()
                .map(|new| {
                    let result = remember(tx, new)?;
                    check(&result)?;
                    Ok(result)
                })
                .collect()
        })
    }

    pub fn read(&self, id: &str) -> Result<Fact, StoreError> {
        let conn = self.reader()?;
        read(&conn, id)
    }

    pub fn history(&self, id: &str) -> Result<Vec<Fact>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "WITH RECURSIVE ancestors(id, replaces) AS (
                SELECT id, replaces FROM facts WHERE id = ?1
                UNION ALL
                SELECT f.id, f.replaces FROM facts f JOIN ancestors a ON f.id = a.replaces
             ), chain(id, depth) AS (
                SELECT id, 0 FROM ancestors WHERE replaces IS NULL
                UNION ALL
                SELECT f.id, c.depth + 1 FROM facts f JOIN chain c ON f.replaces = c.id
             )
             SELECT {COLUMNS} FROM facts JOIN chain USING (id) ORDER BY chain.depth"
        ))?;
        let facts = stmt
            .query_map([id], fact_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        if facts.is_empty() {
            return Err(StoreError::NotFound);
        }
        Ok(facts)
    }

    pub fn supersede(&self, id: &str, mut new: NewFact, reason: &str) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let previous = head(tx, id, FactStatus::Active)?;
            if new.space != previous.space {
                return Err(StoreError::Invalid(
                    "A replacement must be in the same space".into(),
                ));
            }
            if crate::todo::is_todo(&previous) {
                let mut tags = normalize_tags(&new.tags)?;
                if !tags.iter().any(|tag| tag == crate::todo::TAG) {
                    tags.push(crate::todo::TAG.into());
                }
                new.tags = tags;
            }
            set_status(tx, id, FactStatus::Superseded, reason)?;
            insert(tx, new, Some(id))
        })
    }

    pub fn retire(&self, id: &str, reason: &str) -> Result<(), StoreError> {
        self.write(|tx| {
            let fact = head(tx, id, FactStatus::Active)?;
            if crate::todo::is_todo(&fact) {
                return Err(crate::todo::permanent_error());
            }
            set_status(tx, id, FactStatus::Retired, reason)?;
            crate::search::index_retired(tx, &fact.id, &fact.text, &fact.tags)
        })
    }

    pub fn restore(&self, id: &str) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let previous = head(tx, id, FactStatus::Retired)?;
            let tags = previous.tags.clone();
            crate::search::remove_retired(tx, id)?;
            insert(tx, revision(previous, tags), Some(id))
        })
    }

    pub fn tags_set(&self, id: &str, tags: Vec<String>) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let previous = head(tx, id, FactStatus::Active)?;
            let tags = normalize_tags(&tags)?;
            if crate::todo::is_todo(&previous) && !tags.iter().any(|tag| tag == crate::todo::TAG) {
                return Err(crate::todo::permanent_error());
            }
            set_status(tx, id, FactStatus::Superseded, "Tags updated")?;
            insert(tx, revision(previous, tags), Some(id))
        })
    }

    pub fn move_facts(&self, ids: &[String], to_space: &str) -> Result<Vec<Fact>, StoreError> {
        self.write(|tx| {
            crate::space::require_space(tx, to_space)?;
            ids.iter()
                .map(|id| {
                    let previous = head(tx, id, FactStatus::Active)?;
                    if crate::todo::is_todo(&previous) {
                        return Err(crate::todo::permanent_error());
                    }
                    let tags = previous.tags.clone();
                    let mut new = revision(previous, tags);
                    new.space = to_space.to_owned();
                    set_status(tx, id, FactStatus::Retired, "moved")?;
                    insert(tx, new, Some(id))
                })
                .collect()
        })
    }

    pub(crate) fn write<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut writer = self.writer.lock().map_err(|_| StoreError::Conflict)?;
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = operation(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}

pub(crate) fn read(conn: &Connection, id: &str) -> Result<Fact, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM facts WHERE id = ?1"),
        [id],
        fact_from_row,
    )
    .optional()?
    .ok_or(StoreError::NotFound)
}

pub(crate) fn head(conn: &Connection, id: &str, status: FactStatus) -> Result<Fact, StoreError> {
    let fact = read(conn, id)?;
    if fact.status != status
        || conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM facts WHERE replaces = ?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Err(StoreError::Conflict);
    }
    Ok(fact)
}

fn set_status(
    conn: &Connection,
    id: &str,
    status: FactStatus,
    reason: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE facts SET status = ?1, reason = ?2 WHERE id = ?3",
        (status.as_str(), reason, id),
    )?;
    crate::search::remove(conn, id)
}

fn revision(previous: Fact, tags: Vec<String>) -> NewFact {
    NewFact {
        space: previous.space,
        text: previous.text,
        tags,
        author: previous.author,
        source: previous.source,
        at: None,
        origin: None,
    }
}

pub(crate) fn insert(
    conn: &Connection,
    new: NewFact,
    replaces: Option<&str>,
) -> Result<Fact, StoreError> {
    crate::space::require_space(conn, &new.space)?;
    let created_at = validate_time(new.at)?;
    if new.text.chars().take(200_001).count() > 200_000 {
        return Err(StoreError::Invalid(
            "A note cannot exceed 200,000 characters".into(),
        ));
    }
    let tags = normalize_tags(&new.tags)?;
    if tags.iter().any(|tag| tag == crate::todo::TAG) {
        crate::todo::ensure_single(conn, &new.space, replaces)?;
    }
    let tags_json =
        serde_json::to_string(&tags).map_err(|error| StoreError::Invalid(error.to_string()))?;
    if let Some(origin) = &new.origin {
        if replaces.is_some() && origin_fact_id(conn, &new.space, origin)?.is_some() {
            return Err(StoreError::Conflict);
        }
    }
    let (app, entry) = new
        .origin
        .as_ref()
        .map_or((None, None), |(app, entry)| (Some(app), Some(entry)));
    let (id, pinned): (String, bool) = conn.query_row(
        "INSERT INTO facts (id, space, text, tags_json, author, source, origin_app, origin_id,
                            created_at, replaces, status, pinned, pinned_at, text_hash)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active',
                 coalesce((SELECT pinned FROM facts WHERE id = ?9), 0),
                 (SELECT pinned_at FROM facts WHERE id = ?9), ?10)
         RETURNING id, pinned",
        params![
            new.space,
            new.text,
            tags_json,
            new.author,
            new.source,
            app,
            entry,
            created_at,
            replaces,
            text_hash(&new.text).as_slice()
        ],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut insert_tag = conn.prepare_cached(
        "INSERT INTO fact_tags (fact_id, tag) VALUES (?1, ?2)
         ON CONFLICT (fact_id, tag) DO NOTHING",
    )?;
    for tag in &tags {
        insert_tag.execute((&id, fold_tag(tag)))?;
    }
    crate::search::index(conn, &id, &new.text, &tags)?;
    Ok(Fact {
        id,
        space: new.space,
        text: new.text,
        tags,
        author: new.author,
        source: new.source,
        origin: new.origin,
        created_at,
        replaces: replaces.map(str::to_owned),
        status: FactStatus::Active,
        reason: None,
        pinned,
    })
}

pub(crate) fn validate_time(at: Option<i64>) -> Result<i64, StoreError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| i64::try_from(time.as_secs()).ok())
        .ok_or_else(|| StoreError::Invalid("System clock is out of range".into()))?;
    let at = at.unwrap_or(now);
    if at > now.saturating_add(300) {
        return Err(StoreError::Invalid(
            "Capture time cannot be more than five minutes in the future".into(),
        ));
    }
    Ok(at)
}

pub(crate) fn fact_from_row(row: &Row<'_>) -> rusqlite::Result<Fact> {
    let tags_json: String = row.get(3)?;
    let tags = serde_json::from_str(&tags_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let status: String = row.get(10)?;
    let status = match status.as_str() {
        "active" => FactStatus::Active,
        "superseded" => FactStatus::Superseded,
        "retired" => FactStatus::Retired,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(Fact {
        id: row.get(0)?,
        space: row.get(1)?,
        text: row.get(2)?,
        tags,
        author: row.get(4)?,
        source: row.get(5)?,
        origin: row.get::<_, Option<String>>(6)?.zip(row.get(7)?),
        created_at: row.get(8)?,
        replaces: row.get(9)?,
        status,
        reason: row.get(11)?,
        pinned: row.get(12)?,
    })
}
