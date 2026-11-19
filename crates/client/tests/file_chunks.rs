#![cfg(feature = "sync")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Big files between real nodes over HTTP, in chunks: a file of 30 MiB goes both ways and its digest matches, a chunk
//! that does not match is refused and a cut transfer resumes without fetching again what came, a peer of an earlier
//! version gets the small files whole and none of the big ones (said, never held by it) until it takes chunks, the
//! policy `off` stops chunks too, and a big recording erased at its origin is erased everywhere.

use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

use memcp_client::peer_sync::{Pass, Peer};
use memcp_core::access::{self, Rights};
use memcp_core::chunks::{self, CHUNK_BYTES};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::files::{self, FileInfo};
use memcp_core::policy::{self, Policy};
use memcp_core::text::Language;
use memcp_core::users::{self, OWNER};
use memcp_core::{Brain, Space, util};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve_on};
use serde_json::Value;

fn temp(name: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-chunks-{}-{n}-{name}", std::process::id()));
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

fn key_for_a(n: &Node) -> String {
    access::connect_peer(
        &n.brain.open_control().unwrap(),
        OWNER,
        A_ON_B,
        &Rights::parse("all").unwrap(),
        false,
    )
    .unwrap()
}

/// Serves `n`, as a node that takes chunks or as one of an earlier version: the address of A's connection there.
fn serve(n: &Node, chunks: bool) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut app = App::new(Brain::open(&n.dir).unwrap());
    app.file_chunks = chunks;
    let app = Arc::new(app);
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
    format!("http://127.0.0.1:{port}/mcp/{A_ON_B}")
}

/// Node A, which runs the passes, and node B, which serves them.
fn pair(spaces: &[&str], b_takes_chunks: bool) -> (Node, Node, Peer, String, String) {
    let a = node("a", spaces);
    let b = node("b", spaces);
    let key = key_for_a(&b);
    let addr = serve(&b, b_takes_chunks);
    let peer = Peer::new(&addr, &key, B_FROM_A);
    (a, b, peer, addr, key)
}

fn pass(peer: &Peer, n: &Node) -> Pass {
    let p = peer.sync_once(&n.brain, util::now()).unwrap();
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    p
}

/// A recording of `len` bytes (a WAV header, then bytes that differ from chunk to chunk), made in memory.
fn wav(len: usize, seed: u32) -> Vec<u8> {
    let mut v = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    let mut x = seed;
    v.resize_with(len, || {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (x >> 24) as u8
    });
    v
}

/// Keeps `bytes` in `space` of `n` the way a watched folder does: from a file on disk, removed after.
fn put(n: &Node, space: &str, name: &str, bytes: &[u8], tags: &[&str]) -> FileInfo {
    let path = n.dir.join(format!("incoming-{name}"));
    std::fs::write(&path, bytes).unwrap();
    let tags: Vec<String> = tags.iter().map(|t| (*t).to_string()).collect();
    let f = files::put_path(&n.brain, &sp(space), "device", name, "", &path, &tags).unwrap();
    std::fs::remove_file(&path).unwrap();
    f
}

/// The digest of the bytes `n` holds for the file `id`, or `None` when it does not hold them.
fn held(n: &Node, space: &str, id: &str) -> Option<String> {
    files::path_of(&n.brain, &sp(space), id)
        .unwrap()
        .map(|(_, p)| chunks::digest(&std::fs::read(p).unwrap()))
}

fn raw(addr: &str, key: &str, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{addr}/sync/{path}");
    let auth = format!("Bearer {key}");
    let mut res = if method == "GET" {
        agent.get(&url).header("Authorization", &auth).call()
    } else {
        agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/octet-stream")
            .send(body)
    }
    .unwrap();
    let status = res.status().as_u16();
    let bytes = res
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_vec()
        .unwrap();
    (status, bytes)
}

fn json(addr: &str, key: &str, path: &str) -> Value {
    let (status, body) = raw(addr, key, "GET", path, &[]);
    assert_eq!(status, 200, "{path}");
    serde_json::from_slice(&body).unwrap()
}

/// The file events of `space` a peer gets from the node at `addr`.
fn file_events(addr: &str, key: &str, query: &str) -> Vec<Value> {
    json(addr, key, &format!("events?{query}"))["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "file_put")
        .cloned()
        .collect()
}

const BIG: usize = 30 * 1024 * 1024;

#[test]
fn a_file_of_30_mib_goes_both_ways_in_chunks_and_its_digest_matches() {
    let (a, b, peer, addr, key) = pair(&["work"], true);
    let small = put(&b, "work", "short.wav", &wav(2000, 1), &[]);
    let theirs = put(&b, "work", "meeting.wav", &wav(BIG, 2), &[]);
    let mine = put(&a, "work", "lecture.wav", &wav(BIG, 3), &[]);
    // B says it takes chunks; a peer that does not ask for them gets the small file whole and not the big one.
    assert_eq!(
        json(&addr, &key, "spaces")["chunks"].as_u64(),
        Some(CHUNK_BYTES as u64)
    );
    let old = file_events(&addr, &key, "space=work&after=0");
    assert_eq!(old.len(), 1, "{old:?}");
    assert_eq!(old[0]["payload"]["id"], small.id.as_str());
    assert!(old[0]["payload"]["data"].is_string());
    // Asked again by the same machine, now with chunks: B goes back to what it left out.
    let listed = json(&addr, &key, "spaces");
    let head = listed["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["space"] == "work")
        .and_then(|s| s["head"].as_i64())
        .unwrap();
    // First an empty page that sends its cursor back before what was left out, then the file.
    let back = json(
        &addr,
        &key,
        &format!("events?space=work&after={head}&chunks=1"),
    );
    assert_eq!(back["events"].as_array().unwrap().len(), 0);
    let before = back["next"].as_i64().unwrap();
    assert!(before < head);
    let new = file_events(&addr, &key, &format!("space=work&after={before}&chunks=1"));
    let new: Vec<Value> = new
        .into_iter()
        .filter(|e| e["payload"]["id"] != small.id.as_str())
        .collect();
    assert_eq!(new.len(), 1, "{new:?}");
    assert_eq!(new[0]["payload"]["id"], theirs.id.as_str());
    assert_eq!(
        new[0]["payload"]["chunks"].as_array().unwrap().len(),
        BIG.div_ceil(CHUNK_BYTES)
    );
    assert!(new[0]["payload"].get("data").is_none());

    let p = pass(&peer, &a);
    assert_eq!(p.chunks_in, BIG.div_ceil(CHUNK_BYTES));
    assert_eq!(p.chunks_out, BIG.div_ceil(CHUNK_BYTES));
    assert_eq!(held(&a, "work", &theirs.id), Some(theirs.sha256.clone()));
    assert_eq!(held(&b, "work", &mine.id), Some(mine.sha256.clone()));
    assert_eq!(held(&a, "work", &small.id), Some(small.sha256.clone()));
    assert_eq!(chunks::staged(&a.brain, &sp("work"), &theirs.sha256), 0);
    assert_eq!(chunks::staged(&b.brain, &sp("work"), &mine.sha256), 0);
    // Nothing more to move.
    let again = pass(&peer, &a);
    assert_eq!((again.chunks_in, again.chunks_out), (0, 0));
}

#[test]
fn a_corrupt_chunk_is_refused_and_the_next_pass_fetches_only_what_is_missing() {
    let (a, b, peer, _, _) = pair(&["work"], true);
    let bytes = wav(BIG, 4);
    let f = put(&b, "work", "meeting.wav", &bytes, &[]);
    // Chunk 5 of B's copy is damaged on disk: A refuses it, keeps the five before, and holds nothing yet.
    let blob = files::blob_path(&b.brain, &sp("work"), &f.sha256);
    let at = 5 * CHUNK_BYTES;
    let write_at = |data: &[u8]| {
        let mut o = std::fs::OpenOptions::new().write(true).open(&blob).unwrap();
        o.seek(SeekFrom::Start(at as u64)).unwrap();
        o.write_all(data).unwrap();
    };
    write_at(&[0u8; 64]);
    let cut = peer.sync_once(&a.brain, util::now()).unwrap();
    assert!(
        cut.problems
            .iter()
            .any(|p| p.contains("chunk 5") && p.contains("refused")),
        "{:?}",
        cut.problems
    );
    assert_eq!(cut.chunks_in, 5);
    assert_eq!(held(&a, "work", &f.id), None);
    assert!(files::info(&a.brain, &sp("work"), &f.id).unwrap().is_none());
    assert_eq!(chunks::staged(&a.brain, &sp("work"), &f.sha256), 5);
    // Mended: the next pass fetches the three it lacks, not the five it has.
    write_at(&bytes[at..at + 64]);
    let p = pass(&peer, &a);
    assert_eq!(p.chunks_in, BIG.div_ceil(CHUNK_BYTES) - 5);
    assert_eq!(held(&a, "work", &f.id), Some(f.sha256.clone()));
    assert_eq!(chunks::staged(&a.brain, &sp("work"), &f.sha256), 0);
}

#[test]
fn a_peer_of_an_earlier_version_gets_small_files_whole_and_the_big_ones_once_it_takes_chunks() {
    let (a, b, peer, _, key) = pair(&["work"], false);
    let small = put(&a, "work", "short.wav", &wav(2000, 5), &[]);
    let big = put(&a, "work", "lecture.wav", &wav(BIG, 6), &[]);
    let theirs = put(&b, "work", "note.wav", &wav(3000, 7), &[]);
    let p = peer.sync_once(&a.brain, util::now()).unwrap();
    assert_eq!(p.problems.len(), 1, "{:?}", p.problems);
    assert!(
        p.problems[0].contains("1 file(s) over 24 MB not sent"),
        "{:?}",
        p.problems
    );
    assert_eq!(p.chunks_out, 0);
    assert_eq!(held(&b, "work", &small.id), Some(small.sha256.clone()));
    assert!(
        files::info(&b.brain, &sp("work"), &big.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(held(&a, "work", &theirs.id), Some(theirs.sha256.clone()));
    // Never counted as held by B: the window here would keep it.
    let unheld = |eid: &str| -> bool {
        a.brain
            .open_space(&sp("work"))
            .unwrap()
            .query_row(
                "SELECT count(*) > 0 FROM sync_unheld WHERE peer = ?1 AND eid = ?2",
                [B_FROM_A, eid],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!(unheld(&format!("f:{}", big.id)));
    // The next pass says nothing more about it.
    pass(&peer, &a);
    // B is updated: it takes chunks, and gets what was left out.
    let addr = serve(&b, true);
    let peer = Peer::new(&addr, &key, B_FROM_A);
    let p = pass(&peer, &a);
    assert_eq!(p.chunks_out, BIG.div_ceil(CHUNK_BYTES));
    assert_eq!(held(&b, "work", &big.id), Some(big.sha256.clone()));
    assert!(!unheld(&format!("f:{}", big.id)));
}

fn owner(n: &Node) -> users::User {
    users::user(&n.brain.open_control().unwrap(), OWNER)
        .unwrap()
        .unwrap()
}

#[test]
fn the_policy_off_stops_chunks_too() {
    let (a, b, peer, addr, key) = pair(&["work", "health"], true);
    pass(&peer, &a);
    let f = put(&b, "health", "scan.wav", &wav(BIG, 8), &[]);
    // B can give it: chunk 0 is served.
    let (status, _) = raw(
        &addr,
        &key,
        "GET",
        &format!("chunk?space=health&file={}&n=0", f.sha256),
        &[],
    );
    assert_eq!(status, 200);
    policy::set(&b.brain, &owner(&b), A_ON_B, &sp("health"), Policy::Off).unwrap();
    let (status, _) = raw(
        &addr,
        &key,
        "GET",
        &format!("chunk?space=health&file={}&n=0", f.sha256),
        &[],
    );
    assert_eq!(status, 403);
    let bytes = vec![1u8; 100];
    let (status, _) = raw(
        &addr,
        &key,
        "POST",
        &format!(
            "chunk?space=health&file={}&hash={}",
            f.sha256,
            chunks::digest(&bytes)
        ),
        &bytes,
    );
    assert_eq!(status, 403);
    let p = pass(&peer, &a);
    assert_eq!(p.chunks_in, 0);
    assert!(
        files::info(&a.brain, &sp("health"), &f.id)
            .unwrap()
            .is_none()
    );
    // Within the space B gives, a chunk of a file that is not that space's is never served.
    let (status, _) = raw(
        &addr,
        &key,
        "GET",
        &format!("chunk?space=work&file={}&n=0", f.sha256),
        &[],
    );
    assert_eq!(status, 404);
}

#[test]
fn a_big_recording_erased_at_its_origin_is_erased_on_the_peer() {
    let (a, b, peer, _, _) = pair(&["work"], true);
    let rec = put(&a, "work", "memo.wav", &wav(BIG, 9), &["inbox/audio"]);
    pass(&peer, &a);
    assert_eq!(held(&b, "work", &rec.id), Some(rec.sha256.clone()));
    let s = sp("work");
    let conn = a.brain.open_space(&s).unwrap();
    let Remembered::Created(text) = facts::remember(
        &conn,
        &s,
        "me",
        NewFact::agent(&format!("Memo {}: call the plumber.", rec.name), "agent:t"),
        &[],
    )
    .unwrap() else {
        panic!("no text");
    };
    files::erase(&a.brain, &s, "dictation", &rec.id, &s, &text.id).unwrap();
    pass(&peer, &a);
    assert!(files::erased(&b.brain, &s, &rec.id).unwrap());
    assert_eq!(held(&b, "work", &rec.id), None);
    assert!(!files::blob_path(&b.brain, &s, &rec.sha256).exists());
}

/// What a node left out for a peer that took no chunks is sent again once the peer takes them, and a lost answer loses
/// nothing: the mark stays until the peer asks from before it.
#[test]
fn what_was_left_out_for_a_peer_is_served_again_even_when_an_answer_is_lost() {
    let b = node("b", &["work"]);
    let key = key_for_a(&b);
    let addr = serve(&b, true);
    let big = put(&b, "work", "lecture.wav", &wav(BIG, 9), &[]);
    let small = put(&b, "work", "short.wav", &wav(2000, 10), &[]);
    // The peer, of an earlier version, takes no chunks: the big file is left out, its cursor goes past it.
    let page = json(&addr, &key, "events?space=work&after=0");
    let ids: Vec<&str> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "file_put")
        .map(|e| e["payload"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [small.id.as_str()]);
    let past = page["next"].as_i64().unwrap();
    // Updated, it asks from its cursor: sent back before the mark. The answer is lost: it asks again, same answer.
    for _ in 0..2 {
        let back = json(
            &addr,
            &key,
            &format!("events?space=work&after={past}&chunks=1"),
        );
        assert_eq!(back["events"].as_array().unwrap().len(), 0);
        assert!(back["next"].as_i64().unwrap() < past);
        assert_eq!(back["more"], true);
    }
    let back = json(
        &addr,
        &key,
        &format!("events?space=work&after={past}&chunks=1"),
    );
    let before = back["next"].as_i64().unwrap();
    // From before the mark: the big file comes, in chunks; asked twice (a lost answer), it comes twice.
    for _ in 0..2 {
        let got = file_events(&addr, &key, &format!("space=work&after={before}&chunks=1"));
        assert!(got.iter().any(|e| e["payload"]["id"] == big.id.as_str()));
        assert!(got.iter().all(|e| e["payload"]["chunks"].is_array() || e["payload"]["id"] != big.id.as_str()));
    }
    // Past it now: served as usual, no more sending back.
    let page = json(
        &addr,
        &key,
        &format!("events?space=work&after={past}&chunks=1"),
    );
    assert_eq!(page["next"].as_i64().unwrap(), past);
}
