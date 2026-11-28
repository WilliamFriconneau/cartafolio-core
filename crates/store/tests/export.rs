#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_store::{NewFact, NewFile, Store};
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn setup() -> (TempDir, Store, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).unwrap();
    store.space_create("notes", "Notes", None).unwrap();
    let out = dir.path().join("out");
    (dir, store, out)
}

fn note(text: &str) -> NewFact {
    NewFact {
        space: "notes".into(),
        text: text.into(),
        tags: vec!["Été".into(), "ideas".into()],
        author: "Writer\n---".into(),
        source: "capture: offline".into(),
        at: Some(1_704_067_200),
        origin: None,
    }
}

fn put(store: &Store) -> String {
    store
        .file_put(NewFile {
            space: "notes".into(),
            name: "sample.bin".into(),
            mime: "application/octet-stream".into(),
            reader: &[0, 1, 2][..],
            max_size: 3,
            note: Some("Attachment description".into()),
        })
        .unwrap()
        .id
}

#[test]
fn export_round_trips_text_tags_and_pin() {
    let (_dir, store, out) = setup();
    let text = "# A note\n\n---\nLast line without a newline";
    let id = store.remember(note(text)).unwrap().fact.id;
    store.pin(&id, true).unwrap();

    store.export(&out).unwrap();

    let exported = fs::read_to_string(out.join(format!("notes/2024-01/{id}.md"))).unwrap();
    let (header, body) = exported
        .strip_prefix("---\n")
        .unwrap()
        .split_once("\n---\n")
        .unwrap();
    let fields: serde_json::Map<String, Value> = header
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(": ").unwrap();
            (key.into(), serde_json::from_str(value).unwrap())
        })
        .collect();
    assert_eq!(body, text);
    assert_eq!(fields["id"], id);
    assert_eq!(fields["tags"], serde_json::json!(["été", "ideas"]));
    assert_eq!(fields["author"], "Writer\n---");
    assert_eq!(fields["source"], "capture: offline");
    assert_eq!(fields["created"], 1_704_067_200);
    assert_eq!(fields["pinned"], true);
    assert!(fields["replaces"].is_null());
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(snapshot(&entry.path()));
        } else {
            files.push((
                entry.path(),
                fs::read(entry.path()).unwrap(),
                entry.metadata().unwrap().modified().unwrap(),
            ));
        }
    }
    files.sort();
    files
}

#[test]
fn export_twice_changes_nothing() {
    let (_dir, store, out) = setup();
    store.remember(note("Sample")).unwrap();
    let id = put(&store);
    store.export(&out).unwrap();
    let before = snapshot(&out);
    assert_eq!(before.len(), 4);
    assert_eq!(
        fs::read(out.join(format!("notes/_files/{id}/sample.bin"))).unwrap(),
        [0, 1, 2]
    );
    assert!(
        fs::read_to_string(out.join(format!("notes/_files/{id}.md")))
            .unwrap()
            .ends_with("Attachment description")
    );
    store.export(&out).unwrap();
    assert_eq!(snapshot(&out), before);
}

#[test]
fn export_never_writes_outside_out() {
    let (dir, store, out) = setup();
    store.remember(note("Sample")).unwrap();
    let id = put(&store);
    let db = Connection::open(dir.path().join("db/store.db")).unwrap();
    db.pragma_update(None, "foreign_keys", false).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("guard"), "original").unwrap();
    db.execute("UPDATE facts SET space = '../outside'", [])
        .unwrap();
    assert!(store.export(&out).is_err());
    db.execute("UPDATE facts SET space = 'notes'", []).unwrap();
    db.execute(
        "UPDATE files SET name = ?1 WHERE id = ?2",
        (outside.join("guard").to_str().unwrap(), id.as_str()),
    )
    .unwrap();
    assert!(store.export(&out).is_err());
    db.execute("UPDATE files SET name = '../guard'", [])
        .unwrap();
    assert!(store.export(&out).is_err());
    #[cfg(unix)]
    {
        db.execute("UPDATE files SET name = 'sample.bin'", [])
            .unwrap();
        let attachment = out.join(format!("notes/_files/{id}"));
        fs::create_dir_all(attachment.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &attachment).unwrap();
        assert!(store.export(&out).is_err());
    }
    assert_eq!(
        fs::read_to_string(outside.join("guard")).unwrap(),
        "original"
    );
    assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
}

#[test]
fn history_goes_under_history_folder() {
    let (_dir, store, out) = setup();
    let old = store.remember(note("First version")).unwrap().fact;
    let retired = store.remember(note("Retired note")).unwrap().fact;
    store.export(&out).unwrap();
    let new = store
        .supersede(&old.id, note("Second version"), "Revised")
        .unwrap();
    store.retire(&retired.id, "Obsolete").unwrap();
    store.export(&out).unwrap();
    for fact in [old, retired] {
        assert!(!out.join(format!("notes/2024-01/{}.md", fact.id)).exists());
        let history =
            fs::read_to_string(out.join(format!("notes/_history/{}.md", fact.id))).unwrap();
        assert!(history.ends_with(&fact.text));
    }
    let active = fs::read_to_string(out.join(format!("notes/2024-01/{}.md", new.id))).unwrap();
    assert!(active.ends_with("Second version"));
    assert!(active.contains(&format!("replaces: \"{}\"", new.replaces.unwrap())));
}
