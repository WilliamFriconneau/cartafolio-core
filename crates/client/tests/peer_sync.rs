#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Two real nodes kept in step over HTTP: a first full copy, writes on both sides, a pass that brings both to the same
//! memory, a second pass that changes nothing, a wrong key refused, and the live status at 100 %.

use std::sync::Arc;
use std::time::Duration;

use memcp_client::peer_sync::Peer;
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::text::Language;
use memcp_core::{Brain, Space, users};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};

const NOW: i64 = 1_794_800_000;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("memcp-peer-sync-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write(b: &Brain, space: &Space, text: &str) {
    let conn = b.open_space(space).expect("space");
    match facts::remember(&conn, space, "me", NewFact::agent(text, "agent:t"), &[])
        .expect("remember")
    {
        Remembered::Created(_) => {}
        other => panic!("not created: {other:?}"),
    }
}

fn texts(b: &Brain, space: &Space) -> Vec<String> {
    let conn = b.open_space(space).expect("space");
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")
        .expect("query");
    st.query_map([], |r| r.get::<_, String>(0))
        .expect("rows")
        .flatten()
        .collect()
}

/// The server node: one person, one space, a peer connection; served over HTTP on a free port.
fn server() -> (Brain, String, String, std::path::PathBuf) {
    let dir = temp("server");
    let brain = Brain::init(&dir, &[], Language::English).expect("init");
    let token = {
        let ctl = brain.open_control().expect("control");
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .expect("user")
            .id;
        brain.add_space_of_kind(&sam, "sam", "me").expect("vault");
        access::connect_for(
            &ctl,
            &sam,
            "peer-mac-ab12",
            &Rights::parse("sam-me:rw").expect("rights"),
            None,
            false,
            false,
        )
        .expect("connect")
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let app = Arc::new(App::new(Brain::open(&dir).expect("open")));
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
        std::thread::sleep(Duration::from_millis(100));
    }
    (
        brain,
        format!("http://127.0.0.1:{port}/mcp/peer-mac-ab12"),
        token,
        dir,
    )
}

#[test]
fn two_nodes_end_with_the_same_memory_and_a_second_pass_changes_nothing() {
    let (srv, address, token, sdir) = server();
    let space = Space::new("sam-me").expect("space");
    for text in [
        "The boiler was serviced in March",
        "Buy flour and eggs for Sunday",
        "The car needs new tyres before winter",
    ] {
        write(&srv, &space, text);
    }
    let mdir = temp("mac");
    let mac = Brain::init(&mdir, &[], Language::English).expect("mac");
    let peer = Peer::new(&address, &token, "peer-server");

    // First pass: the Mac had nothing, it gets the space and its three facts.
    let first = peer.sync_once(&mac, NOW).expect("first pass");
    assert!(first.problems.is_empty(), "{:?}", first.problems);
    assert_eq!((first.spaces, first.pulled), (1, 3));
    assert_eq!(texts(&mac, &space), texts(&srv, &space));

    // Writes on both sides, then one pass: the same six facts on both.
    write(&mac, &space, "Call the dentist about the crown");
    write(&mac, &space, "Read the chapter on tides tonight");
    write(&srv, &space, "The garden hose leaks at the tap");
    let second = peer.sync_once(&mac, NOW + 60).expect("second pass");
    assert!(second.problems.is_empty(), "{:?}", second.problems);
    assert_eq!((second.pulled, second.pushed), (1, 2));
    let both = texts(&srv, &space);
    assert_eq!(both.len(), 6, "{both:?}");
    assert_eq!(texts(&mac, &space), both);

    // Again: nothing to do, nothing duplicated.
    let third = peer.sync_once(&mac, NOW + 120).expect("third pass");
    assert_eq!((third.pulled, third.pushed, third.disputed), (0, 0, 0));
    assert_eq!(texts(&mac, &space).len(), 6);
    assert_eq!(texts(&srv, &space).len(), 6);

    // The server shows this peer, in step.
    let status = memcp_core::sync::status(&srv, NOW + 130).expect("status");
    let me = status
        .iter()
        .find(|p| p.peer == "peer-mac-ab12")
        .expect("the Mac is listed");
    assert!(me.spaces.iter().all(|s| s.percent == 100), "{me:?}");

    // A wrong key is refused and changes nothing.
    let stranger = Peer::new(&address, "not-a-key", "peer-server");
    assert!(stranger.sync_once(&mac, NOW + 140).is_err());
    assert_eq!(texts(&srv, &space).len(), 6);

    let _ = std::fs::remove_dir_all(sdir);
    let _ = std::fs::remove_dir_all(mdir);
}
