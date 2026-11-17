#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! One person, two nodes that know them under two ids. A laptop starts its own memory: its person is the bootstrap
//! user (`owner`). A home installation holds several people, one of them the same human under another id
//! (`robin`). The laptop joins the home node as a peer of `robin`. The peer connection says whose memory it is:
//! the laptop's person and `robin` are the same human, and their box and spaces go both ways. Nothing of another
//! person of the home node ever crosses.

use std::sync::Arc;

use memcp_client::peer_sync::Peer;
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::text::Language;
use memcp_core::users::{self, OWNER};
use memcp_core::{Brain, Space, triage, util};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};

fn temp(name: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-person-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The laptop: a memory of its own, its person the bootstrap user, its box `owner-a-trier`.
struct Laptop {
    brain: Brain,
    triage: Space,
    _dir: Dir,
}

fn laptop() -> Laptop {
    let dir = temp("laptop");
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let triage = triage::ensure(&brain, OWNER, "owner").unwrap();
    Laptop {
        brain,
        triage,
        _dir: Dir(dir),
    }
}

/// The home node: the bootstrap user stays someone else (`owner`, with a box and a space of its own), `robin` is a
/// person made later (an id of the form `u_…`), and `carol` a third person. Served on a socket of the test.
struct Home {
    brain: Brain,
    robin: String,
    triage: Space,
    others: Vec<Space>,
    base: String,
    _dir: Dir,
}

fn home() -> Home {
    let dir = temp("home");
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let ctl = brain.open_control().unwrap();
    let robin = users::create_user(&ctl, "test", "robin", "Robin", false)
        .unwrap()
        .id;
    let carol = users::create_user(&ctl, "test", "carol", "Carol", false)
        .unwrap()
        .id;
    assert_ne!(robin, OWNER);
    let triage = triage::ensure(&brain, &robin, "robin").unwrap();
    // Other people's boxes and spaces: never the laptop's.
    let mut others = vec![
        triage::ensure(&brain, OWNER, "owner").unwrap(),
        triage::ensure(&brain, &carol, "carol").unwrap(),
    ];
    others.push(
        brain
            .add_custom_space(&carol, "carol", "Diary", None)
            .unwrap(),
    );
    others.push(
        brain
            .add_custom_space(OWNER, "owner", "Ledger", None)
            .unwrap(),
    );
    for s in &others {
        note(&brain, s, &format!("Private to another person in {s}"));
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Arc::new(App::new(Brain::open(&dir).unwrap()));
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
    Home {
        brain,
        robin,
        triage,
        others,
        base: format!("http://127.0.0.1:{port}"),
        _dir: Dir(dir),
    }
}

/// A peer connection on the home node for `user`, as a join makes it: its address and key.
fn join(h: &Home, user: &str, machine: &str) -> (String, String) {
    let agent = format!("peer-{machine}-0001");
    let key = access::connect_peer(
        &h.brain.open_control().unwrap(),
        user,
        &agent,
        &Rights::parse("all").unwrap(),
        false,
    )
    .unwrap();
    (format!("{}/mcp/{agent}", h.base), key)
}

fn note(brain: &Brain, space: &Space, text: &str) -> String {
    let conn = brain.open_space(space).unwrap();
    let mut new = NewFact::agent(text, "agent:dictation");
    new.tags = vec!["inbox".into()];
    match facts::remember(&conn, space, "dictation", new, &[]).unwrap() {
        Remembered::Created(f) => f.id,
        other => panic!("not created: {other:?}"),
    }
}

fn active(brain: &Brain, space: &Space) -> Vec<String> {
    let conn = brain.open_space(space).unwrap();
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

fn pass(peer: &Peer, brain: &Brain) -> memcp_client::peer_sync::Pass {
    let p = peer.sync_once(brain, util::now()).unwrap();
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    p
}

/// No text of another person of the home node is anywhere on the laptop.
fn nothing_of_others(l: &Laptop, h: &Home) {
    for s in l.brain.spaces().unwrap() {
        for t in active(&l.brain, &s) {
            assert!(!t.starts_with("Private to another person"), "{s}: {t}");
        }
    }
    for s in &h.others {
        assert!(!l.brain.has_space(s).unwrap() || s == &l.triage, "{s}");
    }
}

#[test]
fn a_laptop_joined_to_a_person_of_another_id_shares_their_box_both_ways() {
    let l = laptop();
    let h = home();
    let (address, key) = join(&h, &h.robin, "laptop");
    let to_home = Peer::new(&address, &key, "peer-home");

    note(
        &l.brain,
        &l.triage,
        "Call the plumber about the kitchen tap",
    );
    note(&h.brain, &h.triage, "Book the car service for December");
    pass(&to_home, &l.brain);
    pass(&to_home, &l.brain);
    let both = vec![
        "Book the car service for December".to_string(),
        "Call the plumber about the kitchen tap".to_string(),
    ];
    assert_eq!(active(&h.brain, &h.triage), both);
    assert_eq!(active(&l.brain, &l.triage), both);
    // One box for the laptop's person; the home box was paired, not copied beside it.
    let boxes: Vec<Space> = l
        .brain
        .spaces()
        .unwrap()
        .into_iter()
        .filter(|s| triage::is_triage(&l.brain, s))
        .collect();
    assert_eq!(boxes, std::slice::from_ref(&l.triage));
    nothing_of_others(&l, &h);
    // The other people's boxes at home did not get the laptop's note.
    for s in &h.others {
        assert!(
            !active(&h.brain, s).iter().any(|t| t.contains("plumber")),
            "{s}"
        );
    }
}

#[test]
fn a_space_made_on_the_laptop_is_the_persons_at_home_and_back() {
    let l = laptop();
    let h = home();
    let (address, key) = join(&h, &h.robin, "laptop");
    let to_home = Peer::new(&address, &key, "peer-home");

    let music = l
        .brain
        .add_custom_space(OWNER, "owner", "Music", None)
        .unwrap();
    note(&l.brain, &music, "Tune the guitar before Sunday");
    let p = pass(&to_home, &l.brain);
    assert_eq!(p.offered, 1, "{p:?}");
    assert!(
        users::is_space_owner(&h.brain.open_control().unwrap(), &music, &h.robin).unwrap(),
        "the laptop's space is robin's at home"
    );
    assert_eq!(active(&h.brain, &music), ["Tune the guitar before Sunday"]);

    // A space robin makes at home reaches the laptop as its person's.
    let garden = h
        .brain
        .add_custom_space(&h.robin, "robin", "Garden", None)
        .unwrap();
    note(&h.brain, &garden, "Prune the apple tree in February");
    pass(&to_home, &l.brain);
    assert!(users::is_space_owner(&l.brain.open_control().unwrap(), &garden, OWNER).unwrap());
    assert_eq!(
        active(&l.brain, &garden),
        ["Prune the apple tree in February"]
    );
    nothing_of_others(&l, &h);

    // Nothing is offered twice.
    let again = pass(&to_home, &l.brain);
    assert_eq!((again.offered, again.pulled, again.pushed), (0, 0, 0));
}

#[test]
fn a_laptop_whose_person_is_not_the_bootstrap_user_still_shares_their_box_and_spaces() {
    // The laptop's person was made as a person of their own (an id `u_…`); the bootstrap user holds nothing there.
    let dir = temp("laptop-u");
    let _keep = Dir(dir.clone());
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let me = users::create_user(&brain.open_control().unwrap(), "test", "rob", "Rob", true)
        .unwrap()
        .id;
    let mine = triage::ensure(&brain, &me, "rob").unwrap();
    let music = brain.add_custom_space(&me, "rob", "Music", None).unwrap();
    note(&brain, &mine, "Call the plumber about the kitchen tap");
    let h = home();
    let (address, key) = join(&h, &h.robin, "laptop");
    // No `--person`: the sync finds the person of this node by itself.
    assert_eq!(
        users::node_person(&brain.open_control().unwrap()).unwrap(),
        Some(me.clone())
    );
    let to_home = Peer::new(&address, &key, "peer-home");
    note(&h.brain, &h.triage, "Book the car service for December");
    let p = pass(&to_home, &brain);
    assert_eq!(p.offered, 1, "{p:?}");
    pass(&to_home, &brain);
    let both = vec![
        "Book the car service for December".to_string(),
        "Call the plumber about the kitchen tap".to_string(),
    ];
    assert_eq!(active(&h.brain, &h.triage), both);
    assert_eq!(active(&brain, &mine), both);
    assert!(users::is_space_owner(&h.brain.open_control().unwrap(), &music, &h.robin).unwrap());
    // No second box beside the person's own.
    let boxes: Vec<Space> = brain
        .spaces()
        .unwrap()
        .into_iter()
        .filter(|s| triage::is_triage(&brain, s))
        .collect();
    assert_eq!(boxes, [mine]);
}

#[test]
fn only_a_peer_of_the_person_offers_a_space_and_another_persons_key_stays_in_their_memory() {
    let l = laptop();
    let h = home();
    // A device paired to capture (the box alone) may not offer spaces.
    let agent = "peer-phone-0001";
    let grant = format!("{}:rw", h.triage);
    let key = access::connect_peer(
        &h.brain.open_control().unwrap(),
        &h.robin,
        agent,
        &Rights::parse(&grant).unwrap(),
        false,
    )
    .unwrap();
    let phone = Peer::new(&format!("{}/mcp/{agent}", h.base), &key, "peer-home");
    l.brain
        .add_custom_space(OWNER, "owner", "Music", None)
        .unwrap();
    let p = phone.sync_once(&l.brain, util::now()).unwrap();
    assert_eq!(p.offered, 0, "{p:?}");
    assert!(
        p.problems
            .iter()
            .any(|x| x.contains("owner-music") && x.contains("not a peer of the person")),
        "{:?}",
        p.problems
    );
    assert!(
        !h.brain
            .has_space(&Space::new("owner-music").unwrap())
            .unwrap()
    );
    // The refusal is kept for the page: a sync in the background has nobody reading its output.
    let kept = memcp_core::sync::last_pass(&l.brain.open_control().unwrap(), "peer-home").unwrap();
    assert!(
        kept["problems"]
            .to_string()
            .contains("not offered (owner-music)"),
        "{kept}"
    );

    // A peer key of carol's: what the laptop holds goes to carol's memory there, never to robin's.
    let carol = users::resolve(&h.brain.open_control().unwrap(), "carol")
        .unwrap()
        .id;
    let (address, key) = join(&h, &carol, "laptop");
    let to_carol = Peer::new(&address, &key, "peer-home-carol");
    note(
        &l.brain,
        &l.triage,
        "Call the plumber about the kitchen tap",
    );
    note(&h.brain, &h.triage, "Book the car service for December");
    pass(&to_carol, &l.brain);
    pass(&to_carol, &l.brain);
    assert!(
        active(&l.brain, &l.triage)
            .iter()
            .all(|t| !t.contains("car service"))
    );
    assert!(
        active(&h.brain, &h.triage)
            .iter()
            .all(|t| !t.contains("plumber"))
    );
    let music = Space::new("owner-music").unwrap();
    let ctl = h.brain.open_control().unwrap();
    assert!(users::is_space_owner(&ctl, &music, &carol).unwrap());
    assert!(!users::is_space_owner(&ctl, &music, &h.robin).unwrap());
}

#[test]
fn a_space_of_the_peer_never_merges_into_a_space_of_another_person_of_the_same_name() {
    let l = laptop();
    let h = home();
    let (address, key) = join(&h, &h.robin, "laptop");
    let to_home = Peer::new(&address, &key, "peer-home");

    // At home, robin keeps a space; on the laptop, another human who is also called robin has one of the same name.
    let home_garden = h
        .brain
        .add_custom_space(&h.robin, "robin", "Garden", None)
        .unwrap();
    note(&h.brain, &home_garden, "Robin at home: the apple tree");
    let other = users::create_user(
        &l.brain.open_control().unwrap(),
        "test",
        "robin",
        "Robin",
        false,
    )
    .unwrap()
    .id;
    let laptop_garden = l
        .brain
        .add_custom_space(&other, "robin", "Garden", None)
        .unwrap();
    assert_eq!(home_garden, laptop_garden);
    note(&l.brain, &laptop_garden, "The other robin: a private line");

    let p = to_home.sync_once(&l.brain, util::now()).unwrap();
    // The clash is said, and nothing crosses either way.
    assert!(
        p.problems
            .iter()
            .any(|x| x.contains(home_garden.as_str()) && x.contains("another person")),
        "{:?}",
        p.problems
    );
    assert_eq!(
        active(&l.brain, &laptop_garden),
        ["The other robin: a private line"]
    );
    assert_eq!(
        active(&h.brain, &home_garden),
        ["Robin at home: the apple tree"]
    );
}
