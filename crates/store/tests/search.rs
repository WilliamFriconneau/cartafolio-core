#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{Hit, NewFact, RecallQuery, Store, StoreError};
use rusqlite::Connection;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    Connection::open(dir.path().join("store.db"))
        .unwrap()
        .execute_batch(
            "INSERT INTO spaces (name, label, position)
             VALUES ('notes', 'Notes', 1), ('other', 'Other', 2)",
        )
        .unwrap();
    (dir, store)
}

fn note(text: &str) -> NewFact {
    NewFact {
        space: "notes".into(),
        text: text.into(),
        tags: Vec::new(),
        author: "writer".into(),
        source: "capture".into(),
        at: None,
        origin: None,
    }
}

fn recall(store: &Store, text: &str) -> Vec<Hit> {
    store
        .recall(RecallQuery {
            text: text.into(),
            ..RecallQuery::default()
        })
        .unwrap()
}

#[test]
fn accents_and_plurals_do_not_matter() {
    let (_dir, store) = store();
    store.remember(note("Chevaux en été et pommes")).unwrap();
    assert_eq!(recall(&store, "cheval ete pomme").len(), 1);
    assert_eq!(recall(&store, "chevaux eté pommes").len(), 1);
}

#[test]
fn aux_plurals_fold_to_al() {
    let (_dir, store) = store();
    store.remember(note("Les journaux et les animaux")).unwrap();
    assert_eq!(recall(&store, "journal animal").len(), 1);
    assert_eq!(recall(&store, "journaux animaux").len(), 1);
}

#[test]
fn fts_syntax_in_a_query_is_inert() {
    let (_dir, store) = store();
    store.remember(note("orchard")).unwrap();
    for query in [
        "\"",
        "*",
        "-",
        "...",
        "orch*",
        "NEAR(orchard missing)",
        "orchard AND missing",
        "orchard OR missing",
        "text:orchard",
        "(orchard NOT missing)",
    ] {
        assert!(recall(&store, query).is_empty(), "{query}");
    }
    assert_eq!(recall(&store, "\"orchard").len(), 1);
    assert_eq!(recall(&store, "-orchard").len(), 1);
}

#[test]
fn a_query_never_returns_another_spaces_notes_when_spaces_are_given() {
    let (_dir, store) = store();
    let a = note("orchard");
    let mut b = a.clone();
    b.space = "other".into();
    store.remember_many(vec![a, b]).unwrap();
    for text in ["orchard", "\"orchard", "orchard*"] {
        let hits = store
            .recall(RecallQuery {
                text: text.into(),
                spaces: vec!["notes".into()],
                ..RecallQuery::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].fact.space, "notes");
    }
}

#[test]
fn retired_notes_are_not_found() {
    let (_dir, store) = store();
    let first = store.remember(note("orchard")).unwrap().fact;
    let next = store
        .supersede(&first.id, note("meadow"), "Updated")
        .unwrap();
    assert!(recall(&store, "orchard").is_empty());
    store.retire(&next.id, "Archived").unwrap();
    assert!(recall(&store, "meadow").is_empty());
    for (text, id) in [("orchard", &first.id), ("meadow", &next.id)] {
        let hits = store
            .recall(RecallQuery {
                text: text.into(),
                include_retired: true,
                ..RecallQuery::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(&hits[0].fact.id, id);
    }
    let restored = store.restore(&next.id).unwrap();
    assert_eq!(recall(&store, "meadow")[0].fact.id, restored.id);
}

#[test]
fn a_phrase_in_quotes_is_matched_as_words_all_present() {
    let (_dir, store) = store();
    store.remember(note("orchard beside river")).unwrap();
    store.remember(note("orchard beside meadow")).unwrap();
    let hits = recall(&store, "\"river orchard\"");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fact.text, "orchard beside river");
}

#[test]
fn tags_are_searchable_and_retagging_updates_the_active_result() {
    let (_dir, store) = store();
    let mut new = note("orchard");
    new.tags = vec!["Saisons/Été".into()];
    let first = store.remember(new).unwrap().fact;
    assert_eq!(recall(&store, "saison ete")[0].fact.id, first.id);
    let next = store.tags_set(&first.id, vec!["Hiver".into()]).unwrap();
    assert!(recall(&store, "ete").is_empty());
    assert_eq!(recall(&store, "hiver orchard")[0].fact.id, next.id);
    assert_eq!(recall(&store, "orchard").len(), 1);
}

#[test]
fn filters_and_recency_apply_before_the_result_limit() {
    let (_dir, store) = store();
    for (time, tag) in [(1, "été"), (2, "été"), (3, "été"), (4, "hiver")] {
        let mut new = note(&format!("orchard {time}"));
        new.at = Some(time);
        new.tags = vec![tag.into()];
        store.remember(new).unwrap();
    }
    let mut query = RecallQuery {
        text: "orchard".into(),
        tags_any: vec!["ETE".into()],
        since: Some(2),
        limit: 2,
        ..RecallQuery::default()
    };
    let hits = store.recall(query.clone()).unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| hit.fact.created_at)
            .collect::<Vec<_>>(),
        [3, 2]
    );
    query.limit = 51;
    assert!(matches!(store.recall(query), Err(StoreError::Invalid(_))));
}

#[test]
fn snippet_keeps_original_text_around_the_first_match() {
    let (_dir, store) = store();
    let text = format!("{}été {}été", "🪴".repeat(250), "🌱".repeat(250));
    store.remember(note(&text)).unwrap();
    let hits = recall(&store, "ete");
    let snippet = &hits[0].snippet;
    assert!(snippet.chars().count() <= 200);
    assert!(snippet.contains("🪴été 🌱"));
    assert_eq!(snippet.matches("été").count(), 1);
}

#[test]
fn a_failed_batch_leaves_no_index_rows() {
    let (dir, store) = store();
    let mut invalid = note("river");
    invalid.space = "missing".into();
    assert!(store.remember_many(vec![note("orchard"), invalid]).is_err());
    let conn = Connection::open(dir.path().join("store.db")).unwrap();
    let count: u32 = conn
        .query_row("SELECT count(*) FROM fts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
#[ignore = "Wall-clock benchmark; run explicitly with PENSINE_BENCH=1"]
fn ten_thousand_notes_answer_in_under_50_ms() {
    if std::env::var("PENSINE_BENCH").as_deref() != Ok("1") {
        return;
    }
    let (_dir, store) = store();
    let notes = (0..10_000)
        .map(|index| note(&format!("orchard {index}")))
        .collect();
    store.remember_many(notes).unwrap();
    let start = Instant::now();
    let hits = recall(&store, "orchard");
    let elapsed = start.elapsed();
    assert_eq!(hits.len(), 50);
    assert!(elapsed < Duration::from_millis(50), "{elapsed:?}");
}
