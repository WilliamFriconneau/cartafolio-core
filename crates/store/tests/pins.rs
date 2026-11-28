#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{Fact, ListQuery, NewFact, RecallQuery, Store, StoreError};
use tempfile::TempDir;

fn setup() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    store.space_create("other", "Other", None).unwrap();
    (dir, store)
}

fn note(text: &str) -> NewFact {
    NewFact {
        space: "notes".into(),
        text: text.into(),
        tags: vec!["garden".into()],
        author: "fixture".into(),
        source: "test".into(),
        at: None,
        origin: None,
    }
}

fn save(store: &Store, text: &str) -> Fact {
    store.remember(note(text)).unwrap().fact
}

#[test]
fn pin_is_an_in_place_change_with_no_new_version() {
    let (dir, store) = setup();
    let mut fact = save(&store, "Track the garden");
    assert!(!fact.pinned);
    fact.pinned = true;
    assert_eq!(store.pin(&fact.id, true).unwrap(), fact);
    assert_eq!(store.history(&fact.id).unwrap(), [fact.clone()]);
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.read(&fact.id).unwrap(), fact);
    fact.pinned = false;
    assert_eq!(store.pin(&fact.id, false).unwrap(), fact);
    assert_eq!(store.history(&fact.id).unwrap(), [fact]);
}

#[test]
fn a_pin_survives_supersede_tags_set_restore_and_move() {
    let (_dir, store) = setup();
    let original = save(&store, "Track the garden");
    store.pin(&original.id, true).unwrap();
    let replacement = store
        .supersede(&original.id, note("Track the orchard"), "Correction")
        .unwrap();
    assert!(replacement.pinned);
    let tagged = store
        .tags_set(&replacement.id, vec!["garden".into()])
        .unwrap();
    assert!(tagged.pinned);
    store.retire(&tagged.id, "Pause").unwrap();
    let restored = store.restore(&tagged.id).unwrap();
    assert!(restored.pinned);
    let moved = store.move_facts(&[restored.id], "other").unwrap();
    assert!(moved[0].pinned);
    assert_eq!(moved[0].space, "other");
    assert!(store
        .list(ListQuery {
            pinned: Some(true),
            tags_any: vec!["garden".into()],
            ..ListQuery::default()
        })
        .unwrap()
        .items
        .iter()
        .any(|fact| fact.id == moved[0].id));
}

#[test]
fn only_the_head_can_be_pinned() {
    let (_dir, store) = setup();
    let original = save(&store, "Track the garden");
    let head = store.tags_set(&original.id, vec![]).unwrap();
    assert!(matches!(
        store.pin("missing", true),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.pin(&original.id, true),
        Err(StoreError::Conflict)
    ));
    store.retire(&head.id, "Pause").unwrap();
    assert!(matches!(
        store.pin(&head.id, false),
        Err(StoreError::Conflict)
    ));
}

#[test]
fn list_pinned_returns_newest_pin_first() {
    let (_dir, store) = setup();
    let older = save(&store, "Track the orchard");
    let newer = save(&store, "Track the garden");
    let unpinned = save(&store, "Track the meadow");
    store.pin(&newer.id, true).unwrap();
    store.pin(&older.id, true).unwrap();
    let mut query = ListQuery {
        pinned: Some(true),
        tags_any: vec!["garden".into()],
        limit: 1,
        ..ListQuery::default()
    };
    let first = store.list(query.clone()).unwrap();
    assert_eq!(first.items[0].id, older.id);
    query.cursor = Some(first.next_cursor.unwrap());
    let second = store.list(query).unwrap();
    assert_eq!(second.items[0].id, newer.id);
    assert!(second.next_cursor.is_none());
    assert_eq!(
        store
            .list(ListQuery {
                pinned: Some(false),
                tags_any: vec!["garden".into()],
                ..ListQuery::default()
            })
            .unwrap()
            .items,
        [unpinned]
    );
}

#[test]
fn a_pinned_note_ranks_above_an_equal_unpinned_one() {
    let (_dir, store) = setup();
    let older = save(&store, "orchard east");
    let newer = save(&store, "orchard west");
    let query = RecallQuery {
        text: "orchard".into(),
        ..RecallQuery::default()
    };
    let before = store.recall(query.clone()).unwrap();
    assert_eq!(before[0].fact.id, newer.id);
    assert_eq!(before[0].score, before[1].score);
    store.pin(&older.id, true).unwrap();
    let hits = store.recall(RecallQuery { limit: 1, ..query }).unwrap();
    assert_eq!(hits[0].fact.id, older.id);
    assert!(hits[0].fact.pinned);
    assert_eq!(hits[0].score, before[1].score * 1.5);
}
