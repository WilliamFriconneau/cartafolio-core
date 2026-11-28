use crate::{Fact, NewFact, StoreError};
use rusqlite::{params, Connection};

pub(crate) const TAG: &str = "todo";
const TEXT: &str = "# Todo\n\n";
const AUTHOR: &str = "system";
const SOURCE: &str = "system";
const PERMANENT: &str = "the todo note of a space is permanent: update it with supersede";
const UNIQUE: &str = "A space can only have one todo note";

pub(crate) fn permanent_error() -> StoreError {
    StoreError::Invalid(PERMANENT.into())
}

pub(crate) fn is_todo(fact: &Fact) -> bool {
    fact.tags.iter().any(|tag| tag == TAG)
}

/// Refuses a second active note carrying the todo tag in the same space.
pub(crate) fn ensure_single(
    conn: &Connection,
    space: &str,
    replacing: Option<&str>,
) -> Result<(), StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM facts f JOIN fact_tags t ON t.fact_id = f.id
             WHERE f.space = ?1 AND f.status = 'active' AND t.tag = ?2
               AND (?3 IS NULL OR f.id <> ?3)
         )",
        params![space, TAG, replacing],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        return Err(StoreError::Invalid(UNIQUE.into()));
    }
    Ok(())
}

pub(crate) fn insert(conn: &Connection, space: &str) -> Result<Fact, StoreError> {
    let mut fact = crate::fact::insert(
        conn,
        NewFact {
            space: space.into(),
            text: TEXT.into(),
            tags: vec![TAG.into()],
            author: AUTHOR.into(),
            source: SOURCE.into(),
            at: None,
            origin: None,
        },
        None,
    )?;
    pin(conn, &fact.id)?;
    fact.pinned = true;
    Ok(fact)
}

pub(crate) fn backfill(conn: &Connection) -> Result<(), StoreError> {
    let spaces = conn
        .prepare("SELECT name FROM spaces")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    for space in spaces {
        if !has_todo(conn, &space)? {
            insert(conn, &space)?;
        }
    }
    Ok(())
}

fn has_todo(conn: &Connection, space: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM facts f JOIN fact_tags t ON t.fact_id = f.id
             WHERE f.space = ?1 AND f.status = 'active' AND t.tag = ?2
         )",
        params![space, TAG],
        |row| row.get::<_, bool>(0),
    )?)
}

fn pin(conn: &Connection, id: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE facts SET pinned = 1, pinned_at = max(
             CAST(unixepoch('subsec') * 1000 AS INTEGER),
             coalesce((SELECT max(pinned_at) FROM facts WHERE pinned_at IS NOT NULL), 0) + 1
         ) WHERE id = ?1",
        [id],
    )?;
    Ok(())
}
