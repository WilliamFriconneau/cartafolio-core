//! The person organises her spaces; nothing is lost, the sensitive one stays apart.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::*;

fn space(id: &str, path: &str, sensitive: bool) -> Space {
    let mut s = Space::default_notes(id);
    s.id = id.into();
    s.path = path.into();
    s.sensitive = sensitive;
    s
}

fn folder(id: &str) -> ConnectorConfig {
    ConnectorConfig {
        id: id.into(),
        kind: ConnectorKind::Markdown,
        mode: Mode::Export,
        target: String::new(),
        icloud: false,
    }
}

fn tree() -> Tree {
    let mut dev = space("dev", "dev", false);
    dev.connectors = vec![folder("code-a"), folder("code-b")];
    let mut memcp = space("memcp", "dev/memcp", false);
    memcp.connectors = vec![folder("memcp-dir")];
    Tree::new(vec![
        Space::default_notes("Perso"),
        space("work", "work", false),
        space("client", "work/client-a", false),
        space("deep", "work/client-a/projet/lot-1", false),
        space("sante", "sante", true),
        dev,
        memcp,
    ])
    .unwrap()
}

/// Increasing clocks, as the node gives them.
fn stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MS: AtomicU64 = AtomicU64::new(1);
    Hlc::tick(None, MS.fetch_add(1, Ordering::Relaxed), "test").encode()
}

fn paths(t: &Tree) -> Vec<&str> {
    t.spaces().iter().map(|s| s.path.as_str()).collect()
}

#[test]
fn renaming_work_to_pro_moves_its_sub_spaces_and_names_the_connectors() {
    let mut t = tree();
    let moved = t.rename("work", "Pro").unwrap();
    assert!(moved.is_empty());
    assert_eq!(t.get("work").unwrap().name, "Pro");
    assert_eq!(t.get("deep").unwrap().path, "pro/client-a/projet/lot-1");
    let moved = t.rename("dev", "Code").unwrap();
    let ids: Vec<&str> = moved.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["code-a", "code-b", "memcp-dir"]);
}

#[test]
fn a_move_never_goes_under_itself_nor_onto_a_taken_path() {
    let mut t = tree();
    assert_eq!(
        t.move_to("work", "work/client-a/x"),
        Err(SpaceError::IntoItself)
    );
    assert_eq!(
        t.move_to("client", "dev/memcp"),
        Err(SpaceError::DuplicatePath("dev/memcp".into()))
    );
    t.move_to("client", "dev/client-a").unwrap();
    assert_eq!(t.get("deep").unwrap().path, "dev/client-a/projet/lot-1");
}

#[test]
fn the_sensitive_space_stays_apart() {
    let mut t = tree();
    // Not swallowed, not seen from an open parent, not under a parent that takes its children's notes.
    assert_eq!(
        t.merge("sante", "notes", stamp),
        Err(SpaceError::SensitiveIntoOpen)
    );
    assert_eq!(
        t.delete("sante", "work", stamp),
        Err(SpaceError::SensitiveIntoOpen)
    );
    t.move_to("sante", "work/sante").unwrap();
    assert_eq!(t.seen_from("work"), ["work", "client", "deep"]);
    t.move_to("sante", "sante").unwrap();
    let mut work = t.get("work").unwrap().clone();
    work.includes_children = true;
    t.save(work).unwrap();
    assert_eq!(
        t.move_to("sante", "work/sante"),
        Err(SpaceError::SensitiveIntoOpen)
    );
}

#[test]
fn notes_of_sub_spaces_are_seen_from_the_parent() {
    let t = tree();
    assert_eq!(t.seen_from("dev"), ["dev", "memcp"]);
    assert_eq!(t.seen_from("memcp"), ["memcp"]);
}

#[test]
fn merging_keeps_everything_and_deleting_hands_the_notes_over() {
    let mut t = tree();
    t.set_words("client", RuleKind::UniqueWord, &["Dupond".into()], || {
        ("r1".into(), stamp())
    })
    .unwrap();
    t.merge("client", "dev", stamp).unwrap();
    assert!(t.get("client").is_none());
    assert_eq!(t.get("deep").unwrap().path, "dev/projet/lot-1");
    assert_eq!(t.words("dev", RuleKind::UniqueWord), ["Dupond"]);
    assert_eq!(
        t.delete("notes", "dev", stamp),
        Err(SpaceError::DefaultSpace)
    );
    t.delete("work", "notes", stamp).unwrap();
    assert!(!paths(&t).contains(&"work"));
}

#[test]
fn reordering_keeps_the_unnamed_after() {
    let mut t = tree();
    t.reorder(&["sante".into(), "dev".into()]);
    assert_eq!(&paths(&t)[..3], ["sante", "dev", "notes"]);
}

#[test]
fn a_note_of_a_sub_space_goes_only_to_its_folders() {
    let t = tree();
    let p = plan(&t, "n1", &["memcp".into()], false);
    let keys: Vec<&str> = p.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(keys, ["n1/memcp/memcp-dir"]);
}
