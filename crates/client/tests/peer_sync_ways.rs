#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Peers in the ways that go wrong: both sides write, both change the same note, a peer goes away and comes back, a peer
//! speaks another protocol. Every server is a real node on a socket opened by the test (port 0).

use std::sync::Arc;

use memcp_client::peer_sync::Peer;
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::sync;
use memcp_core::text::Language;
use memcp_core::{Brain, Space, users};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};
use serde_json::json;

const NOW: i64 = 1_794_800_000;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("memcp-peer-ways-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Made-up words, so that no two texts look alike to the near-duplicate guard.
fn words(seed: u64) -> String {
    const SYL: [&str; 24] = [
        "ka", "lo", "mi", "ru", "te", "vo", "an", "is", "ul", "be", "do", "fi", "go", "ha", "je",
        "ny", "pa", "qu", "si", "tu", "we", "xa", "yo", "ze",
    ];
    let mut x = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (0..5)
        .map(|_| {
            (0..3)
                .map(|_| {
                    x = x
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    SYL[(x >> 33) as usize % SYL.len()]
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn space() -> Space {
    Space::new("sam-me").unwrap()
}

fn say(b: &Brain, text: &str) -> String {
    let conn = b.open_space(&space()).unwrap();
    match facts::remember(&conn, &space(), "me", NewFact::agent(text, "agent:t"), &[]).unwrap() {
        Remembered::Created(f) => f.id,
        other => panic!("not created: {other:?}"),
    }
}

fn replace(b: &Brain, old: &str, text: &str) {
    let conn = b.open_space(&space()).unwrap();
    facts::supersede(
        &conn,
        &space(),
        "me",
        old,
        NewFact::agent(text, "agent:t"),
        None,
    )
    .unwrap();
}

fn all_texts(b: &Brain) -> Vec<String> {
    let conn = b.open_space(&space()).unwrap();
    let mut st = conn
        .prepare("SELECT text FROM facts ORDER BY text")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

fn active(b: &Brain) -> Vec<String> {
    let conn = b.open_space(&space()).unwrap();
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

struct Server {
    brain: Brain,
    address: String,
    key: String,
    dir: std::path::PathBuf,
}

fn server(name: &str) -> Server {
    let dir = temp(name);
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let key = {
        let ctl = brain.open_control().unwrap();
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .unwrap()
            .id;
        brain.add_space_of_kind(&sam, "sam", "me").unwrap();
        access::connect_for(
            &ctl,
            &sam,
            "peer-box-0001",
            &Rights::parse("sam-me:rw").unwrap(),
            None,
            false,
            false,
        )
        .unwrap()
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Arc::new(App::new(Brain::open(&dir).unwrap()));
    std::thread::spawn(move || {
        let l = Listeners {
            listen: format!("127.0.0.1:{port}"),
            identity: None,
            ingress: None,
            threads: 4,
        };
        let _ = serve_on(app, listener, &l, "");
    });
    Server {
        brain,
        address: format!("http://127.0.0.1:{port}/mcp/peer-box-0001"),
        key,
        dir,
    }
}

fn local(name: &str) -> (Brain, std::path::PathBuf) {
    let dir = temp(name);
    (Brain::init(&dir, &[], Language::English).unwrap(), dir)
}

fn clean(dirs: &[&std::path::PathBuf]) {
    for d in dirs {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn writes_on_both_nodes_meet_and_a_pass_with_nothing_new_changes_nothing() {
    let s = server("both-s");
    let (b, db) = local("both-b");
    let peer = Peer::new(&s.address, &s.key, "peer-server");
    say(&s.brain, "Server: the lease ends in June.");
    peer.sync_once(&b, NOW).unwrap();
    say(&s.brain, "Server: a second note about the lease.");
    say(&b, "Laptop: the invoice was paid on the third.");
    say(&b, "Laptop: the tyres need changing in autumn.");
    let pass = peer.sync_once(&b, NOW + 10).unwrap();
    assert!(pass.problems.is_empty(), "{:?}", pass.problems);
    assert_eq!((pass.pulled, pass.pushed), (1, 2));
    assert_eq!(active(&s.brain), active(&b));
    assert_eq!(active(&b).len(), 4);
    let again = peer.sync_once(&b, NOW + 20).unwrap();
    assert_eq!((again.pulled, again.pushed, again.disputed), (0, 0, 0));
    clean(&[&s.dir, &db]);
}

#[test]
fn the_same_note_changed_on_both_sides_is_kept_both_ways_and_listed_as_a_dispute() {
    let s = server("dispute-s");
    let (b, db) = local("dispute-b");
    let peer = Peer::new(&s.address, &s.key, "peer-server");
    let id = say(&s.brain, "The meeting is on Monday.");
    peer.sync_once(&b, NOW).unwrap();
    assert_eq!(active(&b), ["The meeting is on Monday."]);
    // Each side corrects it, not knowing of the other.
    replace(&s.brain, &id, "The meeting moved to Tuesday.");
    replace(&b, &id, "The meeting moved to Wednesday.");
    for i in 1..=3 {
        let pass = peer.sync_once(&b, NOW + i * 10).unwrap();
        assert!(pass.problems.is_empty(), "{:?}", pass.problems);
    }
    // Nothing is lost: all three versions are on both nodes, and both know the note is in dispute.
    assert_eq!(all_texts(&s.brain), all_texts(&b));
    let all = all_texts(&b);
    for t in [
        "The meeting is on Monday.",
        "The meeting moved to Tuesday.",
        "The meeting moved to Wednesday.",
    ] {
        assert!(all.iter().any(|x| x == t), "{t} is kept: {all:?}");
    }
    for brain in [&s.brain, &b] {
        let conn = brain.open_space(&space()).unwrap();
        let disputes = sync::disputes(&conn).unwrap();
        assert!(disputes.iter().any(|(f, _)| *f == id), "{disputes:?}");
    }
    // The two nodes end in the same state, and nothing more happens.
    assert_eq!(active(&s.brain), active(&b));
    let again = peer.sync_once(&b, NOW + 100).unwrap();
    assert_eq!((again.pulled, again.pushed), (0, 0));
    clean(&[&s.dir, &db]);
}

#[test]
fn a_note_retired_on_one_side_and_corrected_on_the_other_is_in_dispute_not_lost() {
    let s = server("retire-s");
    let (b, db) = local("retire-b");
    let peer = Peer::new(&s.address, &s.key, "peer-server");
    let id = say(&s.brain, "The garage code is one two three four.");
    peer.sync_once(&b, NOW).unwrap();
    facts::retire(&s.brain.open_space(&space()).unwrap(), "me", &id, "wrong").unwrap();
    replace(&b, &id, "The garage code changed in spring.");
    for i in 1..=3 {
        peer.sync_once(&b, NOW + i * 10).unwrap();
    }
    assert_eq!(all_texts(&s.brain), all_texts(&b));
    assert!(
        all_texts(&b)
            .iter()
            .any(|t| t.contains("changed in spring"))
    );
    let disputes = sync::disputes(&b.open_space(&space()).unwrap()).unwrap();
    assert!(disputes.iter().any(|(f, _)| *f == id), "{disputes:?}");
    clean(&[&s.dir, &db]);
}

#[test]
fn a_peer_that_goes_away_is_resumed_where_it_stopped_when_it_returns() {
    let s = server("away-s");
    let (b, db) = local("away-b");
    let peer = Peer::new(&s.address, &s.key, "peer-server");
    for i in 0..5 {
        say(&s.brain, &words(100 + i));
    }
    peer.sync_once(&b, NOW).unwrap();
    let before = sync::cursors(&b, "peer-server", &space()).unwrap();
    // The peer cannot be reached (nothing listens at this address): the pass fails and moves no cursor.
    let gone = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        Peer::new(
            &format!("http://127.0.0.1:{port}/mcp/peer-box-0001"),
            &s.key,
            "peer-server",
        )
    };
    assert!(gone.sync_once(&b, NOW + 10).is_err());
    assert_eq!(sync::cursors(&b, "peer-server", &space()).unwrap(), before);
    // Both sides keep writing while apart.
    for i in 0..4 {
        say(&s.brain, &words(200 + i));
        say(&b, &words(300 + i));
    }
    // It returns: everything crosses, once.
    let pass = peer.sync_once(&b, NOW + 100).unwrap();
    assert!(pass.problems.is_empty(), "{:?}", pass.problems);
    assert_eq!((pass.pulled, pass.pushed), (4, 4));
    assert_eq!(active(&s.brain), active(&b));
    assert_eq!(active(&b).len(), 13);
    let status = sync::status(&b, NOW + 101).unwrap();
    assert!(
        status.iter().any(|p| p.peer == "peer-server"),
        "the peer is in the status"
    );
    clean(&[&s.dir, &db]);
}

#[test]
fn a_peer_cut_short_in_the_middle_of_a_large_catch_up_resumes_without_a_gap_or_a_double() {
    let s = server("cut-s");
    let (b, db) = local("cut-b");
    let peer = Peer::new(&s.address, &s.key, "peer-server");
    for i in 0..650 {
        say(&s.brain, &words(1000 + i));
    }
    // A first pass is cut after the first page by moving the cursor back as a crash would leave it: the second pass
    // reads again from there and applies nothing twice.
    peer.sync_once(&b, NOW).unwrap();
    assert_eq!(active(&b).len(), 650);
    sync::set_cursor(&b, "peer-server", &space(), sync::Cursor::Pulled, 100).unwrap();
    sync::set_cursor(&b, "peer-server", &space(), sync::Cursor::Pushed, 0).unwrap();
    let pass = peer.sync_once(&b, NOW + 10).unwrap();
    assert!(pass.problems.is_empty());
    assert_eq!(active(&b).len(), 650, "no duplicate");
    assert_eq!(active(&s.brain), active(&b));
    clean(&[&s.dir, &db]);
}

/// A node that answers like a peer of another protocol: it lists no space, and says its protocol.
fn fake_peer(protocol: Option<i64>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for rq in server.incoming_requests() {
            let mut body = json!({ "version": "0.0.1", "spaces": [] });
            if let Some(p) = protocol {
                body["protocol"] = json!(p);
            }
            let _ = rq.respond(
                tiny_http::Response::from_string(body.to_string()).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                ),
            );
        }
    });
    format!("http://127.0.0.1:{port}/mcp/peer-x-0001")
}

#[test]
fn a_peer_with_a_newer_protocol_is_refused_with_a_clear_message_and_nothing_is_touched() {
    let (b, db) = local("newer");
    say_in_local(&b);
    let before = all_texts(&b);
    let peer = Peer::new(
        &fake_peer(Some(memcp_core::sync::PROTOCOL + 1)),
        "k",
        "peer-new",
    );
    let err = peer.sync_once(&b, NOW).unwrap_err();
    assert!(
        err.contains("newer exchange") && err.contains("update this machine"),
        "{err}"
    );
    assert_eq!(all_texts(&b), before);
    clean(&[&db]);
}

fn say_in_local(b: &Brain) {
    let ctl = b.open_control().unwrap();
    let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
        .unwrap()
        .id;
    b.add_space_of_kind(&sam, "sam", "me").unwrap();
    say(b, "A note that stays.");
}

#[test]
fn a_peer_with_an_older_protocol_or_none_is_still_served_as_the_first_one() {
    let (b, db) = local("older");
    say_in_local(&b);
    for protocol in [None, Some(0), Some(memcp_core::sync::PROTOCOL)] {
        let peer = Peer::new(&fake_peer(protocol), "k", "peer-old");
        let pass = peer.sync_once(&b, NOW).unwrap();
        assert!(
            pass.problems.is_empty(),
            "{protocol:?}: {:?}",
            pass.problems
        );
        assert_eq!(pass.spaces, 0);
    }
    assert_eq!(all_texts(&b), ["A note that stays."]);
    clean(&[&db]);
}

#[test]
fn a_revoked_peer_is_refused_at_its_next_pass_and_gets_nothing() {
    let s = server("revoked-s");
    let (b, db) = local("revoked-b");
    let peer = Peer::new(&s.address, &s.key, "peer-server");
    say(&s.brain, "Before the revocation.");
    peer.sync_once(&b, NOW).unwrap();
    access::revoke(&s.brain.open_control().unwrap(), "peer-box-0001").unwrap();
    say(&s.brain, "After the revocation.");
    say(&b, "Written on the revoked laptop.");
    assert!(peer.sync_once(&b, NOW + 10).is_err());
    assert_eq!(active(&b).len(), 2, "it received nothing more");
    assert_eq!(
        active(&s.brain).len(),
        2,
        "and the server took nothing from it"
    );
    assert!(
        !active(&s.brain)
            .iter()
            .any(|t| t.contains("revoked laptop")),
        "nothing it wrote reached the server"
    );
    clean(&[&s.dir, &db]);
}
