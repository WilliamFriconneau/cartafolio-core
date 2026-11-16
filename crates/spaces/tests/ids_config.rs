//! Ids, the logical clock and the spaces file: ready for peers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use memcp_spaces::*;

#[test]
fn ids_are_uuid_v7_unique_and_sorted_by_time() {
    let a = uuid_v7_at(1_700_000_000_000);
    let b = uuid_v7_at(1_700_000_000_001);
    assert_eq!(a.len(), 36);
    assert_eq!(&a[14..15], "7");
    assert!(a < b);
    let many: std::collections::HashSet<String> = (0..1000).map(|_| new_id()).collect();
    assert_eq!(many.len(), 1000);
}

#[test]
fn the_clock_never_goes_back_and_sorts_as_text() {
    let a = Hlc::tick(None, 1000, "n1");
    let b = Hlc::tick(Some(&a), 900, "n1");
    assert!(b > a);
    assert_eq!((b.ms, b.counter), (1000, 1));
    let remote = Hlc {
        ms: 5000,
        counter: 3,
        node: "n2".into(),
    };
    let c = b.receive(&remote, 1200);
    assert_eq!((c.ms, c.counter, c.node.as_str()), (5000, 4, "n1"));
    assert!(a.encode() < b.encode() && b.encode() < c.encode());
    assert_eq!(Hlc::decode(&c.encode()), Some(c));
}

#[test]
fn the_spaces_file_is_written_whole_and_read_back() {
    let dir = std::env::temp_dir().join(format!("memcp-spaces-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("spaces.json");
    let mut tree = load_spaces(&path, "Notes").unwrap();
    assert_eq!(tree.spaces().len(), 1);
    let mut pro = Space::default_notes("Pro");
    pro.id = new_id();
    pro.path = "pro".into();
    let pro_id = pro.id.clone();
    tree.save(pro).unwrap();
    tree.set_words(&pro_id, RuleKind::UniqueWord, &["Acme".into()], || {
        (new_id(), Hlc::tick(None, now_ms(), "t").encode())
    })
    .unwrap();
    save_spaces(&path, &tree).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"format\": \"kalios.spaces/1\""));
    assert_eq!(load_spaces(&path, "Notes").unwrap(), tree);
    assert!(parse_spaces("{\"format\":\"autre/1\",\"spaces\":[]}", "Notes").is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
