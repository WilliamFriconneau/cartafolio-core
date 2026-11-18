#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The sync policy per space and per node, between real nodes over HTTP: each node enforces its own side, on the path
//! that sends and on the path that receives, whichever node runs the pass. `off` keeps a space apart both ways while
//! the others go on; `send` and `receive` go one way; a peer that pushes anyway is refused; a space made later follows
//! the choice for spaces made later; back to `both`, everything missed arrives.

use std::sync::Arc;

use memcp_client::peer_sync::{Pass, Peer};
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::policy::{self, Policy};
use memcp_core::text::Language;
use memcp_core::users::{self, OWNER};
use memcp_core::{Brain, Space, sync, util};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};
use serde_json::{Value, json};

fn temp(name: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-policy-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

struct Node {
    brain: Brain,
    dir: std::path::PathBuf,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn sp(s: &str) -> Space {
    Space::new(s).unwrap()
}

fn node(name: &str, spaces: &[&str]) -> Node {
    let dir = temp(name);
    let spaces: Vec<Space> = spaces.iter().map(|s| sp(s)).collect();
    let brain = Brain::init(&dir, &spaces, Language::English).unwrap();
    Node { brain, dir }
}

/// The connection of machine A on node B.
const A_ON_B: &str = "peer-a-0001";
/// The name node A keeps node B under.
const B_FROM_A: &str = "peer-b";

/// Serves `n` with a connection for machine A: its address and key.
fn serve(n: &Node) -> (String, String) {
    let key = access::connect_peer(
        &n.brain.open_control().unwrap(),
        OWNER,
        A_ON_B,
        &Rights::parse("all").unwrap(),
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
    (format!("http://127.0.0.1:{port}/mcp/{A_ON_B}"), key)
}

/// Node A, which runs the passes, and node B, which serves them.
fn pair(spaces: &[&str]) -> (Node, Node, Peer, String, String) {
    let a = node("a", spaces);
    let b = node("b", spaces);
    let (addr, key) = serve(&b);
    let peer = Peer::new(&addr, &key, B_FROM_A);
    (a, b, peer, addr, key)
}

fn pass(peer: &Peer, n: &Node) -> Pass {
    let p = peer.sync_once(&n.brain, util::now()).unwrap();
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    p
}

fn note(n: &Node, space: &str, text: &str) {
    let s = sp(space);
    let conn = n.brain.open_space(&s).unwrap();
    match facts::remember(&conn, &s, "me", NewFact::agent(text, "agent:t"), &[]).unwrap() {
        Remembered::Created(_) => {}
        other => panic!("not created: {other:?}"),
    }
}

fn texts(n: &Node, space: &str) -> Vec<String> {
    if !n.brain.has_space(&sp(space)).unwrap() {
        return vec![];
    }
    let conn = n.brain.open_space(&sp(space)).unwrap();
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

fn owner(n: &Node) -> users::User {
    users::user(&n.brain.open_control().unwrap(), OWNER)
        .unwrap()
        .unwrap()
}

fn set(n: &Node, peer: &str, space: &str, p: Policy) {
    policy::set(&n.brain, &owner(n), peer, &sp(space), p).unwrap();
}

fn view_row(n: &Node, peer: &str, space: &str) -> Value {
    let v = policy::view(&n.brain, &owner(n), peer).unwrap();
    v["spaces"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["space"] == space).cloned())
        .unwrap_or_default()
}

#[test]
fn off_on_the_node_that_runs_the_pass_keeps_a_space_apart_both_ways_and_back_to_both_nothing_is_lost()
 {
    let (a, b, peer, _, _) = pair(&["work", "health"]);
    pass(&peer, &a);
    set(&a, B_FROM_A, "health", Policy::Off);
    note(&a, "health", "A blood test is due in May");
    note(&b, "health", "The dentist moved to Tuesday");
    note(&a, "work", "The badge is renewed in June");
    note(&b, "work", "The office closes on Friday");
    pass(&peer, &a);
    assert_eq!(texts(&a, "health"), ["A blood test is due in May"]);
    assert_eq!(texts(&b, "health"), ["The dentist moved to Tuesday"]);
    assert_eq!(texts(&a, "work"), texts(&b, "work"));
    assert_eq!(texts(&a, "work").len(), 2);
    // `off` erases nothing, and the page says this machine holds a copy from before.
    assert_eq!(view_row(&a, B_FROM_A, "health")["exchanged"], true);
    // Back to both: what was missed on either side arrives.
    set(&a, B_FROM_A, "health", Policy::Both);
    pass(&peer, &a);
    assert_eq!(texts(&a, "health"), texts(&b, "health"));
    assert_eq!(texts(&a, "health").len(), 2);
}

#[test]
fn off_on_the_serving_node_keeps_the_space_apart_and_is_never_listed_there() {
    let (a, b, peer, addr, key) = pair(&["work", "health"]);
    pass(&peer, &a);
    set(&b, A_ON_B, "health", Policy::Off);
    note(&a, "health", "A blood test is due in May");
    note(&b, "health", "The dentist moved to Tuesday");
    note(&a, "work", "The badge is renewed in June");
    pass(&peer, &a);
    assert_eq!(texts(&a, "health"), ["A blood test is due in May"]);
    assert_eq!(texts(&b, "health"), ["The dentist moved to Tuesday"]);
    assert_eq!(texts(&b, "work"), ["The badge is renewed in June"]);
    let listed = get(&addr, &key, "spaces");
    let names: Vec<&str> = listed["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["space"].as_str())
        .collect();
    assert!(!names.contains(&"health"), "{names:?}");
    assert!(names.contains(&"work"), "{names:?}");
    // A, offering its health space, was told no: its page shows the other side.
    assert_eq!(view_row(&a, B_FROM_A, "health")["theirs"], "off");
    assert_eq!(view_row(&a, B_FROM_A, "work")["theirs"], "both");
    // Back to both on B: everything missed arrives.
    set(&b, A_ON_B, "health", Policy::Both);
    pass(&peer, &a);
    assert_eq!(texts(&a, "health").len(), 2);
    assert_eq!(texts(&a, "health"), texts(&b, "health"));
}

#[test]
fn send_and_receive_go_one_way_on_either_side_and_each_page_shows_the_other_side() {
    let (a, b, peer, _, _) = pair(&["work", "home"]);
    pass(&peer, &a);
    // A sends its work to B and takes none of B's.
    set(&a, B_FROM_A, "work", Policy::Send);
    // B takes home from A and sends it none.
    set(&b, A_ON_B, "home", Policy::Receive);
    note(&a, "work", "The badge is renewed in June");
    note(&b, "work", "The office closes on Friday");
    note(&a, "home", "The boiler is serviced in March");
    note(&b, "home", "The gutter needs cleaning");
    pass(&peer, &a);
    assert_eq!(texts(&a, "work"), ["The badge is renewed in June"]);
    assert_eq!(
        texts(&b, "work"),
        [
            "The badge is renewed in June",
            "The office closes on Friday"
        ]
    );
    assert_eq!(texts(&a, "home"), ["The boiler is serviced in March"]);
    assert_eq!(
        texts(&b, "home"),
        [
            "The boiler is serviced in March",
            "The gutter needs cleaning"
        ]
    );
    // Each node tells its side: A read B's in its list, B was told A's.
    assert_eq!(view_row(&a, B_FROM_A, "home")["theirs"], "receive");
    assert_eq!(view_row(&b, A_ON_B, "work")["theirs"], "send");
    assert_eq!(view_row(&b, A_ON_B, "home")["policy"], "receive");
}

fn get(addr: &str, key: &str, path: &str) -> Value {
    let (status, v) = call(addr, key, "GET", path, None);
    assert_eq!(status, 200, "{path}: {v}");
    v
}

fn call(addr: &str, key: &str, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{addr}/sync/{path}");
    let auth = format!("Bearer {key}");
    let mut res = match (method, body) {
        ("GET", _) => agent.get(&url).header("Authorization", &auth).call(),
        (_, b) => agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send(b.cloned().unwrap_or(Value::Null).to_string().as_bytes()),
    }
    .unwrap();
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[test]
fn a_peer_that_ignores_the_policy_and_pushes_anyway_is_refused_for_that_space() {
    let (a, b, peer, addr, key) = pair(&["work", "health"]);
    pass(&peer, &a);
    set(&b, A_ON_B, "health", Policy::Off);
    note(&a, "health", "A blood test is due in May");
    note(&b, "health", "The dentist moved to Tuesday");
    // A node of an older version, or one that does not care: it pushes, pulls and offers all the same.
    let page = sync::events_after(&a.brain, &sp("health"), 0, 500, 4 << 20, None).unwrap();
    assert!(!page.events.is_empty());
    let body =
        json!({ "events": page.events.iter().map(sync::Event::to_json).collect::<Vec<_>>() });
    let (status, _) = call(&addr, &key, "POST", "events?space=health", Some(&body));
    assert_eq!(status, 403);
    let (status, _) = call(&addr, &key, "GET", "events?space=health&after=0", None);
    assert_eq!(status, 403);
    let offer = json!({ "space": "health", "kind": "custom", "since": 1 });
    let (status, answer) = call(&addr, &key, "POST", "spaces", Some(&offer));
    assert_eq!((status, answer["result"].as_str()), (200, Some("declined")));
    let archive = json!({ "space": "health", "archived_at": util::now() });
    let (status, answer) = call(&addr, &key, "POST", "spaces", Some(&archive));
    assert_eq!((status, answer["result"].as_str()), (200, Some("kept")));
    assert!(b.brain.has_space(&sp("health")).unwrap());
    assert_eq!(texts(&b, "health"), ["The dentist moved to Tuesday"]);
    // `receive` on B: A's events are taken, B sends none.
    set(&b, A_ON_B, "health", Policy::Receive);
    let (status, _) = call(&addr, &key, "GET", "events?space=health&after=0", None);
    assert_eq!(status, 403);
    let (status, _) = call(&addr, &key, "POST", "events?space=health", Some(&body));
    assert_eq!(status, 200);
    assert_eq!(texts(&b, "health").len(), 2);
    // `send` on B: B's events go, A's are refused.
    set(&b, A_ON_B, "work", Policy::Send);
    note(&a, "work", "The badge is renewed in June");
    let page = sync::events_after(&a.brain, &sp("work"), 0, 500, 4 << 20, None).unwrap();
    let body =
        json!({ "events": page.events.iter().map(sync::Event::to_json).collect::<Vec<_>>() });
    let (status, _) = call(&addr, &key, "POST", "events?space=work", Some(&body));
    assert_eq!(status, 403);
    let (status, _) = call(&addr, &key, "GET", "events?space=work&after=0", None);
    assert_eq!(status, 200);
    assert!(texts(&b, "work").is_empty());
}

#[test]
fn a_space_made_later_follows_the_choice_for_spaces_made_later_and_joins_when_turned_on() {
    let (a, b, peer, _, _) = pair(&["work"]);
    pass(&peer, &a);
    policy::set_later(&a.brain, &owner(&a), B_FROM_A, Policy::Off).unwrap();
    // Made after the choice, on either side.
    a.brain.add_space_for(&sp("travel"), OWNER).unwrap();
    note(&a, "travel", "The train leaves at nine");
    b.brain.add_space_for(&sp("garden"), OWNER).unwrap();
    note(&b, "garden", "The roses need water");
    note(&a, "work", "The badge is renewed in June");
    pass(&peer, &a);
    assert!(!b.brain.has_space(&sp("travel")).unwrap());
    assert!(!a.brain.has_space(&sp("garden")).unwrap());
    assert_eq!(texts(&b, "work"), ["The badge is renewed in June"]);
    // Turned on for one of them: it is made there with what it holds.
    set(&a, B_FROM_A, "travel", Policy::Both);
    pass(&peer, &a);
    assert_eq!(texts(&b, "travel"), ["The train leaves at nine"]);
    assert!(!a.brain.has_space(&sp("garden")).unwrap());
}
