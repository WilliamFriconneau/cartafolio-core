#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use pensine_store::{FactStatus, ListQuery, NewFact};
use std::process::Output;
use support::{Fixture, OLD_KEY};

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrates_every_active_note_with_time_author_and_tags() {
    let fixture = Fixture::new().await;
    success(fixture.run(&[]));
    let spaces = fixture.account.store.spaces().unwrap();
    assert_eq!(spaces.iter().map(|s| s.notes).sum::<u64>(), 316);
    assert_eq!(
        spaces
            .iter()
            .find(|s| s.name == "child")
            .unwrap()
            .parent
            .as_deref(),
        Some("root")
    );
    let inbox = fixture
        .account
        .store
        .list(ListQuery {
            spaces: vec!["inbox".into()],
            pinned: Some(false),
            ..ListQuery::default()
        })
        .unwrap();
    let note = &inbox.items[0];
    assert_eq!(note.text, "Fictional capture entry 0");
    assert_eq!(note.author, "Synthetic scribe");
    assert_eq!(note.created_at, 1000);
    assert_eq!(note.tags, ["été", "inbox"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rerunning_adds_nothing() {
    let fixture = Fixture::new().await;
    success(fixture.run(&[]));
    let report = success(fixture.run(&[]));
    assert!(report.contains("notes sent=0 duplicates=104"), "{report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delta_run_adds_only_new_notes() {
    let fixture = Fixture::new().await;
    success(fixture.run(&[]));
    fixture.delta();
    let report = success(fixture.run(&[]));
    assert_eq!(
        report.matches("notes sent=1 duplicates=104").count(),
        3,
        "{report}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_tags_are_dropped_and_counted() {
    let fixture = Fixture::new().await;
    let report = success(fixture.run(&[]));
    assert_eq!(report.matches("tags dropped=1").count(), 3, "{report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_notes_are_skipped_unless_asked() {
    let fixture = Fixture::new().await;
    assert!(success(fixture.run(&["--only", "root"])).contains("old count=104"));
    assert!(
        success(fixture.run(&["--only", "root", "--include-retired"])).contains("old count=105")
    );
    let retired = fixture
        .account
        .store
        .list(ListQuery {
            status: Some(FactStatus::Retired),
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(retired.items.len(), 1);
    assert_eq!(retired.items[0].origin.as_ref().unwrap().1, "root-999");
    success(fixture.run(&["--only", "root", "--include-retired"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_are_copied_once() {
    let fixture = Fixture::new().await;
    let archive = tempfile::tempdir().unwrap();
    let report = success(fixture.run(&["--archive", archive.path().to_str().unwrap()]));
    assert!(report.contains("files sent=1"));
    assert!(report.contains("over 8000000 bytes"));
    let manifest = std::fs::read_to_string(archive.path().join("root.jsonl")).unwrap();
    let records: Vec<serde_json::Value> = manifest
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        records.iter().filter(|row| row["type"] == "note").count(),
        104
    );
    for (id, size) in [("file-small", 7), ("file-large", 8_000_001)] {
        let file = records
            .iter()
            .find(|row| row["record"]["metadata"]["id"] == id)
            .unwrap();
        let archived = archive
            .path()
            .join(file["record"]["blob"].as_str().unwrap());
        assert_eq!(std::fs::metadata(archived).unwrap().len(), size);
    }
    let report = success(fixture.run(&[]));
    assert!(!report.contains("files sent=1"));
    let (meta, path) = fixture
        .account
        .store
        .file_get(
            &fixture
                .account
                .store
                .file_list(ListQuery::default())
                .unwrap()
                .items[0]
                .id,
        )
        .unwrap();
    assert_eq!(meta.name, "sample.txt");
    assert_eq!(meta.mime, "text/plain");
    assert_eq!(meta.note.as_deref(), Some("Fixture"));
    assert_eq!(std::fs::read(path).unwrap(), b"example");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_integrity_line_detects_a_difference() {
    let fixture = Fixture::new().await;
    success(fixture.run(&["--only", "root"]));
    let store = &fixture.account.store;
    let fact = store
        .list(ListQuery {
            pinned: Some(false),
            ..ListQuery::default()
        })
        .unwrap()
        .items
        .remove(0);
    store
        .supersede(
            &fact.id,
            NewFact {
                space: "root".into(),
                text: "Changed fixture".into(),
                tags: vec![],
                author: "Editor".into(),
                source: "fixture".into(),
                at: None,
                origin: None,
            },
            "Fixture edit",
        )
        .unwrap();
    let result = fixture.run(&["--only", "root"]);
    assert_eq!(result.status.code(), Some(1));
    let report = String::from_utf8(result.stdout).unwrap();
    assert!(
        report.contains("integrity") && report.contains(&fact.origin.unwrap().1),
        "{report}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_never_appears_in_output() {
    let fixture = Fixture::new().await;
    success(fixture.run(&["--dry-run"]));
    assert!(fixture.account.store.spaces().unwrap().is_empty());
    fixture.refuse();
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(1));
    for stream in [output.stdout, output.stderr] {
        let stream = String::from_utf8(stream).unwrap();
        assert!(!stream.contains(OLD_KEY));
        assert!(!stream.contains(fixture.new_key()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_duplicate_origin_is_verified_against_the_existing_text() {
    let fixture = Fixture::new().await;
    let store = &fixture.account.store;
    store.space_create("root", "Root", None).unwrap();
    store
        .remember(NewFact {
            space: "root".into(),
            text: "Fictional root entry 0".into(),
            tags: vec![],
            author: "Fixture author".into(),
            source: "fixture".into(),
            at: None,
            origin: None,
        })
        .unwrap();
    let report = success(fixture.run(&["--only", "root"]));
    assert!(
        report.contains("old count=104 new count=104 notes sent=103 duplicates=1"),
        "{report}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_permanent_note_error_does_not_discard_the_rest_of_its_batch() {
    let fixture = Fixture::new().await;
    fixture.reject_one_note();
    let output = fixture.run(&["--only", "root"]);
    assert_eq!(output.status.code(), Some(1));
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("new count=103 notes sent=103"), "{report}");
    assert!(report.contains("Destination note \"root-2\""), "{report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transient_http_failure_is_retried_five_times() {
    let fixture = Fixture::new().await;
    fixture.fail_requests(5);
    success(fixture.run(&["--dry-run", "--only", "root"]));
    assert_eq!(fixture.failed_requests(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_mapping_overrides_inbox_routing() {
    let fixture = Fixture::new().await;
    success(fixture.run(&["--only", "capture", "--map", "capture=imported"]));
    let spaces = fixture.account.store.spaces().unwrap();
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces[0].name, "imported");
    assert_eq!(spaces[0].notes, 105);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_migration_tool_turns_an_old_todo_tag_into_the_todo_content() {
    let fixture = Fixture::new().await;
    fixture.old_todo_note();
    let report = success(fixture.run(&["--only", "root"]));
    assert!(report.contains("errors=0"), "{report}");
    let store = &fixture.account.store;
    let todo = store
        .list(ListQuery {
            spaces: vec!["root".into()],
            tags_all: vec!["todo".into()],
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(todo.items.len(), 1);
    let note = &todo.items[0];
    assert_eq!(note.text, "Fictional todo entry");
    assert_eq!(
        note.origin.as_ref(),
        Some(&("pensine-old".to_string(), "root-todo".to_string()))
    );
    assert!(note.pinned);
    let rerun = success(fixture.run(&["--only", "root"]));
    assert!(rerun.contains("notes sent=0 duplicates=105"), "{rerun}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_todo_note_never_counts_in_the_integrity_line_as_missing() {
    let fixture = Fixture::new().await;
    let report = success(fixture.run(&["--only", "root"]));
    assert!(report.contains("old count=104 new count=104"), "{report}");
    assert!(report.contains("errors=0"), "{report}");
    assert!(!report.contains("Missing origin"), "{report}");
    let store = &fixture.account.store;
    let root = store
        .spaces()
        .unwrap()
        .into_iter()
        .find(|space| space.name == "root")
        .unwrap();
    let todo = store.read(root.todo.as_deref().unwrap()).unwrap();
    assert!(todo.origin.is_none());
    assert!(todo.pinned);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_is_never_followed_by_read_fact_when_the_item_is_complete() {
    let fixture = Fixture::new().await;
    success(fixture.run(&[]));
    assert_eq!(fixture.read_fact_calls(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progress_goes_to_stderr_without_note_text() {
    let fixture = Fixture::new().await;
    let output = fixture.run(&[]);
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("space root:"), "{stderr}");
    assert!(stderr.contains("notes read"), "{stderr}");
    assert!(!stderr.contains("Fictional"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("space root:"),
        "progress belongs on stderr"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_field_falls_back_to_read_fact_and_is_counted() {
    let fixture = Fixture::new().await;
    fixture.strip_tags_from_pages();
    let report = success(fixture.run(&[]));
    assert!(fixture.read_fact_calls() > 0);
    assert!(report.contains("needed a second read"), "{report}");
}
