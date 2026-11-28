use crate::StoreError;
use rusqlite::{Connection, TransactionBehavior};

pub(super) const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        "
    CREATE TABLE spaces (
        name TEXT PRIMARY KEY NOT NULL,
        label TEXT NOT NULL,
        parent TEXT REFERENCES spaces(name),
        position INTEGER NOT NULL
    );

    CREATE TABLE facts (
        id TEXT PRIMARY KEY NOT NULL,
        space TEXT NOT NULL REFERENCES spaces(name),
        text TEXT NOT NULL,
        tags_json TEXT NOT NULL,
        author TEXT NOT NULL,
        source TEXT NOT NULL,
        origin_app TEXT,
        origin_id TEXT,
        created_at INTEGER NOT NULL,
        replaces TEXT REFERENCES facts(id),
        status TEXT NOT NULL,
        reason TEXT
    );

    CREATE UNIQUE INDEX facts_origin ON facts(origin_app, origin_id)
        WHERE origin_app IS NOT NULL AND origin_id IS NOT NULL;

    CREATE TABLE fact_tags (
        fact_id TEXT NOT NULL REFERENCES facts(id),
        tag TEXT NOT NULL,
        PRIMARY KEY (fact_id, tag)
    );

    CREATE TABLE files (
        id TEXT PRIMARY KEY NOT NULL,
        space TEXT NOT NULL REFERENCES spaces(name),
        name TEXT NOT NULL,
        mime TEXT NOT NULL,
        size INTEGER NOT NULL,
        sha256 TEXT NOT NULL,
        note TEXT,
        created_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        reason TEXT
    );

    CREATE VIRTUAL TABLE fts USING fts5(fact_id UNINDEXED, text);
    ",
    ),
    (
        2,
        "
    CREATE UNIQUE INDEX facts_successor ON facts(replaces) WHERE replaces IS NOT NULL;
    CREATE INDEX facts_active_space ON facts(space) WHERE status = 'active';

    CREATE TABLE duplicate_origins (
        origin_app TEXT NOT NULL,
        origin_id TEXT NOT NULL,
        fact_id TEXT NOT NULL REFERENCES facts(id),
        PRIMARY KEY (origin_app, origin_id)
    );
    ",
    ),
];

pub(crate) fn migrate(conn: &mut Connection) -> Result<u32, StoreError> {
    let latest = MIGRATIONS.last().map_or(0, |(version, _)| *version);
    for &(version, sql) in MIGRATIONS {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current > latest {
            return Err(StoreError::Invalid(format!(
                "Unsupported store schema version {current}"
            )));
        }
        if current < version {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", version)?;
        }
        tx.commit()?;
    }
    Ok(latest)
}
