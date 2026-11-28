use crate::{capture::text_hash, search, StoreError};
use rusqlite::Connection;

pub(super) const SQL: &str = "
    ALTER TABLE facts ADD COLUMN text_hash BLOB;
    DROP INDEX facts_origin;
    CREATE UNIQUE INDEX facts_origin ON facts(space, origin_app, origin_id)
        WHERE origin_app IS NOT NULL AND origin_id IS NOT NULL;

    ALTER TABLE duplicate_origins RENAME TO old_duplicate_origins;
    CREATE TABLE duplicate_origins (
        space TEXT NOT NULL REFERENCES spaces(name),
        origin_app TEXT NOT NULL,
        origin_id TEXT NOT NULL,
        fact_id TEXT NOT NULL REFERENCES facts(id),
        PRIMARY KEY (space, origin_app, origin_id)
    );
    INSERT INTO duplicate_origins
        SELECT facts.space, old.origin_app, old.origin_id, old.fact_id
        FROM old_duplicate_origins old JOIN facts ON facts.id = old.fact_id;
    DROP TABLE old_duplicate_origins;
    CREATE INDEX duplicate_origins_fact ON duplicate_origins(fact_id);

    CREATE INDEX facts_list_order ON facts(status, created_at);
    CREATE INDEX files_list_order ON files(status, created_at);

    CREATE TABLE cursor_secret (secret BLOB NOT NULL);
    INSERT INTO cursor_secret VALUES (randomblob(32));

    DELETE FROM fts;
    DELETE FROM search_facts;
    CREATE TABLE retired_search_facts (
        rowid INTEGER PRIMARY KEY,
        fact_id TEXT NOT NULL UNIQUE REFERENCES facts(id)
    );
    CREATE VIRTUAL TABLE retired_fts USING fts5(text);
";

pub(super) fn backfill(conn: &Connection) -> Result<(), StoreError> {
    fill_hashes(conn)?;
    conn.execute_batch(
        "CREATE INDEX facts_active_text_hash ON facts(space, text_hash) WHERE status = 'active';",
    )?;
    search::rebuild(conn)
}

fn fill_hashes(conn: &Connection) -> Result<(), StoreError> {
    let mut scan =
        conn.prepare("SELECT rowid, text FROM facts WHERE rowid > ?1 ORDER BY rowid LIMIT 512")?;
    let mut update = conn.prepare("UPDATE facts SET text_hash = ?1 WHERE rowid = ?2")?;
    let mut boundary = i64::MIN;
    loop {
        let batch = scan
            .query_map([boundary], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if batch.is_empty() {
            return Ok(());
        }
        for (rowid, text) in batch {
            update.execute((text_hash(&text).as_slice(), rowid))?;
            boundary = rowid;
        }
    }
}
