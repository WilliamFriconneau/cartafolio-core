#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End to end, over HTTP, against real nodes: writes from many threads at once, three nodes writing everywhere and
//! converging, the doors that must stay shut, and a new node catching up with thousands of notes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use memcp_client::Client;
use memcp_client::peer_sync::Peer;
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::text::Language;
use memcp_core::{Brain, Space, users};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};
use serde_json::json;

const NOW: i64 = 1_794_800_000;

/// Made-up words, so that no two texts look alike to the near-duplicate guard.
fn words(seed: u64, n: usize) -> String {
    const SYL: [&str; 24] = [
        "ka", "lo", "mi", "ru", "te", "vo", "an", "is", "ul", "be", "do", "fi", "go", "ha", "je",
        "ny", "pa", "qu", "si", "tu", "we", "xa", "yo", "ze",
    ];
    let mut x = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (0..n)
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

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("memcp-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn count(b: &Brain, space: &Space) -> i64 {
    b.open_space(space)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM facts WHERE status = 'active'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn texts(b: &Brain, space: &Space) -> Vec<String> {
    let conn = b.open_space(space).unwrap();
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

fn write(b: &Brain, space: &Space, text: &str) {
    let conn = b.open_space(space).unwrap();
    match facts::remember(&conn, space, "me", NewFact::agent(text, "agent:t"), &[]).unwrap() {
        Remembered::Created(_) => {}
        other => panic!("not created: {other:?}"),
    }
}

struct Node {
    brain: Brain,
    base: String,
    tokens: Vec<String>,
    dir: std::path::PathBuf,
}

/// A served node: one person, the space `sam-me`, the connections asked for (name, rights).
fn node(name: &str, connections: &[(&str, &str)]) -> Node {
    let dir = temp(name);
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let tokens = {
        let ctl = brain.open_control().unwrap();
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .unwrap()
            .id;
        brain.add_space_of_kind(&sam, "sam", "me").unwrap();
        connections
            .iter()
            .map(|(agent, rights)| {
                access::connect_for(
                    &ctl,
                    &sam,
                    agent,
                    &Rights::parse(rights).unwrap(),
                    None,
                    false,
                    false,
                )
                .unwrap()
            })
            .collect()
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Arc::new(App::new(Brain::open(&dir).unwrap()));
    std::thread::spawn(move || {
        let l = Listeners {
            listen: format!("127.0.0.1:{port}"),
            identity: None,
            ingress: None,
            threads: 8,
        };
        let _ = serve_on(app, listener, &l, "");
    });
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Node {
        brain,
        base: format!("http://127.0.0.1:{port}"),
        tokens,
        dir,
    }
}

fn space() -> Space {
    Space::new("sam-me").unwrap()
}

#[test]
fn many_writers_at_once_over_http_lose_nothing() {
    let n = node("writers", &[("writer", "sam-me:rw")]);
    let url = format!("{}/mcp/writer", n.base);
    let threads: Vec<_> = (0..8u64)
        .map(|t| {
            let (url, key) = (url.clone(), n.tokens[0].clone());
            std::thread::spawn(move || {
                let mut c = Client::new(&url, &key);
                c.initialize().unwrap();
                for i in 0..40u64 {
                    let r = c
                        .call(
                            "remember",
                            json!({ "space": "sam-me", "text": words(t * 1000 + i, 5) }),
                        )
                        .unwrap();
                    assert!(r.to_string().contains("created"), "{r}");
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(count(&n.brain, &space()), 320);
    let _ = std::fs::remove_dir_all(&n.dir);
}

#[test]
fn three_nodes_writing_everywhere_end_with_the_same_memory() {
    let s = node(
        "three-server",
        &[("peer-a-0001", "sam-me:rw"), ("peer-b-0002", "sam-me:rw")],
    );
    for i in 0..30 {
        write(&s.brain, &space(), &words(10_000 + i, 5));
    }
    let (da, db) = (temp("three-a"), temp("three-b"));
    let a = Brain::init(&da, &[], Language::English).unwrap();
    let b = Brain::init(&db, &[], Language::English).unwrap();
    let pa = Peer::new(
        &format!("{}/mcp/peer-a-0001", s.base),
        &s.tokens[0],
        "peer-server",
    );
    let pb = Peer::new(
        &format!("{}/mcp/peer-b-0002", s.base),
        &s.tokens[1],
        "peer-server",
    );
    pa.sync_once(&a, NOW).unwrap();
    pb.sync_once(&b, NOW).unwrap();
    for i in 0..20 {
        write(&a, &space(), &words(20_000 + i, 5));
        write(&b, &space(), &words(30_000 + i, 5));
        write(&s.brain, &space(), &words(40_000 + i, 5));
    }
    // A, then B, then A again: every node has seen every write.
    for (peer, brain) in [(&pa, &a), (&pb, &b), (&pa, &a)] {
        let pass = peer.sync_once(brain, NOW + 60).unwrap();
        assert!(pass.problems.is_empty(), "{:?}", pass.problems);
    }
    let all = texts(&s.brain, &space());
    assert_eq!(all.len(), 90);
    assert_eq!(texts(&a, &space()), all);
    assert_eq!(texts(&b, &space()), all);
    for d in [&s.dir, &da, &db] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn the_doors_stay_shut() {
    let s = node(
        "doors",
        &[("peer-a-0001", "sam-me:rw"), ("peer-r-0003", "sam-me:r")],
    );
    write(&s.brain, &space(), &words(1, 5));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let get = |path: &str, key: &str| {
        agent
            .get(&format!("{}{path}", s.base))
            .header("Authorization", &format!("Bearer {key}"))
            .call()
            .unwrap()
            .status()
            .as_u16()
    };
    // No key, a wrong key, and a right key at another connection's address.
    assert_eq!(
        agent
            .get(&format!("{}/mcp/peer-a-0001/sync/spaces", s.base))
            .call()
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    assert_eq!(get("/mcp/peer-a-0001/sync/spaces", "hcp_wrong"), 401);
    assert_eq!(get("/mcp/peer-r-0003/sync/spaces", &s.tokens[0]), 403);
    // A space name that tries to leave its place, and a space the key does not reach.
    assert_eq!(
        get(
            "/mcp/peer-a-0001/sync/events?space=../control",
            &s.tokens[0]
        ),
        400
    );
    assert_eq!(
        get("/mcp/peer-a-0001/sync/events?space=sam-other", &s.tokens[0]),
        403
    );
    // A read-only peer reads but cannot push.
    let dr = temp("doors-r");
    let r = Brain::init(&dr, &[], Language::English).unwrap();
    let pr = Peer::new(
        &format!("{}/mcp/peer-r-0003", s.base),
        &s.tokens[1],
        "peer-server",
    );
    pr.sync_once(&r, NOW).unwrap();
    write(&r, &space(), &words(2, 5));
    let push = agent
        .post(&format!(
            "{}/mcp/peer-r-0003/sync/events?space=sam-me",
            s.base
        ))
        .header("Authorization", &format!("Bearer {}", s.tokens[1]))
        .send(json!({ "events": [] }).to_string().as_bytes())
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(push, 403);
    assert_eq!(count(&s.brain, &space()), 1);
    // Malformed events are counted, not kept.
    let bad = agent
        .post(&format!(
            "{}/mcp/peer-a-0001/sync/events?space=sam-me",
            s.base
        ))
        .header("Authorization", &format!("Bearer {}", s.tokens[0]))
        .send(
            json!({ "events": [ { "eid": "x" }, "nonsense", 42 ] })
                .to_string()
                .as_bytes(),
        )
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    assert!(bad.contains("\"malformed\":3"), "{bad}");
    assert_eq!(count(&s.brain, &space()), 1);
    for d in [&s.dir, &dr] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_new_node_catches_up_with_two_thousand_notes_in_pages() {
    let s = node("bulk", &[("peer-n-0004", "sam-me:rw")]);
    for i in 0..2000 {
        write(&s.brain, &space(), &words(50_000 + i, 6));
    }
    let dn = temp("bulk-n");
    let fresh = Brain::init(&dn, &[], Language::English).unwrap();
    let p = Peer::new(
        &format!("{}/mcp/peer-n-0004", s.base),
        &s.tokens[0],
        "peer-server",
    );
    let started = Instant::now();
    let pass = p.sync_once(&fresh, NOW).unwrap();
    let took = started.elapsed();
    assert!(pass.problems.is_empty(), "{:?}", pass.problems);
    assert_eq!(count(&fresh, &space()), 2000);
    assert!(took < Duration::from_secs(60), "too slow: {took:?}");
    eprintln!("2000 notes caught up in {took:?}");
    for d in [&s.dir, &dn] {
        let _ = std::fs::remove_dir_all(d);
    }
}
