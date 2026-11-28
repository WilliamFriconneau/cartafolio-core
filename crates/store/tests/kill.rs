#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{ListQuery, NewFact, Store};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const WRITER_DIR: &str = "PENSINE_KILL_WRITER_DIR";

#[test]
#[ignore = "Subprocess fixture for the SIGKILL recovery test"]
fn kill_test_writer() {
    let Some(dir) = std::env::var_os(WRITER_DIR) else {
        return;
    };
    let store = Store::open(Path::new(&dir)).unwrap();
    for index in 0..1_000_000 {
        store
            .remember(NewFact {
                space: "notes".into(),
                text: format!("Journal entry {index}"),
                tags: vec![],
                author: "writer".into(),
                source: "capture".into(),
                at: None,
                origin: Some(("journal".into(), index.to_string())),
            })
            .unwrap();
        if (index + 1) % 100 == 0 {
            println!("committed {}", index + 1);
            std::io::stdout().flush().unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
#[ignore = "Repeatedly SIGKILLs a child and checks SQLite recovery. Set PENSINE_KILL=1."]
fn sigkill_preserves_committed_notes() {
    use std::os::unix::process::ExitStatusExt;

    if std::env::var("PENSINE_KILL").as_deref() != Ok("1") {
        return;
    }
    for delay in [0, 2, 5, 10, 17, 25, 39, 61] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.space_create("notes", "Notes", None).unwrap();
        drop(store);

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "kill_test_writer", "--ignored", "--nocapture"])
            .env(WRITER_DIR, dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let committed = lines.find_map(|line| {
            line.unwrap()
                .strip_prefix("committed ")
                .map(|count| count.parse::<usize>().unwrap())
        });
        std::thread::sleep(Duration::from_millis(delay));
        child.kill().unwrap();
        assert_eq!(child.wait().unwrap().signal(), Some(9));
        let committed = lines
            .filter_map(|line| {
                line.unwrap()
                    .strip_prefix("committed ")
                    .map(|count| count.parse::<usize>().unwrap())
            })
            .last()
            .or(committed)
            .unwrap();

        let store = Store::open(dir.path()).unwrap();
        let mut query = ListQuery {
            limit: 200,
            ..ListQuery::default()
        };
        let mut entries = Vec::new();
        loop {
            let page = store.list(query.clone()).unwrap();
            entries.extend(
                page.items
                    .into_iter()
                    .filter_map(|fact| fact.origin.map(|(_, id)| id.parse::<usize>().unwrap())),
            );
            query.cursor = page.next_cursor;
            if query.cursor.is_none() {
                break;
            }
        }
        entries.sort_unstable();
        assert!(entries.len() >= committed && entries.len() <= committed + 100);
        assert!(entries
            .into_iter()
            .enumerate()
            .all(|(expected, entry)| expected == entry));
    }
}
