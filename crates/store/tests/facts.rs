#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{normalize_tag, FactStatus, ListQuery, NewFact, Outcome, Store, StoreError};
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    Connection::open(dir.path().join("store.db"))
        .unwrap()
        .execute_batch(
            "INSERT INTO spaces (name, label, position) VALUES ('notes', 'Notes', 1), ('other', 'Other', 2);",
        )
        .unwrap();
    (dir, store)
}

fn note(text: &str) -> NewFact {
    NewFact {
        space: "notes".into(),
        text: text.into(),
        tags: vec!["Ideas".into()],
        author: "writer".into(),
        source: "capture".into(),
        at: None,
        origin: None,
    }
}

#[test]
fn a_note_is_stored_and_read_back() {
    let (_dir, store) = store();
    let saved = store.remember(note("A fictional note")).unwrap();
    assert_eq!(saved.outcome, Outcome::Created);
    assert_eq!(saved.fact.text, "A fictional note");
    assert_eq!(saved.fact.author, "writer");
    assert_eq!(saved.fact.source, "capture");
    assert_eq!(saved.fact.status, FactStatus::Active);
    assert_eq!(store.read(&saved.fact.id).unwrap(), saved.fact);
    assert!(matches!(store.read("unknown"), Err(StoreError::NotFound)));
}

#[test]
fn an_exact_duplicate_is_not_written_twice() {
    let (_dir, store) = store();
    let first = store.remember(note("Same text")).unwrap();
    let duplicate = store.remember(note("Same text")).unwrap();
    assert_eq!(duplicate.outcome, Outcome::Duplicate);
    assert_eq!(duplicate.fact.id, first.fact.id);
    let mut other_space = note("Same text");
    other_space.space = "other".into();
    assert_eq!(
        store.remember(other_space).unwrap().outcome,
        Outcome::Created
    );
    store.retire(&first.fact.id, "Archived").unwrap();
    assert_eq!(
        store.remember(note("Same text")).unwrap().outcome,
        Outcome::Created
    );
}

#[test]
fn the_same_origin_replayed_writes_once() {
    let (_dir, store) = store();
    let mut original = note("Queue entry");
    original.origin = Some(("queue".into(), "entry-1".into()));
    let first = store.remember_many(vec![original.clone()]).unwrap();
    original.text = "Changed payload".into();
    let second = store.remember_many(vec![original]).unwrap();
    assert_eq!(second[0].outcome, Outcome::Replayed);
    assert_eq!(first[0].fact.id, second[0].fact.id);
    assert_eq!(store.list(ListQuery::default()).unwrap().items.len(), 1);

    let mut duplicate = note("Queue entry");
    duplicate.origin = Some(("queue".into(), "entry-2".into()));
    assert_eq!(
        store.remember(duplicate.clone()).unwrap().outcome,
        Outcome::Duplicate
    );
    store.retire(&first[0].fact.id, "Archived").unwrap();
    let replay = store.remember(duplicate).unwrap();
    assert_eq!(replay.outcome, Outcome::Replayed);
    assert_eq!(replay.fact.id, first[0].fact.id);
}

#[test]
fn a_capture_keeps_its_own_time_but_not_the_future() {
    let (_dir, store) = store();
    let mut offline = note("Offline capture");
    offline.at = Some(123);
    assert_eq!(store.remember(offline).unwrap().fact.created_at, 123);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut future = note("Future capture");
    future.at = Some(i64::try_from(now).unwrap() + 600);
    assert!(matches!(
        store.remember(future),
        Err(StoreError::Invalid(_))
    ));
}

#[test]
fn supersede_keeps_the_old_version_in_the_history() {
    let (_dir, store) = store();
    let first = store.remember(note("Draft")).unwrap().fact;
    let second = store
        .supersede(&first.id, note("Revision"), "Updated")
        .unwrap();
    let third = store.tags_set(&second.id, vec!["Final".into()]).unwrap();
    assert_eq!(second.replaces.as_deref(), Some(first.id.as_str()));
    assert_eq!(third.text, second.text);
    assert_eq!(third.tags, ["final"]);
    for id in [&first.id, &second.id, &third.id] {
        let history = store.history(id).unwrap();
        assert_eq!(
            history.iter().map(|fact| &fact.id).collect::<Vec<_>>(),
            [&first.id, &second.id, &third.id]
        );
        assert_eq!(history[0].status, FactStatus::Superseded);
        assert_eq!(history[0].reason.as_deref(), Some("Updated"));
    }
    assert!(matches!(
        store.retire(&first.id, "Old"),
        Err(StoreError::Conflict)
    ));
    assert!(matches!(
        store.supersede(&first.id, note("Branch"), "Old"),
        Err(StoreError::Conflict)
    ));
    assert!(matches!(
        store.tags_set(&first.id, vec![]),
        Err(StoreError::Conflict)
    ));
    assert_eq!(
        store.remember(note("Draft")).unwrap().outcome,
        Outcome::Created
    );
}

#[test]
fn retire_then_restore_gives_a_new_version() {
    let (_dir, store) = store();
    let first = store.remember(note("Keep for later")).unwrap().fact;
    assert!(matches!(
        store.restore(&first.id),
        Err(StoreError::Conflict)
    ));
    store.retire(&first.id, "Archived").unwrap();
    assert_eq!(store.read(&first.id).unwrap().status, FactStatus::Retired);
    assert!(matches!(
        store.retire(&first.id, "Again"),
        Err(StoreError::Conflict)
    ));
    let restored = store.restore(&first.id).unwrap();
    assert_eq!(restored.replaces.as_deref(), Some(first.id.as_str()));
    assert_eq!(restored.status, FactStatus::Active);
    assert_eq!(restored.text, first.text);
    assert_eq!(restored.tags, first.tags);
    assert!(matches!(
        store.restore(&first.id),
        Err(StoreError::Conflict)
    ));
    assert!(matches!(
        store.restore("unknown"),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn tags_are_normalised_and_bad_ones_refused() {
    let (_dir, store) = store();
    assert_eq!(
        normalize_tag("E\u{301}TE\u{301}/Projets_1").unwrap(),
        "été/projets_1"
    );
    for bad in [
        "",
        "a//b",
        "a/b/c/d/e",
        "a b",
        "🌱",
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmno",
    ] {
        assert!(matches!(normalize_tag(bad), Err(StoreError::Invalid(_))));
    }
    let mut tagged = note("With tags");
    tagged.tags = vec!["ÉTÉ".into(), "Projets".into(), "été".into()];
    assert_eq!(
        store.remember(tagged).unwrap().fact.tags,
        ["été", "projets"]
    );
    let page = store
        .list(ListQuery {
            spaces: vec!["notes".into()],
            tags_any: vec!["absent".into(), "ETE".into()],
            tags_all: vec!["ete".into(), "PROJETS".into()],
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(store
        .list(ListQuery {
            tags_all: vec!["ete".into(), "absent".into()],
            ..ListQuery::default()
        })
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn a_note_over_200000_characters_is_refused_with_a_clear_error() {
    let (_dir, store) = store();
    assert!(store.remember(note(&"é".repeat(200_000))).is_ok());
    let error = store.remember(note(&"é".repeat(200_001))).unwrap_err();
    assert!(
        matches!(error, StoreError::Invalid(message) if message.contains("200,000 characters"))
    );
}

#[test]
fn list_pages_are_stable_while_notes_are_added() {
    let (_dir, store) = store();
    for (text, at) in [("Oldest", 100), ("Middle", 200), ("Newest", 200)] {
        let mut new = note(text);
        new.at = Some(at);
        store.remember(new).unwrap();
    }
    let mut query = ListQuery {
        limit: 1,
        since: Some(100),
        until: Some(200),
        ..ListQuery::default()
    };
    let first = store.list(query.clone()).unwrap();
    assert_eq!(first.items[0].text, "Newest");
    for (text, at) in [("Backdated", 150), ("Later", 300)] {
        let mut new = note(text);
        new.at = Some(at);
        store.remember(new).unwrap();
    }
    query.cursor = first.next_cursor;
    let second = store.list(query.clone()).unwrap();
    assert_eq!(second.items[0].text, "Middle");
    query.cursor = second.next_cursor;
    let third = store.list(query).unwrap();
    assert_eq!(third.items[0].text, "Oldest");
    assert!(third.next_cursor.is_none());
}

#[test]
fn remember_many_is_all_or_nothing() {
    let (_dir, store) = store();
    let mut valid = note("Valid");
    valid.origin = Some(("queue".into(), "batch-1".into()));
    let mut invalid = note("Invalid");
    invalid.tags = vec!["bad tag".into()];
    assert!(matches!(
        store.remember_many(vec![valid.clone(), invalid]),
        Err(StoreError::Invalid(_))
    ));
    assert!(store.list(ListQuery::default()).unwrap().items.is_empty());
    assert_eq!(store.remember(valid).unwrap().outcome, Outcome::Created);
}
