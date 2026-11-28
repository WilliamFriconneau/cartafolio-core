use crate::{Store, StoreError};
use std::fs;
use std::sync::Barrier;
use std::thread;

#[test]
fn opens_an_empty_folder_and_is_at_version_1() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new");
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version(), 1);
    assert!(path.join("store.db").is_file());
    assert!(path.join("blobs").is_dir());
    for conn in std::iter::once(&store.writer).chain(store.readers.iter()) {
        let conn = conn.lock().unwrap();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let foreign_keys: u32 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        let timeout: u32 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        assert_eq!(foreign_keys, 1);
        assert_eq!(timeout, 5000);
    }
}

#[test]
fn reopens_without_changing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .writer
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO spaces (name, label, position) VALUES ('test', 'Test', 1)",
            [],
        )
        .unwrap();
    drop(store);
    let database = dir.path().join("store.db");
    let before = fs::read(&database).unwrap();
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(reopened.schema_version(), 1);
    let label: String = reopened.readers[0]
        .lock()
        .unwrap()
        .query_row("SELECT label FROM spaces WHERE name = 'test'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(label, "Test");
    drop(reopened);
    assert_eq!(fs::read(database).unwrap(), before);
}

#[test]
fn two_writers_at_once_never_see_database_is_locked() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let start = Barrier::new(2);
    thread::scope(|scope| {
        for writer in 0..2 {
            let store = &store;
            let start = &start;
            scope.spawn(move || {
                start.wait();
                for index in 0..200 {
                    store
                        .writer
                        .lock()
                        .unwrap()
                        .execute(
                            "INSERT INTO spaces (name, label, position) VALUES (?1, 'Test', ?2)",
                            (format!("space-{writer}-{index}"), index),
                        )
                        .unwrap();
                }
            });
        }
    });
    let count: u32 = store.readers[0]
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 400);
}

#[test]
fn refuses_a_folder_it_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    // A file at the required blobs path prevents setup even when tests run as root.
    fs::write(dir.path().join("blobs"), []).unwrap();
    assert!(matches!(Store::open(dir.path()), Err(StoreError::Io(_))));
}
