#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{FactStatus, NewFact, RecallQuery, Store, StoreError};

#[test]
fn a_child_space_lists_under_its_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("root", "Root", None).unwrap();
    store.space_create("other", "Other", None).unwrap();
    store.space_create("child", "Été 🌱", Some("root")).unwrap();
    let spaces = store.spaces().unwrap();
    assert_eq!(
        spaces.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["root", "child", "other"]
    );
    assert_eq!(spaces[1].label, "Été 🌱");
    assert_eq!(spaces[1].parent.as_deref(), Some("root"));
    assert_eq!((spaces[1].notes, spaces[1].files), (0, 0));
    assert!(matches!(
        store.space_create("child", "Duplicate", None),
        Err(StoreError::Conflict)
    ));
    assert!(matches!(
        store.space_create("orphan", "Orphan", Some("missing")),
        Err(StoreError::NotFound)
    ));
    store
        .space_move("child", Some("Renamed"), Some(None))
        .unwrap();
    let child = store
        .spaces()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "child")
        .unwrap();
    assert_eq!(child.label, "Renamed");
    assert_eq!(child.parent, None);
}

#[test]
fn a_cycle_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("root", "Root", None).unwrap();
    store.space_create("child", "Child", Some("root")).unwrap();
    for parent in ["root", "child"] {
        assert!(matches!(
            store.space_move("root", Some("Changed"), Some(Some(parent))),
            Err(StoreError::Invalid(_))
        ));
    }
    let spaces = store.spaces().unwrap();
    assert_eq!(spaces[0].label, "Root");
    assert_eq!(spaces[0].parent, None);
}

#[test]
fn moving_notes_keeps_their_history_and_tags() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("source", "Source", None).unwrap();
    store.space_create("target", "Target", None).unwrap();
    let mut note = NewFact {
        space: "missing".into(),
        text: "Orchard".into(),
        tags: vec!["Été".into()],
        author: "writer".into(),
        source: "capture".into(),
        at: None,
        origin: None,
    };
    assert!(matches!(
        store.remember(note.clone()),
        Err(StoreError::NotFound)
    ));
    note.space = "source".into();
    let old = store.remember(note).unwrap().fact;
    assert!(matches!(
        store.move_facts(&[old.id.clone(), "missing".into()], "target"),
        Err(StoreError::NotFound)
    ));
    assert_eq!(store.read(&old.id).unwrap(), old);
    let moved = store
        .move_facts(std::slice::from_ref(&old.id), "target")
        .unwrap();
    let history = store.history(&moved[0].id).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].status, FactStatus::Retired);
    assert_eq!(history[0].reason.as_deref(), Some("moved"));
    assert_eq!(history[1], moved[0]);
    assert_eq!(moved[0].space, "target");
    assert_eq!(moved[0].tags, old.tags);
    assert_eq!(moved[0].text, old.text);
    let spaces = store.spaces().unwrap();
    assert_eq!((spaces[0].notes, spaces[1].notes), (0, 1));
    let hits = store
        .recall(RecallQuery {
            text: "orchard".into(),
            ..RecallQuery::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fact, moved[0]);
}

#[test]
fn a_name_with_a_slash_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    for name in ["root/child", "a", "Été", "ROOT", &"a".repeat(41)] {
        assert!(matches!(
            store.space_create(name, "Label", None),
            Err(StoreError::Invalid(_))
        ));
    }
}
