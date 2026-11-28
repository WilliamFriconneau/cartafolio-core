#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{ListQuery, NewFact, RecallQuery, Store};
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
#[ignore = "Builds a 100,000-note corpus and checks wall-clock latency. Set PENSINE_BENCH=1."]
fn one_hundred_thousand_note_benchmark() {
    if std::env::var("PENSINE_BENCH").as_deref() != Ok("1") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    for batch in 0..10 {
        let notes = (batch * 10_000..(batch + 1) * 10_000)
            .map(|i| NewFact {
                space: "notes".into(),
                text: format!("Field journal entry {i}: observations for sector{}", i % 20),
                tags: vec![format!("survey/area{}", i % 5)],
                author: "writer".into(),
                source: "benchmark".into(),
                at: None,
                origin: None,
            })
            .collect();
        store.remember_many(notes).unwrap();
    }
    drop(store);
    let started = Instant::now();
    let store = Store::open(dir.path()).unwrap();
    let startup = started.elapsed();
    let resident = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(resident.status.success());
    let kib: u64 = std::str::from_utf8(&resident.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut recall = Vec::new();
    let mut list = Vec::new();
    for run in 0..100 {
        let query = RecallQuery {
            text: format!("observations sector{}", run % 20),
            spaces: vec!["notes".into()],
            ..RecallQuery::default()
        };
        let started = Instant::now();
        assert_eq!(store.recall(query).unwrap().len(), 50);
        recall.push(started.elapsed());

        let query = ListQuery {
            spaces: vec!["notes".into()],
            ..ListQuery::default()
        };
        let started = Instant::now();
        let page = store.list(query).unwrap();
        assert!(page.next_cursor.is_some());
        list.push(started.elapsed());
    }
    recall.sort_unstable();
    list.sort_unstable();
    eprintln!(
        "100,000 notes, 100 samples: recall p50 {:?}, p95 {:?}; list p50 {:?}, p95 {:?}; reopen {:?}; idle RSS {} KiB",
        recall[49], recall[94], list[49], list[94], startup, kib
    );
    assert!(recall[94] < Duration::from_millis(50));
}
