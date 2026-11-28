use crate::{NewFact, Store};
use rusqlite::Connection;
use std::time::Instant;

fn apply_schema(db: &Connection, version: usize) {
    for &(version, sql) in &crate::schema::MIGRATIONS[..version] {
        db.execute_batch(sql).unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
    }
}

fn previous_schema(db: &Connection, count: u32) {
    apply_schema(db, 5);
    db.execute_batch(
        "INSERT INTO spaces VALUES ('notes', 'Notes', NULL, 1);
         INSERT INTO facts (id, space, text, tags_json, author, source, created_at, status)
         VALUES ('original', 'notes', 'Original text', '[]', 'writer', 'capture', 1, 'superseded'),
                ('retired', 'notes', 'Retired text', '[]', 'writer', 'capture', 2, 'retired'),
                ('active', 'notes', 'Active text', '[]', 'writer', 'capture', 3, 'active');
         UPDATE facts SET replaces = 'original' WHERE id = 'retired';
         UPDATE facts SET origin_app = 'queue', origin_id = 'one' WHERE id = 'original';
         INSERT INTO duplicate_origins VALUES ('queue', 'two', 'active');",
    )
    .unwrap();
    db.execute(
        "WITH RECURSIVE seq(n) AS (SELECT 1 WHERE ?1 > 0 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1)
         INSERT INTO facts (id, space, text, tags_json, author, source, created_at, status)
         SELECT 'sample-' || n, 'notes', 'Sample text ' || n, '[]', 'writer', 'capture', n, 'active'
         FROM seq",
        [count],
    ).unwrap();
    crate::search::backfill(db).unwrap();
    crate::todo::backfill(db).unwrap();
}

fn count(store: &Store, sql: &str) -> u32 {
    store
        .reader()
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn the_hash_migration_fills_existing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Connection::open(dir.path().join("store.db")).unwrap();
    let tx = db.transaction().unwrap();
    previous_schema(&tx, 600);
    tx.commit().unwrap();
    drop(db);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM facts WHERE length(text_hash) = 32"
        ),
        604
    );
    let previous = store.read("active").unwrap();
    let new = NewFact {
        space: previous.space,
        text: previous.text,
        tags: previous.tags,
        author: previous.author,
        source: previous.source,
        at: None,
        origin: None,
    };
    assert_eq!(
        store.remember(new.clone()).unwrap().outcome,
        crate::Outcome::Duplicate
    );
    for (entry, expected) in [("one", "retired"), ("two", "active")] {
        let replay = store
            .remember(NewFact {
                origin: Some(("queue".into(), entry.into())),
                ..new.clone()
            })
            .unwrap();
        assert_eq!(replay.outcome, crate::Outcome::Replayed);
        assert_eq!(replay.fact.id, expected);
    }
}

#[test]
fn the_index_rebuild_migration_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let mut db = Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    previous_schema(&tx, 24_996);
    tx.commit().unwrap();
    drop(db);
    let start = Instant::now();
    for _ in 0..2 {
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM search_facts"), 24_998);
        assert_eq!(count(&store, "SELECT count(*) FROM fts"), 24_998);
        assert_eq!(count(&store, "SELECT count(*) FROM retired_fts"), 1);
    }
    eprintln!("25,000-row migration and reopen: {:?}", start.elapsed());
    assert!(start.elapsed().as_secs() < 15);
}

#[test]
fn superseding_removes_the_old_version_from_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    let note = NewFact {
        space: "notes".into(),
        text: "original".into(),
        tags: vec![],
        author: "writer".into(),
        source: "capture".into(),
        at: None,
        origin: None,
    };
    let original = store.remember(note.clone()).unwrap().fact;
    let edited = store
        .supersede(
            &original.id,
            NewFact {
                text: "edited".into(),
                ..note
            },
            "edit",
        )
        .unwrap();
    let tagged = store.tags_set(&edited.id, vec!["review".into()]).unwrap();
    assert_eq!(count(&store, "SELECT count(*) FROM search_facts"), 2);
    assert_eq!(count(&store, "SELECT count(*) FROM fts"), 2);
    store.retire(&tagged.id, "done").unwrap();
    assert_eq!(count(&store, "SELECT count(*) FROM fts"), 1);
    let restored = store.restore(&tagged.id).unwrap();
    assert_eq!(count(&store, "SELECT count(*) FROM fts"), 2);
    assert_eq!(count(&store, "SELECT count(*) FROM retired_fts"), 0);
    assert_eq!(store.history(&restored.id).unwrap().len(), 4);
}

#[test]
fn existing_spaces_get_a_todo_note_by_migration() {
    let dir = tempfile::tempdir().unwrap();
    let db = Connection::open(dir.path().join("store.db")).unwrap();
    apply_schema(&db, 4);
    db.execute_batch(
        "INSERT INTO spaces VALUES ('alpha', 'Alpha', NULL, 1), ('beta', 'Beta', NULL, 2);
         INSERT INTO facts (id, space, text, tags_json, author, source, created_at, status)
         VALUES ('kept', 'alpha', 'Kept note', '[\"ideas\"]', 'writer', 'capture', 1, 'active');
         INSERT INTO fact_tags VALUES ('kept', 'ideas');",
    )
    .unwrap();
    crate::search::backfill(&db).unwrap();
    drop(db);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.schema_version(), 6);
    let spaces = store.spaces().unwrap();
    for space in &spaces {
        let note = store.read(space.todo.as_deref().unwrap()).unwrap();
        assert_eq!(note.text, "# Todo\n\n");
        assert_eq!(note.tags, ["todo"]);
        assert_eq!(note.author, "system");
        assert_eq!(note.source, "system");
        assert!(note.pinned);
    }
    assert_eq!(spaces.iter().map(|space| space.notes).sum::<u64>(), 3);
    assert_eq!(
        store
            .list(crate::ListQuery {
                spaces: vec!["alpha".into()],
                tags_all: vec!["ideas".into()],
                ..crate::ListQuery::default()
            })
            .unwrap()
            .items
            .len(),
        1
    );
}
