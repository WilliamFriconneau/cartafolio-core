#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The peer directory over real HTTP: three nodes in a line (A-B, B-C) where C reaches A's list through B, a revoked
//! peer that disappears everywhere, versions, and an old node without the endpoint.

use std::sync::Arc;

use memcp_client::peer_sync::Peer;
use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::{Brain, Space, directory, sync, users};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};
use serde_json::{Value, json};

const NOW: i64 = 1_794_800_000;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("memcp-peer-dir-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A node with one person and one space, named.
fn node(tag: &str, name: &str) -> (Brain, std::path::PathBuf) {
    let dir = temp(tag);
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    {
        let ctl = brain.open_control().unwrap();
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .unwrap()
            .id;
        brain.add_vault(&sam, "sam", "me").unwrap();
    }
    directory::set_own_name(&brain, name).unwrap();
    (brain, dir)
}

/// Serves `dir` over HTTP, with one machine of the person per entry of `connections` (every space of theirs, as a join
/// gives); returns each one's address and key.
fn serve(dir: &std::path::Path, connections: &[&str]) -> Vec<(String, String)> {
    let brain = Brain::open(dir).unwrap();
    let ctl = brain.open_control().unwrap();
    let sam = users::user_by_name(&ctl, "sam").unwrap().unwrap().id;
    let keys: Vec<String> = connections
        .iter()
        .map(|c| {
            access::connect_peer(&ctl, &sam, c, &Rights::parse("all").unwrap(), false).unwrap()
        })
        .collect();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Arc::new(App::new(brain));
    std::thread::spawn(move || {
        let l = Listeners {
            listen: format!("127.0.0.1:{port}"),
            identity: None,
            ingress: None,
            threads: 4,
        };
        let _ = serve_on(app, listener, &l, "");
    });
    connections
        .iter()
        .zip(keys)
        .map(|(c, k)| (format!("http://127.0.0.1:{port}/mcp/{c}"), k))
        .collect()
}

fn names(b: &Brain, now: i64) -> Vec<String> {
    sync::peers_view(b, now)
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

fn find(b: &Brain, name: &str, now: i64) -> Option<Value> {
    sync::peers_view(b, now)
        .unwrap()
        .into_iter()
        .find(|r| r["name"] == name)
}

struct Line {
    a: Brain,
    b: Brain,
    c: Brain,
    a_to_b: Peer,
    c_to_b: Peer,
    dirs: Vec<std::path::PathBuf>,
}

/// A-B and B-C: B is served and has a connection for each of the others.
fn line(tag: &str) -> Line {
    let (a, da) = node(&format!("{tag}-a"), "alpha");
    let (b, db) = node(&format!("{tag}-b"), "bravo");
    let (c, dc) = node(&format!("{tag}-c"), "charlie");
    let served = serve(&db, &["peer-alpha-0001", "peer-charlie-0001"]);
    Line {
        a_to_b: Peer::new(&served[0].0, &served[0].1, "peer-bravo"),
        c_to_b: Peer::new(&served[1].0, &served[1].1, "peer-bravo"),
        a,
        b,
        c,
        dirs: vec![da, db, dc],
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

#[test]
fn c_appears_in_a_list_after_two_passes_with_b_in_between() {
    let l = line("line");
    // A has only met B.
    let first = l.a_to_b.sync_once(&l.a, NOW).unwrap();
    assert!(first.problems.is_empty(), "{:?}", first.problems);
    assert_eq!(names(&l.a, NOW), ["bravo"]);
    // C meets B: C learns A from B, and B learns C from C.
    l.c_to_b.sync_once(&l.c, NOW + 10).unwrap();
    assert_eq!(names(&l.c, NOW + 10), ["bravo", "alpha"]);
    assert!(find(&l.a, "charlie", NOW + 10).is_none(), "not yet");
    // A's second pass with B brings C.
    l.a_to_b.sync_once(&l.a, NOW + 20).unwrap();
    let c = find(&l.a, "charlie", NOW + 20).expect("charlie in alpha's list");
    assert_eq!(c["learned_from"], "bravo");
    assert_eq!(c["direct"], false);
    assert_eq!(c["last_seen"], NOW + 10);
    assert_eq!(c["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(c["behind"], false);
    // B knows both as direct peers, by the connection each uses, and has a progress for them.
    let on_b = sync::peers_view(&l.b, NOW + 20).unwrap();
    let alpha = on_b.iter().find(|r| r["name"] == "alpha").unwrap();
    assert_eq!(alpha["direct"], true);
    assert_eq!(alpha["connection"], "peer-alpha-0001");
    assert!(on_b.iter().any(|r| r["name"] == "charlie"));
    // A reaches B directly, with the direction and the percentage.
    let bravo = find(&l.a, "bravo", NOW + 20).unwrap();
    assert_eq!(bravo["direct"], true);
    assert_eq!(bravo["percent"], 100);
    assert_eq!(bravo["connection"], "peer-bravo");
    // Nothing was made up: C's activity is the one C wrote, not the time A heard of it.
    assert_eq!(
        find(&l.a, "charlie", NOW + 9999).unwrap()["last_seen"],
        NOW + 10
    );
}

#[test]
fn a_revoked_peer_disappears_everywhere_and_stays_gone() {
    let l = line("revoke");
    l.a_to_b.sync_once(&l.a, NOW).unwrap();
    l.c_to_b.sync_once(&l.c, NOW + 10).unwrap();
    l.a_to_b.sync_once(&l.a, NOW + 20).unwrap();
    assert!(find(&l.a, "charlie", NOW + 20).is_some());
    // The person revokes C's connection on B.
    {
        let ctl = l.b.open_control().unwrap();
        assert!(access::revoke(&ctl, "peer-charlie-0001").unwrap());
    }
    assert!(find(&l.b, "charlie", NOW + 30).is_none());
    // A's next pass brings the flag; C's own pass is refused (its key is cancelled) and cannot bring it back.
    l.a_to_b.sync_once(&l.a, NOW + 40).unwrap();
    assert!(find(&l.a, "charlie", NOW + 40).is_none());
    assert!(l.c_to_b.sync_once(&l.c, NOW + 50).is_err());
    l.a_to_b.sync_once(&l.a, NOW + 60).unwrap();
    assert!(find(&l.a, "charlie", NOW + 60).is_none());
    assert!(find(&l.b, "charlie", NOW + 60).is_none());
    // Kept, flagged.
    assert!(
        directory::entries(&l.a)
            .unwrap()
            .iter()
            .any(|e| e.name == "charlie" && e.revoked)
    );
}

#[test]
fn versions_are_carried_and_an_older_one_is_behind() {
    let l = line("version");
    // C is an older build: tell B so with its own entry, as it would on a pass.
    directory::merge(
        &l.b,
        "charlie",
        &[json!({ "name": "charlie", "last_seen": NOW, "version": "20200101.0.1" })],
        NOW,
    )
    .unwrap();
    l.a_to_b.sync_once(&l.a, NOW + 5).unwrap();
    let c = find(&l.a, "charlie", NOW + 5).unwrap();
    assert_eq!(c["version"], "20200101.0.1");
    assert_eq!(c["behind"], true);
    let b = find(&l.a, "bravo", NOW + 5).unwrap();
    assert_eq!(b["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(b["behind"], false);
}

#[test]
fn exposures_reach_the_other_nodes() {
    let l = line("expose");
    directory::set_exposures(
        &l.c,
        &[json!({"kind": "loopback", "address": "127.0.0.1:8791"})],
    )
    .unwrap();
    l.c_to_b.sync_once(&l.c, NOW).unwrap();
    l.a_to_b.sync_once(&l.a, NOW + 10).unwrap();
    let c = find(&l.a, "charlie", NOW + 10).unwrap();
    assert_eq!(c["exposures"][0]["kind"], "loopback");
    assert_eq!(c["only_local"], true);
    assert_eq!(c["note"], "syncs_on_open");
}

/// An old node: it answers the exchange but has no `/sync/peers`.
fn old_node() -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for rq in server.incoming_requests() {
            let (code, body) = if rq.url().ends_with("/sync/spaces") {
                (
                    200,
                    json!({ "protocol": 1, "version": "20260101.0.1", "spaces": [] }),
                )
            } else {
                (404, json!({ "error": "not found" }))
            };
            let _ = rq.respond(
                tiny_http::Response::from_string(body.to_string())
                    .with_status_code(code)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
            );
        }
    });
    format!("http://127.0.0.1:{port}/mcp/peer-x-0001")
}

#[test]
fn an_old_node_without_the_endpoint_is_tolerated() {
    let (a, da) = node("old-a", "alpha");
    let pass = Peer::new(&old_node(), "k", "peer-old")
        .sync_once(&a, NOW)
        .unwrap();
    // The person of this node is sam (the one who owns its spaces): the only thing said is that the old node takes
    // no new space; the missing directory endpoint is no problem.
    assert!(
        pass.problems
            .iter()
            .all(|p| p.starts_with("the peer runs an older version")),
        "{:?}",
        pass.problems
    );
    // Only this node's own entry; nobody was invented.
    assert!(names(&a, NOW).is_empty());
    assert_eq!(directory::entries(&a).unwrap().len(), 1);
    // The new node still answers an old client that never asks for peers: the other routes are unchanged.
    let (b, db) = node("old-b", "bravo");
    let served = serve(&db, &["peer-alpha-0001"]);
    let space = Space::new("sam-me").unwrap();
    assert!(b.open_space(&space).is_ok());
    let pass = Peer::new(&served[0].0, &served[0].1, "peer-bravo")
        .sync_once(&a, NOW + 1)
        .unwrap();
    assert!(pass.problems.is_empty(), "{:?}", pass.problems);
    for d in [da, db] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn the_status_and_the_endpoint_carry_the_directory() {
    let (b, db) = node("http-b", "bravo");
    let served = serve(&db, &["peer-alpha-0001"]);
    directory::merge(
        &b,
        "x",
        &[json!({"name": "zulu", "last_seen": NOW, "version": "20200101.0.1"})],
        NOW,
    )
    .unwrap();
    let get = |path: &str| -> Value {
        let mut res = ureq::get(&format!("{}/sync/{path}", served[0].0))
            .header("Authorization", &format!("Bearer {}", served[0].1))
            .call()
            .unwrap();
        serde_json::from_str(&res.body_mut().read_to_string().unwrap()).unwrap()
    };
    let peers = get("peers");
    assert_eq!(peers["node"]["name"], "bravo");
    let list = peers["peers"].as_array().unwrap();
    assert!(
        list.iter().any(|e| e["name"] == "bravo"),
        "its own entry, written when it served"
    );
    assert!(
        list.iter()
            .any(|e| e["name"] == "zulu" && e["version"] == "20200101.0.1")
    );
    let status = get("status");
    assert_eq!(status["node"]["name"], "bravo");
    let zulu = status["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "zulu")
        .unwrap();
    assert_eq!(zulu["behind"], true);
    assert!(status["connections"].is_array());
    let _ = std::fs::remove_dir_all(db);
}
