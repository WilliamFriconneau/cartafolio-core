#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The nodes of one person are one swarm: a space made on any node, its label and its place in the tree, a rename or a
//! move, a note left in Inbox, the filing of that note and an archive reach every node. Every node is a real one;
//! the one the others reach is served on a socket opened by the test (port 0).

use std::sync::Arc;

use memcp_client::peer_sync::Peer;
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::layout::Change;
use memcp_core::lifecycle::{Children, Disposition};
use memcp_core::text::Language;
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space, triage, util};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};
use serde_json::json;

fn temp(name: &str) -> std::path::PathBuf {
    // One folder per node: the tests run side by side.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-swarm-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A node of the person, with its own Inbox (named after what the node calls the person: every node a
/// different name, as on a real installation).
struct Node {
    brain: Brain,
    dir: std::path::PathBuf,
    triage: Space,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn node(name: &str) -> Node {
    let dir = temp(name);
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let triage = triage::ensure(&brain, OWNER, name).unwrap();
    Node { brain, dir, triage }
}

/// Serves `n` and makes a connection there for one machine of the person: its address and key.
fn serve(n: &Node, machine: &str) -> (String, String) {
    let agent = format!("peer-{machine}-0001");
    let key = access::connect_for(
        &n.brain.open_control().unwrap(),
        OWNER,
        &agent,
        &Rights::parse("all").unwrap(),
        None,
        false,
        false,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Arc::new(App::new(Brain::open(&n.dir).unwrap()));
    std::thread::spawn(move || {
        let l = Listeners {
            listen: format!("127.0.0.1:{port}"),
            identity: None,
            ingress: None,
            threads: 2,
        };
        let _ = serve_on(app, listener, &l, "");
    });
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    (format!("http://127.0.0.1:{port}/mcp/{agent}"), key)
}

/// Another connection on an already served node (`serve` started its server).
fn connection(n: &Node, address_of_first: &str, machine: &str) -> (String, String) {
    let agent = format!("peer-{machine}-0001");
    let key = access::connect_for(
        &n.brain.open_control().unwrap(),
        OWNER,
        &agent,
        &Rights::parse("all").unwrap(),
        None,
        false,
        false,
    )
    .unwrap();
    let base = address_of_first.rsplit_once("/mcp/").unwrap().0;
    (format!("{base}/mcp/{agent}"), key)
}

fn pass(peer: &Peer, n: &Node) -> memcp_client::peer_sync::Pass {
    let p = peer.sync_once(&n.brain, util::now()).unwrap();
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    p
}

/// `(label, parent)` of a space, as the node holds it.
fn place(n: &Node, s: &Space) -> Option<(String, Option<String>)> {
    n.brain
        .layout(OWNER)
        .unwrap()
        .into_iter()
        .find(|p| &p.space == s)
        .map(|p| (p.label, p.parent.map(|x| x.as_str().to_string())))
}

fn note(n: &Node, space: &Space, text: &str) -> String {
    let conn = n.brain.open_space(space).unwrap();
    let mut new = NewFact::agent(text, "agent:dictation");
    new.tags = vec!["inbox".into()];
    match facts::remember(&conn, space, "dictation", new, &[]).unwrap() {
        Remembered::Created(f) => f.id,
        other => panic!("not created: {other:?}"),
    }
}

fn active(n: &Node, space: &Space) -> Vec<String> {
    let conn = n.brain.open_space(space).unwrap();
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

#[test]
fn a_sub_space_made_on_one_node_reaches_the_others_with_its_label_and_parent() {
    let a = node("alma");
    let b = node("bea");
    let c = node("cyd");
    let (addr_b, key_b) = serve(&a, "b");
    let (addr_c, key_c) = connection(&a, &addr_b, "c");
    let b_to_a = Peer::new(&addr_b, &key_b, "peer-a");
    let c_to_a = Peer::new(&addr_c, &key_c, "peer-a");

    // On B, a space and a sub-space of it.
    let hobbies = b
        .brain
        .add_custom_space(OWNER, "bea", "Hobbies", None)
        .unwrap();
    let pompiers = b
        .brain
        .add_custom_space(OWNER, "bea", "Pompiers", Some(&hobbies))
        .unwrap();
    note(&b, &pompiers, "Training on Thursday at the station");

    // One pass of B with A: both are on A, in their place, with their note.
    let p = pass(&b_to_a, &b);
    assert_eq!(p.offered, 2, "{p:?}");
    assert_eq!(place(&a, &hobbies), Some(("Hobbies".into(), None)));
    assert_eq!(
        place(&a, &pompiers),
        Some(("Pompiers".into(), Some(hobbies.as_str().into())))
    );
    assert_eq!(
        active(&a, &pompiers),
        ["Training on Thursday at the station"]
    );

    // C, the third node, gets them from A in one pass.
    pass(&c_to_a, &c);
    assert_eq!(
        place(&c, &pompiers),
        Some(("Pompiers".into(), Some(hobbies.as_str().into())))
    );
    assert_eq!(
        active(&c, &pompiers),
        ["Training on Thursday at the station"]
    );

    // Renamed and moved to the top on A: B and C follow.
    a.brain
        .set_layout(
            OWNER,
            &pompiers,
            &Change {
                label: Some("Sapeurs-pompiers".into()),
                parent: Some(None),
                position: None,
            },
        )
        .unwrap();
    pass(&b_to_a, &b);
    pass(&c_to_a, &c);
    assert_eq!(
        place(&b, &pompiers),
        Some(("Sapeurs-pompiers".into(), None))
    );
    assert_eq!(
        place(&c, &pompiers),
        Some(("Sapeurs-pompiers".into(), None))
    );

    // Renamed again on C, later: the newest wins everywhere.
    c.brain
        .set_layout(
            OWNER,
            &pompiers,
            &Change {
                label: Some("Pompiers".into()),
                parent: Some(Some(hobbies.clone())),
                position: None,
            },
        )
        .unwrap();
    pass(&c_to_a, &c);
    pass(&b_to_a, &b);
    for n in [&a, &b, &c] {
        assert_eq!(
            place(n, &pompiers),
            Some(("Pompiers".into(), Some(hobbies.as_str().into())))
        );
    }

    // Nothing is offered twice.
    let again = pass(&b_to_a, &b);
    assert_eq!((again.offered, again.pulled, again.pushed), (0, 0, 0));
}

#[test]
fn a_dictation_left_in_a_nodes_a_trier_is_in_the_a_trier_of_every_node() {
    let a = node("alma");
    let b = node("bea");
    let c = node("cyd");
    assert_ne!(a.triage, b.triage);
    let (addr_b, key_b) = serve(&a, "b");
    let (addr_c, key_c) = connection(&a, &addr_b, "c");
    let b_to_a = Peer::new(&addr_b, &key_b, "peer-a");
    let c_to_a = Peer::new(&addr_c, &key_c, "peer-a");

    note(&b, &b.triage, "Call the plumber about the kitchen tap");
    note(&a, &a.triage, "Book the car service for December");
    pass(&b_to_a, &b);
    pass(&c_to_a, &c);
    pass(&b_to_a, &b);
    let both = vec![
        "Book the car service for December".to_string(),
        "Call the plumber about the kitchen tap".to_string(),
    ];
    assert_eq!(active(&a, &a.triage), both);
    assert_eq!(active(&b, &b.triage), both);
    assert_eq!(active(&c, &c.triage), both);
    // One box per node: the peer's box was not copied beside it.
    for n in [&a, &b, &c] {
        let boxes = n
            .brain
            .spaces()
            .unwrap()
            .into_iter()
            .filter(|s| triage::is_triage(&n.brain, s))
            .count();
        assert_eq!(boxes, 1);
    }
}

#[test]
fn a_note_of_a_trier_is_filed_once_whichever_janitors_file_it() {
    let a = node("alma");
    let b = node("bea");
    let (addr, key) = serve(&a, "b");
    let b_to_a = Peer::new(&addr, &key, "peer-a");
    let home = b
        .brain
        .add_custom_space(OWNER, "bea", "Home", None)
        .unwrap();
    let first = note(&b, &b.triage, "The boiler pressure dropped again tonight");
    let second = note(&b, &b.triage, "Buy a new filter for the hood");
    pass(&b_to_a, &b);

    // The janitor of A files the first note; after a pass, B does not offer it to its own janitor any more.
    triage::send(&a.brain, &a.triage, &first, &home, "janitor-a", OWNER).unwrap();
    pass(&b_to_a, &b);
    let waiting = triage::waiting(&b.brain, &b.triage, 50).unwrap();
    let ids: Vec<&str> = waiting["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert_eq!(ids, [second.as_str()]);

    // Both janitors file the second note before they hear of each other: one note once the nodes meet.
    triage::send(&a.brain, &a.triage, &second, &home, "janitor-a", OWNER).unwrap();
    triage::send(&b.brain, &b.triage, &second, &home, "janitor-b", OWNER).unwrap();
    pass(&b_to_a, &b);
    pass(&b_to_a, &b);
    let filed = vec![
        "Buy a new filter for the hood".to_string(),
        "The boiler pressure dropped again tonight".to_string(),
    ];
    assert_eq!(active(&a, &home), filed);
    assert_eq!(active(&b, &home), filed);
    assert!(active(&a, &a.triage).is_empty());
    assert!(active(&b, &b.triage).is_empty());
}

#[test]
fn a_space_archived_on_one_node_is_archived_on_the_others_and_not_made_again() {
    let a = node("alma");
    let b = node("bea");
    let (addr, key) = serve(&a, "b");
    let b_to_a = Peer::new(&addr, &key, "peer-a");
    let garden = b
        .brain
        .add_custom_space(OWNER, "bea", "Garden", None)
        .unwrap();
    note(&b, &garden, "Prune the apple tree in February");
    pass(&b_to_a, &b);
    assert!(a.brain.has_space(&garden).unwrap());

    // Archived on A: B archives its copy at its next pass (kept in its archive), and A does not get it back.
    a.brain
        .delete_space(
            OWNER,
            &garden,
            &Disposition::Archive,
            &Children::Grandparent,
            false,
            util::now() + 1,
        )
        .unwrap();
    let p = pass(&b_to_a, &b);
    assert_eq!(p.archived, 1, "{p:?}");
    assert!(!b.brain.has_space(&garden).unwrap());
    assert_eq!(b.brain.archived_spaces(OWNER).unwrap().len(), 1);
    pass(&b_to_a, &b);
    assert!(!a.brain.has_space(&garden).unwrap());
    assert!(!b.brain.has_space(&garden).unwrap());
}

#[test]
fn a_peer_of_an_older_version_is_kept_in_step_and_the_pass_says_what_it_did_not_take() {
    let b = node("bea");
    b.brain
        .add_custom_space(OWNER, "bea", "Music", None)
        .unwrap();
    // A node of the previous version: it lists its spaces with no word of offers.
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for rq in server.incoming_requests() {
            let body = json!({ "protocol": 1, "version": "0.0.1", "spaces": [] });
            let _ = rq.respond(
                tiny_http::Response::from_string(body.to_string()).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                ),
            );
        }
    });
    let peer = Peer::new(
        &format!("http://127.0.0.1:{port}/mcp/peer-x-0001"),
        "k",
        "peer-old",
    );
    let p = peer.sync_once(&b.brain, util::now()).unwrap();
    assert_eq!((p.spaces, p.offered), (0, 0));
    assert_eq!(p.problems.len(), 1, "{:?}", p.problems);
    assert!(p.problems[0].contains("older version"), "{:?}", p.problems);
    assert!(p.problems[0].contains("bea-music"), "{:?}", p.problems);
}

#[test]
fn a_window_replica_never_prunes_a_note_its_peer_refused() {
    let a = node("alma");
    let b = node("bea");
    let (addr, key) = serve(&a, "b");
    let b_to_a = Peer::new(&addr, &key, "peer-a");
    // B keeps 30 days. It holds two old notes: one A takes, one A refuses (an author longer than a peer accepts).
    memcp_core::sync::set_window_days(&b.brain, Some(30)).unwrap();
    let conn = b.brain.open_space(&b.triage).unwrap();
    let taken = note(&b, &b.triage, "The boiler was serviced in March");
    let long = "x".repeat(300);
    let Remembered::Created(refused) = facts::remember(
        &conn,
        &b.triage,
        &long,
        NewFact::agent("The only copy of the gate code", "agent:dictation"),
        &[],
    )
    .unwrap() else {
        panic!("not created");
    };
    let old = util::now() - 90 * 86_400;
    for id in [&taken, &refused.id] {
        conn.execute(
            &format!("UPDATE facts SET created_at = {old} WHERE id = ?1"),
            [id.as_str()],
        )
        .unwrap();
    }

    let p = b_to_a.sync_once(&b.brain, util::now()).unwrap();
    // The refusal is said; the note A took is pruned on B, the one A refused stays: it exists nowhere else.
    assert!(
        p.problems.iter().any(|x| x.contains("refused")),
        "{:?}",
        p.problems
    );
    assert_eq!(p.pruned, 1, "{p:?}");
    assert_eq!(active(&b, &b.triage), ["The only copy of the gate code"]);
    assert_eq!(active(&a, &a.triage), ["The boiler was serviced in March"]);
    // And at the next passes too.
    b_to_a.sync_once(&b.brain, util::now()).unwrap();
    assert_eq!(active(&b, &b.triage), ["The only copy of the gate code"]);
}
