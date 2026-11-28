use super::Client;
use pensine_store::{RecallQuery, Store};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path};

pub fn check(dir: &Path, client: &Client) {
    let store = Store::open(dir).unwrap();
    let db = Connection::open(dir.join("store.db")).unwrap();
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    assert_empty(&db, "PRAGMA foreign_key_check");
    chains(&db);
    tags_and_search(&db);
    recall_modes(&store, &db);
    files(&store, &db, client);
    spaces(&db, client);
    assert_empty(
        &db,
        "
        SELECT origin_app, origin_id FROM facts WHERE origin_app IS NOT NULL
        GROUP BY space, origin_app, origin_id HAVING count(*) <> 1",
    );
    assert_empty(
        &db,
        "
        SELECT f.id FROM facts f WHERE f.origin_app = 'queue'
        AND (SELECT count(*) FROM facts WHERE space = f.space AND text = f.text) <> 1",
    );
}

fn assert_empty(db: &Connection, sql: &str) {
    assert!(
        !db.prepare(sql).unwrap().exists([]).unwrap(),
        "invariant failed: {sql}"
    );
}

fn chains(db: &Connection) {
    assert_empty(
        db,
        "
        SELECT replaces FROM facts WHERE replaces IS NOT NULL
        GROUP BY replaces HAVING count(*) <> 1",
    );
    assert_empty(
        db,
        "
        WITH RECURSIVE reachable(id) AS (
            SELECT id FROM facts WHERE replaces IS NULL
            UNION SELECT facts.id FROM facts JOIN reachable ON facts.replaces = reachable.id
        ) SELECT id FROM facts EXCEPT SELECT id FROM reachable",
    );
    assert_empty(
        db,
        "
        SELECT id FROM facts f
        WHERE (status = 'active' AND EXISTS(SELECT 1 FROM facts WHERE replaces = f.id))
           OR (status = 'superseded' AND NOT EXISTS(SELECT 1 FROM facts WHERE replaces = f.id))
           OR pinned <> (pinned_at IS NOT NULL)",
    );
    assert_empty(
        db,
        "
        SELECT name FROM spaces s WHERE 1 <> (
            SELECT count(*) FROM facts f JOIN fact_tags t ON t.fact_id = f.id
            WHERE f.space = s.name AND f.status = 'active' AND f.pinned = 1 AND t.tag = 'todo'
              AND NOT EXISTS(SELECT 1 FROM facts WHERE replaces = f.id)
        ) OR 1 <> (
            SELECT count(*) FROM facts f JOIN fact_tags t ON t.fact_id = f.id
            WHERE f.space = s.name AND f.status = 'active' AND t.tag = 'todo'
        )",
    );
}

fn tags_and_search(db: &Connection) {
    for (lhs, rhs) in [
        ("SELECT facts.id, tag.value FROM facts, json_each(tags_json) tag",
         "SELECT fact_id, tag FROM fact_tags"),
        ("SELECT id FROM facts WHERE status = 'active'",
         "SELECT search_facts.fact_id FROM search_facts JOIN fts ON fts.rowid = search_facts.rowid WHERE fts MATCH 'probe OR todo'"),
        ("SELECT id FROM facts f WHERE status = 'retired' AND NOT EXISTS(SELECT 1 FROM facts WHERE replaces = f.id)",
         "SELECT retired_search_facts.fact_id FROM retired_search_facts JOIN retired_fts ON retired_fts.rowid = retired_search_facts.rowid WHERE retired_fts MATCH 'probe OR todo'"),
    ] {
        assert_empty(db, &format!("{lhs} EXCEPT {rhs}"));
        assert_empty(db, &format!("{rhs} EXCEPT {lhs}"));
    }
    for (table, index) in [
        ("search_facts", "fts"),
        ("retired_search_facts", "retired_fts"),
    ] {
        assert_empty(
            db,
            &format!("SELECT fact_id FROM {table} GROUP BY fact_id HAVING count(*) <> 1"),
        );
        assert_empty(
            db,
            &format!("SELECT rowid FROM {index} EXCEPT SELECT rowid FROM {table}"),
        );
        assert_empty(db, &format!("SELECT rowid FROM {table} EXCEPT SELECT rowid FROM {index} WHERE {index} MATCH 'probe OR todo'"));
    }
}

fn recall_modes(store: &Store, db: &Connection) {
    for include_retired in [false, true] {
        let expected = db
            .prepare(
                "SELECT id FROM facts f WHERE text LIKE '% start'
               AND (status = 'active' OR (?1 AND status = 'retired'))
               AND NOT EXISTS(SELECT 1 FROM facts WHERE replaces = f.id)",
            )
            .unwrap()
            .query_map([include_retired], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .unwrap();
        let hits = store
            .recall(RecallQuery {
                text: "start".into(),
                include_retired,
                ..RecallQuery::default()
            })
            .unwrap();
        let found: BTreeSet<_> = hits.iter().map(|hit| hit.fact.id.clone()).collect();
        assert_eq!(hits.len(), found.len());
        assert_eq!(found, expected, "include_retired={include_retired}");
    }
}

fn files(store: &Store, db: &Connection, client: &Client) {
    let mut statement = db
        .prepare("SELECT id, size, sha256 FROM files WHERE status = 'active'")
        .unwrap();
    let active = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap();
    let mut total = 0;
    for row in active {
        let (id, size, sha) = row.unwrap();
        let (_, path) = store.file_get(&id).unwrap();
        let data = fs::read(path).unwrap();
        assert_eq!(data.len() as u64, size);
        assert_eq!(format!("{:x}", Sha256::digest(&data)), sha);
        total += size;
    }
    let report = store.sweep_blobs(false).unwrap();
    assert!(report.orphans.is_empty(), "orphan blobs: {report:?}");
    assert!(report.temporary.is_empty(), "staged uploads: {report:?}");
    assert_eq!(
        client.call("access", json!({})).unwrap()["file_storage"]["bytes"],
        total
    );
}

fn spaces(db: &Connection, client: &Client) {
    let reported = client.call("spaces", json!({})).unwrap();
    let reported = reported.as_array().unwrap();
    let mut query = db
        .prepare(
            "
        SELECT name,
           (SELECT count(*) FROM facts WHERE space = s.name AND status = 'active'),
           (SELECT count(*) FROM files WHERE space = s.name AND status = 'active')
        FROM spaces s",
        )
        .unwrap();
    let expected = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(reported.len(), expected.len());
    for (space, notes, files) in expected {
        let actual = reported.iter().find(|item| item["name"] == space).unwrap();
        assert_eq!(actual["notes"], notes);
        assert_eq!(actual["files"], files);
    }
}
