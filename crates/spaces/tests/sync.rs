//! Next to a synchronised folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::*;

#[test]
fn never_write_over_a_hand_made_change() {
    let a = content_hash("a");
    let b = content_hash("b");
    let c = content_hash("c");
    assert_eq!(write_decision(None, None, &a), WriteDecision::Write);
    assert_eq!(write_decision(Some(&a), Some(&b), &a), WriteDecision::Skip);
    assert_eq!(write_decision(Some(&a), Some(&a), &b), WriteDecision::Write);
    assert_eq!(
        write_decision(Some(&c), Some(&a), &b),
        WriteDecision::ConflictCopy
    );
    // A file there that we never wrote is someone else's.
    assert_eq!(
        write_decision(Some(&c), None, &b),
        WriteDecision::ConflictCopy
    );
}

#[test]
fn copies_made_by_sync_tools_are_known() {
    let machines = vec!["serveur".to_string()];
    for name in [
        "note (1).md",
        "note.sync-conflict-20261116-120000-ABC.md",
        "note (conflicted copy).md",
        "note-serveur.md",
    ] {
        assert!(is_conflict_copy(name, &machines), "{name}");
    }
    for name in ["note.md", "rdv (jeudi).md", "2026-11-16-0805-rappeler.md"] {
        assert!(!is_conflict_copy(name, &machines), "{name}");
    }
    assert_eq!(
        conflict_name("note.md", "2026-11-16 0805"),
        "note (kalios 2026-11-16 0805).md"
    );
}

#[test]
fn hidden_and_build_folders_are_left_alone() {
    for name in [".git", ".DS_Store", "node_modules", "target", ".obsidian"] {
        assert!(is_ignored(name), "{name}");
    }
    assert!(!is_ignored("notes"));
}
