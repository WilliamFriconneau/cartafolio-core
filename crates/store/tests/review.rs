#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{FactStatus, ListQuery, NewFact, Outcome, RecallQuery, Store};
use std::time::Instant;

fn note(text: &str, tags: &[&str]) -> NewFact {
    NewFact {
        space: "notes".into(),
        text: text.into(),
        tags: tags.iter().map(|tag| (*tag).into()).collect(),
        author: "writer".into(),
        source: "capture".into(),
        at: None,
        origin: None,
    }
}

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    (dir, store)
}

fn capture() -> NewFact {
    NewFact {
        origin: Some(("queue".into(), "entry".into())),
        ..note("original", &[])
    }
}

#[test]
fn a_replay_after_supersede_returns_the_head() {
    let (_dir, store) = open();
    let first = store.remember(capture()).unwrap().fact;
    let head = store
        .supersede(&first.id, note("corrected", &[]), "edit")
        .unwrap();
    let replay = store.remember(capture()).unwrap();
    assert_eq!(replay.outcome, Outcome::Replayed);
    assert_eq!(replay.fact, head);
}

#[test]
fn a_replay_after_retire_returns_the_retired_head() {
    let (_dir, store) = open();
    let first = store.remember(capture()).unwrap().fact;
    let head = store
        .supersede(&first.id, note("corrected", &[]), "edit")
        .unwrap();
    store.retire(&head.id, "done").unwrap();
    let replay = store.remember(capture()).unwrap();
    assert_eq!(replay.outcome, Outcome::Replayed);
    assert_eq!(replay.fact.id, head.id);
    assert_eq!(replay.fact.status, FactStatus::Retired);
}

#[test]
fn same_text_with_other_tags_is_a_new_note() {
    let (_dir, store) = open();
    store.remember(note("buy milk", &["alpha"])).unwrap();
    let second = store.remember(note("buy milk", &["beta"])).unwrap();
    assert_eq!(second.outcome, Outcome::Created);
    assert_eq!(second.fact.tags, ["beta"]);
}

#[test]
fn same_text_and_same_tags_is_a_duplicate() {
    let (_dir, store) = open();
    let first = store
        .remember(note("buy milk", &["école", "beta"]))
        .unwrap();
    let second = store
        .remember(note("buy milk", &["BETA", "E\u{301}COLE", "beta"]))
        .unwrap();
    assert_eq!(second.outcome, Outcome::Duplicate);
    assert_eq!(second.fact.id, first.fact.id);
}

#[test]
fn inserts_stay_linear_with_the_hash_index() {
    let (_dir, store) = open();
    let timings: Vec<_> = (0..2)
        .map(|batch| {
            let notes = (0..10_000)
                .map(|i| note(&format!("unique note {batch} {i}"), &[]))
                .collect();
            let start = Instant::now();
            store.remember_many(notes).unwrap();
            start.elapsed()
        })
        .collect();
    eprintln!("Insert batches: {timings:?}");
    assert!(timings[1] <= timings[0].mul_f64(2.5));
}

#[test]
fn include_retired_returns_only_retired_heads() {
    let (_dir, store) = open();
    let first = store.remember(note("review original", &[])).unwrap().fact;
    let head = store
        .supersede(&first.id, note("review corrected", &[]), "edit")
        .unwrap();
    store.retire(&head.id, "done").unwrap();
    let hits = store
        .recall(RecallQuery {
            text: "review".into(),
            include_retired: true,
            ..RecallQuery::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fact.id, head.id);
}

#[test]
fn an_origin_in_a_hidden_space_is_not_observable() {
    let (_dir, store) = open();
    store.space_create("hidden", "Hidden", None).unwrap();
    store
        .remember(NewFact {
            space: "hidden".into(),
            ..capture()
        })
        .unwrap();
    let result = store.remember(capture()).unwrap();
    assert_eq!(result.outcome, Outcome::Created);
    assert_eq!(result.fact.space, "notes");
}

fn page_query() -> ListQuery {
    ListQuery {
        spaces: vec!["notes".into()],
        limit: 1,
        ..ListQuery::default()
    }
}

#[test]
fn cursors_are_opaque_and_tamper_proof() {
    let (_dir, store) = open();
    store.remember(note("first", &[])).unwrap();
    store.remember(note("second", &[])).unwrap();
    let cursor = store.list(page_query()).unwrap().next_cursor.unwrap();
    assert!(!cursor.contains('.'));
    let mutated = format!(
        "{}{}",
        if cursor.starts_with('0') { '1' } else { '0' },
        &cursor[1..]
    );
    assert!(store
        .list(ListQuery {
            cursor: Some(mutated),
            ..page_query()
        })
        .is_err());
    assert!(store
        .list(ListQuery {
            cursor: Some(cursor),
            spaces: vec![],
            ..page_query()
        })
        .is_err());
}

#[test]
fn cursors_do_not_depend_on_invisible_rows() {
    let (_dir, store) = open();
    store.space_create("hidden", "Hidden", None).unwrap();
    store.remember(note("first", &[])).unwrap();
    store.remember(note("second", &[])).unwrap();
    let before = store.list(page_query()).unwrap();
    store
        .remember(NewFact {
            space: "hidden".into(),
            ..note("outside", &[])
        })
        .unwrap();
    assert_eq!(store.list(page_query()).unwrap(), before);
}

#[test]
fn recall_hides_the_score() {
    let shapes: Vec<_> = ["review private", "unrelated words"]
        .map(|hidden| {
            let (_dir, store) = open();
            store.space_create("hidden", "Hidden", None).unwrap();
            store.remember(note("review visible", &[])).unwrap();
            store
                .remember(NewFact {
                    space: "hidden".into(),
                    ..note(hidden, &[])
                })
                .unwrap();
            let hits = store
                .recall(RecallQuery {
                    text: "review".into(),
                    spaces: vec!["notes".into()],
                    ..RecallQuery::default()
                })
                .unwrap();
            assert_eq!(hits.len(), 1);
            let hit = serde_json::to_value(&hits[0]).unwrap();
            hit.as_object().unwrap().keys().cloned().collect::<Vec<_>>()
        })
        .into();
    assert_eq!(shapes, vec![vec!["fact", "snippet"]; 2]);
}

#[test]
fn file_cursors_are_scoped_and_ignore_hidden_uploads() {
    let (_dir, store) = open();
    let (_other_dir, other) = open();
    store.space_create("hidden", "Hidden", None).unwrap();
    let save = |space: &str, name: &str| {
        store
            .file_put(pensine_store::NewFile {
                space: space.into(),
                name: name.into(),
                mime: "text/plain".into(),
                reader: &b"sample"[..],
                max_size: 100,
                note: None,
            })
            .unwrap();
    };
    save("notes", "first.txt");
    save("notes", "second.txt");
    let page = store.file_list(page_query()).unwrap();
    save("hidden", "hidden.txt");
    assert_eq!(store.file_list(page_query()).unwrap(), page);

    let cursor = page.next_cursor.unwrap();
    let query = ListQuery {
        cursor: Some(cursor.clone()),
        ..page_query()
    };
    assert_eq!(store.file_list(query.clone()).unwrap().items.len(), 1);
    assert!(store.list(query.clone()).is_err());
    assert!(other.file_list(query.clone()).is_err());
    let mutated = format!(
        "{}{}",
        if cursor.starts_with('A') { 'B' } else { 'A' },
        &cursor[1..]
    );
    assert!(store
        .file_list(ListQuery {
            cursor: Some(mutated),
            ..query
        })
        .is_err());
}
