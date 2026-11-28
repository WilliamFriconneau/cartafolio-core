use crate::fact::{fact_from_row, insert, read, validate_time, COLUMNS};
use crate::tags::normalize_tags;
use crate::{Fact, NewFact, Outcome, Remembered, StoreError};
use rusqlite::{Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

pub(crate) fn text_hash(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

pub(crate) fn remember(tx: &Transaction<'_>, new: NewFact) -> Result<Remembered, StoreError> {
    if let Some(origin) = &new.origin {
        if let Some(id) = origin_fact_id(tx, &new.space, origin)? {
            return Ok(Remembered {
                fact: current(tx, &id)?,
                outcome: Outcome::Replayed,
            });
        }
    }

    if let Some(fact) = duplicate(tx, &new)? {
        validate_time(new.at)?;
        if let Some((app, entry)) = &new.origin {
            tx.execute(
                "INSERT INTO duplicate_origins (space, origin_app, origin_id, fact_id)
                 VALUES (?1, ?2, ?3, ?4)",
                (&new.space, app, entry, &fact.id),
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

fn duplicate(conn: &Connection, new: &NewFact) -> Result<Option<Fact>, StoreError> {
    let mut tags = normalize_tags(&new.tags)?;
    tags.sort_unstable();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM facts
         WHERE status = 'active' AND space = ?1 AND text_hash = ?2 AND text = ?3
         ORDER BY rowid"
    ))?;
    let mut matches = stmt.query((&new.space, text_hash(&new.text).as_slice(), &new.text))?;
    while let Some(row) = matches.next()? {
        let fact = fact_from_row(row)?;
        let mut existing = fact.tags.clone();
        existing.sort_unstable();
        if existing == tags {
            return Ok(Some(fact));
        }
    }
    Ok(None)
}

pub(crate) fn origin_fact_id(
    conn: &Connection,
    space: &str,
    (app, entry): &(String, String),
) -> Result<Option<String>, StoreError> {
    Ok(conn.query_row(
        "SELECT id FROM facts WHERE space = ?1 AND origin_app = ?2 AND origin_id = ?3
         UNION ALL
         SELECT fact_id FROM duplicate_origins WHERE space = ?1 AND origin_app = ?2 AND origin_id = ?3",
        (space, app, entry),
        |row| row.get(0),
    ).optional()?)
}

fn current(conn: &Connection, id: &str) -> Result<Fact, StoreError> {
    let id: String = conn.query_row(
        "WITH RECURSIVE chain(id) AS (
             SELECT ?1
             UNION ALL
             SELECT f.id FROM chain JOIN facts f ON f.replaces = chain.id
         )
         SELECT id FROM chain WHERE NOT EXISTS (SELECT 1 FROM facts WHERE replaces = chain.id)",
        [id],
        |row| row.get(0),
    )?;
    read(conn, &id)
}
