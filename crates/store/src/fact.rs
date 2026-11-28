use crate::tags::{fold_tag, normalize_tags};
use crate::{Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const COLUMNS: &str =
    "id, space, text, tags_json, author, source, origin_app, origin_id,
     created_at, replaces, status, reason";

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

#[derive(Clone, Debug, PartialEq, Eq)]
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Duplicate,
    Replayed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remembered {
    pub fact: Fact,
    pub outcome: Outcome,
}

impl Store {
    pub fn remember(&self, new: NewFact) -> Result<Remembered, StoreError> {
        self.write(|tx| remember(tx, new))
    }

    pub fn remember_many(&self, notes: Vec<NewFact>) -> Result<Vec<Remembered>, StoreError> {
        self.write(|tx| notes.into_iter().map(|new| remember(tx, new)).collect())
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

    pub fn supersede(&self, id: &str, new: NewFact, reason: &str) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let previous = head(tx, id, FactStatus::Active)?;
            if new.space != previous.space {
                return Err(StoreError::Invalid(
                    "A replacement must be in the same space".into(),
                ));
            }
            set_status(tx, id, FactStatus::Superseded, reason)?;
            insert(tx, new, Some(id))
        })
    }

    pub fn retire(&self, id: &str, reason: &str) -> Result<(), StoreError> {
        self.write(|tx| {
            head(tx, id, FactStatus::Active)?;
            set_status(tx, id, FactStatus::Retired, reason)
        })
    }

    pub fn restore(&self, id: &str) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let previous = head(tx, id, FactStatus::Retired)?;
            let tags = previous.tags.clone();
            insert(tx, revision(previous, tags), Some(id))
        })
    }

    pub fn tags_set(&self, id: &str, tags: Vec<String>) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let previous = head(tx, id, FactStatus::Active)?;
            set_status(tx, id, FactStatus::Superseded, "Tags updated")?;
            insert(tx, revision(previous, tags), Some(id))
        })
    }

    fn write<T>(
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

fn remember(tx: &Transaction<'_>, new: NewFact) -> Result<Remembered, StoreError> {
    if let Some(origin) = &new.origin {
        if let Some(id) = origin_fact_id(tx, origin)? {
            return Ok(Remembered {
                fact: read(tx, &id)?,
                outcome: Outcome::Replayed,
            });
        }
    }

    let duplicate = tx
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM facts WHERE status = 'active' AND space = ?1 AND text = ?2
                 ORDER BY rowid LIMIT 1"
            ),
            (&new.space, &new.text),
            fact_from_row,
        )
        .optional()?;
    if let Some(fact) = duplicate {
        validate_time(new.at)?;
        normalize_tags(&new.tags)?;
        if let Some((app, entry)) = &new.origin {
            tx.execute(
                "INSERT INTO duplicate_origins (origin_app, origin_id, fact_id) VALUES (?1, ?2, ?3)",
                (app, entry, &fact.id),
            )?;
        }
        return Ok(Remembered {
            fact,
            outcome: Outcome::Duplicate,
        });
    }
    Ok(Remembered {
        fact: insert(tx, new, None)?,
        outcome: Outcome::Created,
    })
}

fn origin_fact_id(
    conn: &Connection,
    (app, entry): &(String, String),
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM facts WHERE origin_app = ?1 AND origin_id = ?2
             UNION ALL
             SELECT fact_id FROM duplicate_origins WHERE origin_app = ?1 AND origin_id = ?2",
            (app, entry),
            |row| row.get(0),
        )
        .optional()?)
}

fn read(conn: &Connection, id: &str) -> Result<Fact, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM facts WHERE id = ?1"),
        [id],
        fact_from_row,
    )
    .optional()?
    .ok_or(StoreError::NotFound)
}

fn head(conn: &Connection, id: &str, status: FactStatus) -> Result<Fact, StoreError> {
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
    Ok(())
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

fn insert(conn: &Connection, new: NewFact, replaces: Option<&str>) -> Result<Fact, StoreError> {
    let created_at = validate_time(new.at)?;
    if new.text.chars().take(200_001).count() > 200_000 {
        return Err(StoreError::Invalid(
            "A note cannot exceed 200,000 characters".into(),
        ));
    }
    let tags = normalize_tags(&new.tags)?;
    let tags_json =
        serde_json::to_string(&tags).map_err(|error| StoreError::Invalid(error.to_string()))?;
    let space_exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM spaces WHERE name = ?1)",
        [&new.space],
        |row| row.get::<_, bool>(0),
    )?;
    if !space_exists {
        return Err(StoreError::NotFound);
    }
    if let Some(origin) = &new.origin {
        if replaces.is_some() && origin_fact_id(conn, origin)?.is_some() {
            return Err(StoreError::Conflict);
        }
    }
    let (app, entry) = new
        .origin
        .as_ref()
        .map_or((None, None), |(app, entry)| (Some(app), Some(entry)));
    let id: String = conn.query_row(
        "INSERT INTO facts (id, space, text, tags_json, author, source, origin_app, origin_id,
                            created_at, replaces, status)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active')
         RETURNING id",
        params![
            new.space, new.text, tags_json, new.author, new.source, app, entry, created_at,
            replaces
        ],
        |row| row.get(0),
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
    })
}

fn validate_time(at: Option<i64>) -> Result<i64, StoreError> {
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
    })
}
