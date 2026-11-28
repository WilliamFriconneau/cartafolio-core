use crate::Store;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn export_does_not_block_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    let (send, receive) = mpsc::channel();
    let finished = std::thread::scope(|scope| {
        let writer = store.writer.lock().unwrap();
        scope.spawn(|| send.send(store.export(&dir.path().join("export"))).unwrap());
        let finished = receive.recv_timeout(Duration::from_secs(2));
        drop(writer);
        finished
    });
    finished.unwrap().unwrap();
}

#[test]
fn blob_persist_syncs_directories_before_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    store
        .file_put(crate::NewFile {
            space: "notes".into(),
            name: "test.txt".into(),
            mime: "text/plain".into(),
            reader: &b"text"[..],
            max_size: 4,
            note: None,
        })
        .unwrap();
    assert_eq!(
        *store.durability_events.lock().unwrap(),
        ["blob", "transaction", "shard", "blobs", "commit"]
    );
}
