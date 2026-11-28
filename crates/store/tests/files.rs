#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{ListQuery, NewFile, Store, StoreError, StoreOptions};
use std::cell::Cell;
use std::fs;
use std::io::{self, Read};
use tempfile::TempDir;

fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    (dir, store)
}

fn file(name: &str) -> NewFile<&[u8]> {
    NewFile {
        space: "notes".into(),
        name: name.into(),
        mime: "text/plain".into(),
        reader: &b"abc"[..],
        max_size: 3,
        note: Some("Sample".into()),
    }
}

#[test]
fn identical_bytes_are_stored_once() {
    let (dir, store) = store();
    let first = store.file_put(file("Été 🌱.txt")).unwrap();
    let second = store.file_put(file("copy.txt")).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.size, 3);
    assert_eq!(first.name, "Été 🌱.txt");
    assert_eq!(first.mime, "text/plain");
    assert_eq!(first.note.as_deref(), Some("Sample"));
    assert_eq!(
        first.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(first.sha256, second.sha256);
    let (saved, path) = store.file_get(&first.id).unwrap();
    assert_eq!(saved, first);
    assert_eq!(path, store.file_get(&second.id).unwrap().1);
    assert_eq!(
        path,
        dir.path()
            .canonicalize()
            .unwrap()
            .join("blobs/ba")
            .join(&first.sha256)
    );
    assert_eq!(fs::read(&path).unwrap(), b"abc");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    assert_eq!(fs::read_dir(dir.path().join("blobs")).unwrap().count(), 1);

    let mut missing = file("missing.txt");
    missing.space = "missing".into();
    assert!(matches!(store.file_put(missing), Err(StoreError::NotFound)));
    let query = ListQuery {
        limit: 1,
        ..ListQuery::default()
    };
    let page = store.file_list(query.clone()).unwrap();
    assert_eq!(page.items, [second]);
    let page = store
        .file_list(ListQuery {
            cursor: page.next_cursor,
            ..query
        })
        .unwrap();
    assert_eq!(page.items, [first]);
    assert!(page.next_cursor.is_none());
}

#[test]
fn a_path_like_name_is_refused() {
    let (dir, store) = store();
    for name in ["../x", "a/b", "a\\b", "a..b", "a\nb", "", "."] {
        assert!(matches!(
            store.file_put(file(name)),
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(fs::read_dir(dir.path().join("blobs")).unwrap().count(), 0);
}

struct CountingReader<'a>(&'a Cell<usize>);

impl Read for CountingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        buf.fill(b'x');
        self.0.set(self.0.get() + buf.len());
        Ok(buf.len())
    }
}

#[test]
fn a_file_over_the_limit_is_refused_before_it_is_written_whole() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with(dir.path(), StoreOptions { max_file_size: 4 }).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    for (max_size, bytes_read) in [(1000, 5), (2, 3)] {
        let count = Cell::new(0);
        let metadata = file("large.txt");
        let result = store.file_put(NewFile {
            space: metadata.space,
            name: metadata.name,
            mime: metadata.mime,
            note: metadata.note,
            reader: CountingReader(&count),
            max_size,
        });
        assert!(matches!(result, Err(StoreError::Invalid(message)) if message.contains("limit")));
        assert_eq!(count.get(), bytes_read);
        assert_eq!(fs::read_dir(dir.path().join("blobs")).unwrap().count(), 0);
        assert!(store
            .file_list(ListQuery::default())
            .unwrap()
            .items
            .is_empty());
    }
}

#[test]
fn a_retired_file_leaves_lists_but_not_the_disk() {
    let (dir, store) = store();
    let first = store.file_put(file("first.txt")).unwrap();
    let second = store.file_put(file("second.txt")).unwrap();
    let path = store.file_get(&first.id).unwrap().1;
    assert_eq!(store.spaces().unwrap()[0].files, 2);
    let reopened = Store::open(dir.path()).unwrap();
    reopened.file_retire(&first.id, "Archived").unwrap();
    assert_eq!(
        store
            .file_list(ListQuery::default())
            .unwrap()
            .items
            .as_slice(),
        std::slice::from_ref(&second)
    );
    assert!(matches!(
        store.file_get(&first.id),
        Err(StoreError::NotFound)
    ));
    assert_eq!(store.spaces().unwrap()[0].files, 1);
    assert!(path.is_file());
    assert_eq!(store.file_get(&second.id).unwrap().1, path);
    store.file_retire(&second.id, "Archived").unwrap();
    assert!(!path.exists());
    assert!(store
        .file_list(ListQuery::default())
        .unwrap()
        .items
        .is_empty());
    assert_eq!(store.spaces().unwrap()[0].files, 0);
    let uploaded = store.file_put(file("again.txt")).unwrap();
    assert_eq!(store.file_get(&uploaded.id).unwrap().1, path);
    assert!(path.is_file());
}

#[test]
fn sweeps_unreferenced_blobs_and_staging_files() {
    let (dir, store) = store();
    let live = store.file_put(file("sample.txt")).unwrap();
    let (_, live_path) = store.file_get(&live.id).unwrap();
    let blobs = dir.path().join("blobs");
    let orphan_hash = "a".repeat(64);
    let orphan = blobs.join("aa").join(orphan_hash);
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, b"unused").unwrap();
    let temp = blobs.join(".tmpInterrupted");
    fs::write(&temp, b"incomplete").unwrap();
    let unrelated = blobs.join("unrelated");
    fs::write(&unrelated, b"keep").unwrap();

    let found = store.sweep_blobs(false).unwrap();
    assert_eq!(found.orphans, std::slice::from_ref(&orphan));
    assert_eq!(found.temporary, std::slice::from_ref(&temp));
    assert!(orphan.exists() && temp.exists());
    assert_eq!(store.sweep_blobs(true).unwrap(), found);
    assert!(!orphan.exists() && !temp.exists());
    assert!(live_path.exists() && unrelated.exists());
    assert!(store.sweep_blobs(false).unwrap().orphans.is_empty());
}
