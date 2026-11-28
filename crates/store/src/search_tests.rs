use crate::{RecallQuery, Store};
use rusqlite::Connection;

#[test]
fn upgrading_from_version_2_indexes_existing_versions_once() {
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open(dir.path().join("store.db")).unwrap();
    for &(version, sql) in &crate::schema::MIGRATIONS[..2] {
        conn.execute_batch(sql).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
    }
    conn.execute_batch(
        "INSERT INTO spaces (name, label, position) VALUES ('notes', 'Notes', 1);
         INSERT INTO facts (id, space, text, tags_json, author, source, created_at, status)
         VALUES ('old', 'notes', 'Chevaux', '[]', 'writer', 'capture', 1, 'retired'),
                ('current', 'notes', 'Meadow', '[\"Chevaux\"]', 'writer', 'capture', 2, 'active');
         INSERT INTO fts (fact_id, text) VALUES ('old', 'stale');",
    )
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.schema_version(), 6);
        let mut hits = store
            .recall(RecallQuery {
                text: "cheval".into(),
                include_retired: true,
                ..RecallQuery::default()
            })
            .unwrap();
        assert!(hits.iter().all(|hit| !hit.fact.pinned));
        hits.sort_by(|left, right| left.fact.id.cmp(&right.fact.id));
        assert_eq!(
            hits.iter()
                .map(|hit| hit.fact.id.as_str())
                .collect::<Vec<_>>(),
            ["current", "old"]
        );
        let count: u32 = store
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM fts", [], |row| row.get(0))
            .unwrap();
        // The active note and the todo note that the upgrade adds to the space.
        assert_eq!(count, 2);
    }
}
