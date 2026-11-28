#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{Fact, FactStatus, ListQuery, NewFact, Store, StoreError};
use rusqlite::Connection;
use tempfile::TempDir;

const PERMANENT: &str = "the todo note of a space is permanent: update it with supersede";

fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("alpha", "Alpha", None).unwrap();
    store.space_create("beta", "Beta", None).unwrap();
    (dir, store)
}

fn note(space: &str, text: &str, tags: &[&str]) -> NewFact {
    NewFact {
        space: space.into(),
        text: text.into(),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        author: "writer".into(),
        source: "test".into(),
        at: None,
        origin: None,
    }
}

fn todo(store: &Store, space: &str) -> Fact {
    let page = store
        .list(ListQuery {
            spaces: vec![space.into()],
            tags_all: vec!["todo".into()],
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(page.items.len(), 1, "{space}");
    page.items[0].clone()
}

#[test]
fn a_new_space_has_a_pinned_todo_note() {
    let (_dir, store) = store();
    let alpha = todo(&store, "alpha");
    assert_eq!(alpha.text, "# Todo\n\n");
    assert_eq!(alpha.tags, ["todo"]);
    assert_eq!(alpha.author, "system");
    assert_eq!(alpha.source, "system");
    assert_eq!(alpha.status, FactStatus::Active);
    assert!(alpha.pinned);
    assert!(alpha.created_at > 0);
    let spaces = store.spaces().unwrap();
    assert_eq!(spaces[0].todo.as_deref(), Some(alpha.id.as_str()));
    assert_eq!(
        spaces[1].todo.as_deref(),
        Some(todo(&store, "beta").id.as_str())
    );
}

#[test]
fn the_todo_note_cannot_be_retired_moved_unpinned_or_untagged() {
    let (_dir, store) = store();
    let alpha = todo(&store, "alpha");
    let retired = store.retire(&alpha.id, "Gone");
    assert!(
        matches!(retired, Err(StoreError::Invalid(ref message)) if message == PERMANENT),
        "{retired:?}"
    );
    let moved = store.move_facts(std::slice::from_ref(&alpha.id), "beta");
    assert!(
        matches!(moved, Err(StoreError::Invalid(ref message)) if message == PERMANENT),
        "{moved:?}"
    );
    let unpinned = store.pin(&alpha.id, false);
    assert!(
        matches!(unpinned, Err(StoreError::Invalid(ref message)) if message == PERMANENT),
        "{unpinned:?}"
    );
    let untagged = store.tags_set(&alpha.id, vec!["other".into()]);
    assert!(
        matches!(untagged, Err(StoreError::Invalid(ref message)) if message == PERMANENT),
        "{untagged:?}"
    );
    assert_eq!(store.read(&alpha.id).unwrap(), alpha);
    assert_eq!(
        store.spaces().unwrap()[0].todo.as_deref(),
        Some(alpha.id.as_str())
    );
}

#[test]
fn supersede_keeps_the_todo_note_pinned_and_tagged() {
    let (_dir, store) = store();
    let alpha = todo(&store, "alpha");
    let updated = store
        .supersede(
            &alpha.id,
            note("alpha", "# Todo\n\n- water the orchard", &[]),
            "Progress",
        )
        .unwrap();
    assert_eq!(updated.text, "# Todo\n\n- water the orchard");
    assert_eq!(updated.tags, ["todo"]);
    assert!(updated.pinned);
    assert_eq!(updated.replaces.as_deref(), Some(alpha.id.as_str()));
    let history = store.history(&alpha.id).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].status, FactStatus::Superseded);
    assert_eq!(history[1], updated);
    let kept = store
        .supersede(
            &updated.id,
            note("alpha", "# Todo\n\n- done", &["todo", "garden"]),
            "Progress",
        )
        .unwrap();
    assert_eq!(kept.tags, ["todo", "garden"]);
    assert!(kept.pinned);
    assert_eq!(
        store.spaces().unwrap()[0].todo.as_deref(),
        Some(kept.id.as_str())
    );
}

#[test]
fn a_second_todo_note_in_a_space_is_refused() {
    let (_dir, store) = store();
    let remembered = store.remember(note("alpha", "Another list", &["todo"]));
    assert!(
        matches!(remembered, Err(StoreError::Invalid(_))),
        "{remembered:?}"
    );
    let batch = store.remember_many(vec![
        note("alpha", "Kept note", &["ideas"]),
        note("alpha", "Second list", &["TODO"]),
    ]);
    assert!(matches!(batch, Err(StoreError::Invalid(_))), "{batch:?}");
    assert_eq!(
        store
            .list(ListQuery {
                spaces: vec!["alpha".into()],
                ..ListQuery::default()
            })
            .unwrap()
            .items
            .len(),
        1
    );
    let plain = store
        .remember(note("alpha", "Plain", &["ideas"]))
        .unwrap()
        .fact;
    let tagged = store.tags_set(&plain.id, vec!["ideas".into(), "todo".into()]);
    assert!(matches!(tagged, Err(StoreError::Invalid(_))), "{tagged:?}");
    let replaced = store.supersede(&plain.id, note("alpha", "Replaced", &["todo"]), "Change");
    assert!(
        matches!(replaced, Err(StoreError::Invalid(_))),
        "{replaced:?}"
    );
}

#[test]
fn existing_spaces_get_a_todo_note_by_migration() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).unwrap();
        store.space_create("alpha", "Alpha", None).unwrap();
        store.space_create("beta", "Beta", None).unwrap();
        store
            .remember(note("alpha", "Kept note", &["ideas"]))
            .unwrap();
    }
    let database = dir.path().join("store.db");
    {
        let conn = Connection::open(&database).unwrap();
        let ids: Vec<String> = conn
            .prepare("SELECT fact_id FROM fact_tags WHERE tag = 'todo'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for id in ids {
            for table in ["fts", "search_facts", "fact_tags"] {
                conn.execute(&format!("DELETE FROM {table} WHERE fact_id = ?1"), [&id])
                    .unwrap();
            }
            conn.execute("DELETE FROM facts WHERE id = ?1", [&id])
                .unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.schema_version(), 5);
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
            .list(ListQuery {
                spaces: vec!["alpha".into()],
                tags_all: vec!["ideas".into()],
                ..ListQuery::default()
            })
            .unwrap()
            .items
            .len(),
        1
    );
}
