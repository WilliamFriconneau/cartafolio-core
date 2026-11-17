#![cfg(all(feature = "sync", target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End to end with REAL PROCESSES: the programs `memcp`, `memcp-manager`, `memcp-sync` and `memcp-capture` are started
//! on ephemeral loopback ports, in temporary directories, and driven the way a person and their devices drive them
//! (the command line, the admin API with a console token, the MCP address, the join of a new node). Nothing of the node
//! is replaced by a mock; the tests read the stores of the nodes directly only to check what the processes did.
//!
//! Run them with `scripts/e2e.sh` (it builds every program first) or with `cargo test --workspace`.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use memcp_client::Client;
use memcp_core::{Brain, Space};
use serde_json::{Value, json};

// --- programs ----------------------------------------------------------------------------------------------------

/// The directory the programs of this build are in: next to `memcp-sync`, which Cargo builds for this test.
fn bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_memcp-sync"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The path of a program of the workspace. The ones of other packages are built once if they are not there yet (a
/// plain `cargo test -p memcp-client` does not build them; `cargo test --workspace` and `scripts/e2e.sh` do).
fn exe(name: &str) -> PathBuf {
    static BUILD: Once = Once::new();
    let path = bin_dir().join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if name == "memcp-sync" {
        return path;
    }
    BUILD.call_once(|| {
        let all = ["memcp", "memcp-manager", "memcp-capture"];
        if all.iter().all(|n| {
            bin_dir()
                .join(format!("{n}{}", std::env::consts::EXE_SUFFIX))
                .is_file()
        }) {
            return;
        }
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args([
                "build",
                "-p",
                "memcp-server",
                "-p",
                "memcp-manager",
                "-p",
                "memcp-capture",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "the programs could not be built");
    });
    assert!(path.is_file(), "{} is not built", path.display());
    path
}

// --- temporary directories and processes ---------------------------------------------------------------------------

/// A directory of one test, removed when the test ends (declare it first: it is dropped last, after the processes).
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Root {
        let dir = std::env::temp_dir().join(format!("memcp-real-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Root(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A child process that is killed (and reaped) when it goes out of scope, whatever happens to the test.
struct Proc(Option<Child>);

impl Proc {
    /// Started in a group of its own, so that what it starts in turn (a node starts `memcp-sync`) goes with it.
    fn spawn(mut cmd: Command) -> Proc {
        use std::os::unix::process::CommandExt;
        Proc(Some(
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap(),
        ))
    }
    /// The exit status once it is over, within `secs` seconds.
    fn wait(&mut self, secs: u64) -> Option<std::process::ExitStatus> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(s) = self.0.as_mut()?.try_wait().unwrap() {
                return Some(s);
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn kill(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", c.id())])
                .output();
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    fn alive(&mut self) -> bool {
        self.0
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.kill();
    }
}

fn until(secs: u64, what: &str, mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !ready() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn read_json(res: &mut ureq::http::Response<ureq::Body>) -> (u16, Value) {
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn token_in(text: &str, prefix: &str) -> String {
    text.split_whitespace()
        .find(|w| w.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} token in: {text}"))
        .to_string()
}

// --- a node --------------------------------------------------------------------------------------------------------

struct Node {
    name: String,
    data: PathBuf,
    port: u16,
    server: Proc,
    console: Option<String>,
}

impl Node {
    /// `memcp init` in a new directory (`--defaults`: the six usual spaces; else none).
    fn init(root: &Root, name: &str, defaults: bool) -> Node {
        let n = Node {
            name: name.into(),
            data: root.path(name),
            port: 0,
            server: Proc(None),
            console: None,
        };
        let mut args = vec!["init"];
        if defaults {
            args.push("--defaults");
        }
        n.cli(&args);
        n
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(exe("memcp"));
        c.arg("--data").arg(&self.data).args(args);
        c
    }

    /// Runs `memcp --data DIR ARGS`; it must succeed; its standard output.
    fn cli(&self, args: &[&str]) -> String {
        let out = self.cli_out(args);
        assert!(
            out.status.success(),
            "memcp {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn cli_out(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Serves the node on an ephemeral port (the same one again after a stop), and waits until it answers
    /// `/.well-known/memcp`.
    fn serve(&mut self) {
        for attempt in 0..10 {
            if self.port == 0 || attempt > 0 {
                // A port that was free a moment ago: another process may take it before this one binds it.
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                self.port = l.local_addr().unwrap().port();
            }
            let mut cmd = self.command(&[
                "serve",
                "--listen",
                &format!("127.0.0.1:{}", self.port),
                "--public-url",
                &self.base(),
                "--node-name",
                &self.name,
                "--threads",
                "8",
            ]);
            cmd.env("MEMCP_SYNC_BIN", exe("memcp-sync"))
                .env("MEMCP_BRIDGE_BIN", exe("memcp-capture"));
            self.server = Proc::spawn(cmd);
            let end = Instant::now() + Duration::from_secs(20);
            loop {
                if !self.server.alive() {
                    break; // could not start (port taken?): another port
                }
                if let Ok(mut r) = http()
                    .get(&format!("{}/.well-known/memcp", self.base()))
                    .call()
                    && r.status().as_u16() == 200
                {
                    let (_, v) = read_json(&mut r);
                    assert_eq!(v["name"], json!(self.name), "{v}");
                    return;
                }
                assert!(Instant::now() < end, "{} did not come up", self.name);
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        panic!("{} could not be served", self.name);
    }

    fn stop(&mut self) {
        self.server.kill();
    }

    /// A key of the console (cached): what the page of the person holds.
    fn console(&mut self) -> String {
        if self.console.is_none() {
            let out = self.cli(&["console-token", "owner"]);
            self.console = Some(token_in(&out, "hcpc_"));
        }
        self.console.clone().unwrap()
    }

    fn admin_get(&mut self, path: &str) -> (u16, Value) {
        let token = self.console();
        let mut r = http()
            .get(&format!("{}/admin/api{path}", self.base()))
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .unwrap();
        read_json(&mut r)
    }

    fn admin_post(&mut self, path: &str, body: &Value) -> (u16, Value) {
        let token = self.console();
        let mut r = http()
            .post(&format!("{}/admin/api{path}", self.base()))
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "memcp")
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .unwrap();
        read_json(&mut r)
    }

    /// An MCP connection made with the command line: its address and key.
    fn connect(&self, agent: &str, grant: &str) -> (String, String) {
        let mut args = vec!["connect", agent, "--public-url"];
        let base = self.base();
        args.push(&base);
        if grant == "all" {
            args.push("--all");
        } else {
            args.extend(["--grant", grant]);
        }
        let out = self.cli(&args);
        (format!("{base}/mcp/{agent}"), token_in(&out, "hcp_"))
    }

    fn client(&self, agent: &str, grant: &str) -> Client {
        let (url, key) = self.connect(agent, grant);
        let mut c = Client::new(&url, &key);
        c.initialize().unwrap();
        c
    }

    fn brain(&self) -> Brain {
        Brain::open(&self.data).unwrap()
    }

    /// The active notes of every space: space to sorted (id, text).
    fn notes(&self) -> BTreeMap<String, Vec<(String, String)>> {
        let b = self.brain();
        let mut out = BTreeMap::new();
        for s in b.spaces().unwrap() {
            let conn = b.open_space(&s).unwrap();
            let mut st = conn
                .prepare("SELECT id, text FROM facts WHERE status = 'active' ORDER BY id")
                .unwrap();
            let rows: Vec<(String, String)> = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .flatten()
                .collect();
            out.insert(s.as_str().to_string(), rows);
        }
        out
    }

    fn count(&self) -> usize {
        self.notes().values().map(Vec::len).sum()
    }

    fn count_in(&self, space: &str) -> usize {
        self.notes().get(space).map_or(0, Vec::len)
    }

    /// One number for the whole memory (spaces, ids and texts): equal on two nodes that hold the same notes.
    fn fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.notes().hash(&mut h);
        h.finish()
    }

    /// One pass of `memcp-sync` against `peer`; the exit code and what it said.
    fn sync(&self, peer: &Peering, extra: &[&str]) -> (i32, String) {
        let out = Command::new(exe("memcp-sync"))
            .arg("--data")
            .arg(&self.data)
            .args(["--peer", &peer.address, "--name", &peer.name, "--node-name"])
            .arg(&self.name)
            .args(extra)
            .env("MEMCP_PEER_TOKEN", &peer.key)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code().unwrap_or(-1), said)
    }

    fn sync_ok(&self, peer: &Peering) {
        let (code, said) = self.sync(peer, &[]);
        assert_eq!(code, 0, "{} syncing: {said}", self.name);
    }
}

/// What a node keeps about a peer it joined: the address of its connection there, the key and the name of the peer.
struct Peering {
    address: String,
    key: String,
    name: String,
}

/// The person on `host` allows the device asking to join (the page's click): returns the three words the page showed.
fn approve(host: &mut Node, device: &str) -> String {
    let mut found = None;
    until(20, "the join request on the host", || {
        let (_, v) = host.admin_get("/join/pending");
        found = v["requests"]
            .as_array()
            .and_then(|l| l.iter().find(|r| r["device"] == json!(device)).cloned());
        found.is_some()
    });
    let req = found.unwrap();
    let (status, v) = host.admin_post("/join/approve", &json!({ "id": req["id"] }));
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["result"], json!("approved"), "{v}");
    req["check"].as_str().unwrap().to_string()
}

/// `joiner` asks `host` to let it in with `memcp-sync --join` and the person approves; the peer is kept in
/// `joiner`'s `peers/`. `device` is what the joiner calls itself.
fn join_by_sync(root: &Root, host: &mut Node, joiner: &Node, device: &str) -> Peering {
    let status_file = root.path(&format!("join-{device}.json"));
    let mut cmd = Command::new(exe("memcp-sync"));
    cmd.arg("--data")
        .arg(&joiner.data)
        .args(["--join", &host.base(), "--status-file"])
        .arg(&status_file)
        .args(["--name", device]);
    let mut child = Proc::spawn(cmd);
    let check = approve(host, device);
    // The joiner showed the same three words on its side, before the answer.
    let status = child.wait(30).expect("memcp-sync --join did not finish");
    assert!(status.success(), "the join failed: {status}");
    let done: Value =
        serde_json::from_str(&std::fs::read_to_string(&status_file).unwrap()).unwrap();
    assert_eq!(done["state"], json!("approved"), "{done}");
    assert!(!check.is_empty());
    peering_of(&joiner.data)
}

fn peering_of(data: &Path) -> Peering {
    let entry = std::fs::read_dir(data.join("peers"))
        .unwrap()
        .flatten()
        .next()
        .expect("no peer was kept");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
    Peering {
        address: v["address"].as_str().unwrap().into(),
        key: v["key"].as_str().unwrap().into(),
        name: v["name"].as_str().unwrap().into(),
    }
}

// --- notes ---------------------------------------------------------------------------------------------------------

/// Made-up words: no two texts look alike to the near-duplicate guard.
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

fn remember(c: &mut Client, space: &str, text: &str) {
    let r = c
        .call("remember", json!({ "space": space, "text": text }))
        .unwrap();
    assert!(r.to_string().contains("created"), "{r}");
}

/// `n` notes written through MCP, over HTTP, spread over the spaces, from `seed`.
fn fill(c: &mut Client, spaces: &[&str], n: u64, seed: u64) {
    for i in 0..n {
        remember(c, spaces[(i as usize) % spaces.len()], &words(seed + i, 6));
    }
}

/// Many notes at once, written straight into the store of a node that is NOT running (a fast way to make a big memory).
fn bulk(data: &Path, space: &str, n: u64, seed: u64) {
    use memcp_core::facts::{self, NewFact, Remembered};
    let b = Brain::open(data).unwrap();
    let s = Space::new(space).unwrap();
    let conn = b.open_space(&s).unwrap();
    for i in 0..n {
        let f = NewFact::agent(&words(seed + i, 6), "agent:bulk");
        match facts::remember(&conn, &s, "owner", f, &[]).unwrap() {
            Remembered::Created(_) => {}
            other => panic!("not created: {other:?}"),
        }
    }
}

const SPACES: [&str; 3] = ["personal", "home", "work"];

// --- (a) a node, a person, an assistant, one address --------------------------------------------------------------

#[test]
fn a_node_answers_to_an_assistant_at_one_address_with_the_key() {
    let root = Root::new("a");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    // An assistant connection made on the command line, and used through the address that carries the key.
    let (url, key) = a.connect("claude", "all");
    let one_address = format!("{url}?key={key}");
    let mut by_key = Client::new(&one_address, "");
    let hello = by_key.initialize().unwrap();
    assert!(hello.is_object(), "{hello}");
    remember(&mut by_key, "personal", &words(1, 6));
    assert_eq!(a.count_in("personal"), 1);
    // The same address with no key, or a wrong one, is refused; the header works as well as the query.
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } });
    let post = |address: &str, bearer: Option<&str>| {
        let mut r = http()
            .post(address)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(b) = bearer {
            r = r.header("Authorization", &format!("Bearer {b}"));
        }
        r.send(init.to_string().as_bytes())
            .unwrap()
            .status()
            .as_u16()
    };
    assert_eq!(post(&one_address, None), 200);
    assert_eq!(post(&url, Some(&key)), 200);
    assert_eq!(post(&url, None), 401);
    assert_eq!(post(&format!("{url}?key=hcp_wrong"), None), 401);
    // The person's own page: a console token opens the admin API, an MCP key never does.
    let (status, state) = a.admin_get("/state");
    assert_eq!(status, 200, "{state}");
    let mut r = http()
        .get(&format!("{}/admin/api/state", a.base()))
        .header("Authorization", &format!("Bearer {key}"))
        .call()
        .unwrap();
    assert_ne!(read_json(&mut r).0, 200);
    // A connection made from the page carries the one-address form in its answer.
    let (status, made) = a.admin_post(
        "/connections",
        &json!({ "agent": "tablet", "rights": "personal:r" }),
    );
    assert_eq!(status, 200, "{made}");
    let with_key = made["mcp_url_with_key"].as_str().unwrap().to_string();
    assert!(with_key.starts_with(&format!("{}/mcp/tablet?key=hcp_", a.base())));
    let mut tablet = Client::new(&with_key, "");
    tablet.initialize().unwrap();
    let seen = tablet
        .call("recall", json!({ "space": "personal", "query": "a" }))
        .unwrap();
    assert!(seen.is_object() || seen.is_array(), "{seen}");
    // Read-only: a write is refused.
    assert!(
        tablet
            .call(
                "remember",
                json!({ "space": "personal", "text": words(2, 6) })
            )
            .is_err()
    );
    // The doctor of the installation finds nothing wrong, and the node ends cleanly.
    let doctor = a.cli(&["doctor"]);
    assert!(
        !doctor.contains("PROBLEM") && !doctor.contains("FAIL"),
        "{doctor}"
    );
}

#[test]
fn the_manager_makes_a_person_and_serves_them() {
    let root = Root::new("manager");
    let data = root.path("managed");
    let manager = |args: &[&str]| {
        Command::new(exe("memcp-manager"))
            .arg("--data")
            .arg(&data)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = manager(&["person", "add", "sam", "--admin"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let people = manager(&["people"]);
    assert!(String::from_utf8_lossy(&people.stdout).contains("sam"));
    // The person's directory is a node of its own: the engine's commands work on it.
    let mine = data.join("people").join("sam");
    let out = Command::new(exe("memcp"))
        .arg("--data")
        .arg(&mine)
        .args(["connect", "claude", "--all"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doctor = manager(&["doctor"]);
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stdout)
    );
    // And it serves them: the same readiness probe.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut cmd = Command::new(exe("memcp-manager"));
    cmd.arg("--data")
        .arg(&data)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .args(["--public-url", &format!("http://127.0.0.1:{port}")]);
    let mut server = Proc::spawn(cmd);
    until(20, "the manager to answer", || {
        assert!(server.alive(), "the manager stopped");
        http()
            .get(&format!("http://127.0.0.1:{port}/.well-known/memcp"))
            .call()
            .is_ok_and(|r| r.status().as_u16() == 200)
    });
}

// --- (b) a node joins another and receives the whole memory -------------------------------------------------------

#[test]
fn a_second_node_joins_through_the_page_and_receives_the_whole_memory() {
    let root = Root::new("b");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let mut assistant = a.client("claude", "all");
    fill(&mut assistant, &SPACES, 45, 100);
    let mut b = Node::init(&root, "beta", false);
    b.serve();
    // B's page asks A to let it in; A's person compares the three words and says yes.
    let (status, started) = b.admin_post("/join-memory", &json!({ "address": a.base() }));
    assert_eq!(status, 200, "{started}");
    let id = started["id"].as_str().unwrap().to_string();
    let mut shown = String::new();
    until(20, "the three words on B's page", || {
        let (_, v) = b.admin_get(&format!("/join-memory?id={id}"));
        if v["state"] == json!("waiting") {
            shown = v["check"].as_str().unwrap_or("").to_string();
            return true;
        }
        false
    });
    let on_a = approve(&mut a, "beta");
    assert_eq!(on_a, shown, "both screens show the same three words");
    let mut approved = false;
    until(30, "B's join to be approved", || {
        let (_, v) = b.admin_get(&format!("/join-memory?id={id}"));
        approved = v["state"] == json!("approved");
        approved
    });
    // B keeps the peer and syncs by itself (the node starts `memcp-sync` as it did for its page): the whole memory arrives.
    until(60, "B to hold the memory of A", || b.count() == 45);
    assert_eq!(b.notes(), a.notes());
    assert_eq!(b.fingerprint(), a.fingerprint());
    for s in SPACES {
        assert_eq!(b.count_in(s), 15, "{s}");
    }
    // The connection of B on A is one of a peer, listed with its use.
    let (_, list) = a.admin_get("/state");
    assert!(list.to_string().contains("peer-beta"), "{list}");
}

// --- (c) writes on both sides converge ---------------------------------------------------------------------------

#[test]
fn writes_on_both_sides_converge() {
    let root = Root::new("c");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let mut ca = a.client("claude", "all");
    fill(&mut ca, &SPACES, 12, 200);
    let mut b = Node::init(&root, "beta", false);
    // B serves before it joins: a node that starts with a peer kept would keep in step with it by itself.
    b.serve();
    let peer = join_by_sync(&root, &mut a, &b, "beta");
    b.sync_ok(&peer);
    assert_eq!(b.fingerprint(), a.fingerprint());
    // Both write, in the same spaces, at the same time.
    let mut cb = b.client("claude", "all");
    fill(&mut cb, &SPACES, 9, 300);
    fill(&mut ca, &SPACES, 9, 400);
    assert_ne!(a.fingerprint(), b.fingerprint());
    b.sync_ok(&peer);
    assert_eq!(a.count(), 12 + 9 + 9);
    assert_eq!(b.fingerprint(), a.fingerprint());
    // Nothing is left to say: a second pass moves nothing, in either direction.
    let (code, said) = b.sync(&peer, &[]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("0 received, 0 sent"), "{said}");
    // A note retired on one side is retired on the other.
    let victim = a.notes()["personal"][0].0.clone();
    let r = ca
        .call(
            "retire",
            json!({ "space": "personal", "id": victim, "reason": "test" }),
        )
        .unwrap();
    assert!(!r.to_string().contains("error"), "{r}");
    b.sync_ok(&peer);
    assert_eq!(b.fingerprint(), a.fingerprint());
    assert_eq!(a.count(), 29);
}

// --- (d) a third node joins B only and shows up in A's list ------------------------------------------------------

#[test]
fn a_third_node_joining_only_the_second_is_listed_by_the_first() {
    let root = Root::new("d");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let mut ca = a.client("claude", "all");
    fill(&mut ca, &SPACES, 9, 500);
    let mut b = Node::init(&root, "beta", false);
    b.serve();
    let a_peer = join_by_sync(&root, &mut a, &b, "beta");
    b.sync_ok(&a_peer);
    // C joins B with the bridge program, not A.
    let c = Node::init(&root, "gamma", false);
    let config = root.path("gamma.conf");
    let mut cmd = Command::new(exe("memcp-capture"));
    cmd.args(["join", &b.base(), "--name", "gamma", "--config"])
        .arg(&config);
    let mut joining = Proc::spawn(cmd);
    approve(&mut b, "gamma");
    assert!(
        joining
            .wait(30)
            .expect("the bridge did not finish")
            .success()
    );
    let conf = std::fs::read_to_string(&config).unwrap();
    let value = |k: &str| {
        conf.lines()
            .find_map(|l| l.strip_prefix(&format!("{k} = ")))
            .unwrap()
            .to_string()
    };
    let c_peer = Peering {
        address: value("url"),
        key: value("key"),
        name: "peer-beta".into(),
    };
    assert!(c_peer.address.starts_with(&b.base()));
    c.sync_ok(&c_peer);
    assert_eq!(
        c.fingerprint(),
        a.fingerprint(),
        "C got A's memory through B"
    );
    // A has never heard of C directly. After B's next pass with A, it lists it.
    let names = |a: &mut Node| -> Vec<String> {
        let (_, v) = a.admin_get("/sync/status");
        v["peers"]
            .as_array()
            .map(|l| {
                l.iter()
                    .map(|p| p["name"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(!names(&mut a).contains(&"gamma".to_string()));
    b.sync_ok(&a_peer);
    let listed = names(&mut a);
    assert!(listed.contains(&"gamma".to_string()), "{listed:?}");
    assert!(listed.contains(&"beta".to_string()), "{listed:?}");
}

// --- (e) a window replica prunes locally -------------------------------------------------------------------------

#[test]
fn a_window_replica_prunes_by_itself_and_leaves_the_origin_alone() {
    let root = Root::new("e");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let mut ca = a.client("claude", "all");
    fill(&mut ca, &["personal"], 20, 600);
    let w = Node::init(&root, "window", false);
    let peer = join_by_sync(&root, &mut a, &w, "window");
    // First the whole memory, then the node keeps the last day only.
    w.sync_ok(&peer);
    assert_eq!(w.count(), 20);
    let before_a = a.fingerprint();
    // Twelve notes are made old on the replica (its own copy), as if they were written a month ago.
    {
        let b = w.brain();
        let conn = b.open_space(&Space::new("personal").unwrap()).unwrap();
        let month_ago = memcp_core::util::now() - 30 * 86_400;
        conn.execute(
            "UPDATE facts SET created_at = ?1 WHERE id IN (SELECT id FROM facts ORDER BY id LIMIT 12)",
            [month_ago],
        )
        .unwrap();
    }
    let (code, said) = w.sync(&peer, &["--window-days", "2"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("12 pruned"), "{said}");
    assert_eq!(w.count(), 8, "the replica holds the last days only");
    assert_eq!(a.count(), 20, "the origin keeps everything");
    assert_eq!(a.fingerprint(), before_a, "and was not touched");
    // The pruned notes do not come back at the next pass, and a new note still travels.
    let (code, said) = w.sync(&peer, &[]);
    assert_eq!(code, 0, "{said}");
    assert_eq!(w.count(), 8, "{said}");
    remember(&mut ca, "personal", &words(6_001, 6));
    w.sync_ok(&peer);
    assert_eq!(w.count(), 9);
    assert_eq!(a.count(), 21);
    // Turning the window off does not bring the old notes back (a node that holds everything feeds them).
    let (code, _) = w.sync(&peer, &["--window-days", "off"]);
    assert_eq!(code, 0);
    assert_eq!(w.count(), 9);
}

// --- (f) revoking a node, and coming back without the three words -------------------------------------------------

#[test]
fn a_revoked_node_is_refused_and_a_node_allowed_in_advance_rejoins_without_words() {
    let root = Root::new("f");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let mut ca = a.client("claude", "all");
    fill(&mut ca, &SPACES, 6, 700);
    let b = Node::init(&root, "beta", false);
    let peer = join_by_sync(&root, &mut a, &b, "beta");
    b.sync_ok(&peer);
    assert_eq!(b.count(), 6);
    // The person revokes B's connection on A's page.
    let (status, v) = a.admin_post(
        "/connections/revoke",
        &json!({ "agent": peer_connection(&peer) }),
    );
    assert_eq!(status, 200, "{v}");
    let (code, said) = b.sync(&peer, &[]);
    assert_eq!(code, 1, "refused: {said}");
    assert!(said.contains("refused"), "{said}");
    // What B already holds stays on it; nothing new reaches it.
    remember(&mut ca, "home", &words(7_001, 6));
    let (code, _) = b.sync(&peer, &[]);
    assert_eq!(code, 1);
    assert_eq!(b.count(), 6);
    // A revoked peer is out of A's list.
    let (_, status_body) = a.admin_get("/sync/status");
    assert!(
        !status_body["peers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == json!("beta") && p["revoked"] != json!(true)),
        "{status_body}"
    );
    // A lets the device back in for a while, without comparing words: it comes in at once.
    let (status, v) = a.admin_post(
        "/join/allow-device",
        &json!({ "device": "beta", "minutes": 5 }),
    );
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["result"], json!("allowed"), "{v}");
    std::fs::remove_dir_all(b.data.join("peers")).unwrap();
    let status_file = root.path("rejoin.json");
    let mut cmd = Command::new(exe("memcp-sync"));
    cmd.arg("--data")
        .arg(&b.data)
        .args(["--join", &a.base(), "--status-file"])
        .arg(&status_file)
        .args(["--name", "beta"]);
    let mut rejoin = Proc::spawn(cmd);
    assert!(rejoin.wait(30).expect("the join did not finish").success());
    let again = peering_of(&b.data);
    assert_ne!(again.key, peer.key, "a new key");
    b.sync_ok(&again);
    assert_eq!(b.count(), 7);
    assert_eq!(b.fingerprint(), a.fingerprint());
    // The permission was for one time only: a second device of that name waits for a person.
    let (_, pending) = a.admin_get("/join/pending");
    assert!(pending["requests"].as_array().unwrap().is_empty());
}

/// The name of the connection a peering holds on its host: the last part of its address.
fn peer_connection(p: &Peering) -> String {
    p.address.rsplit('/').next().unwrap().to_string()
}

// --- (g) the lease of À trier between two sorters ----------------------------------------------------------------

#[test]
fn two_sorters_share_a_trie_box_by_lease_over_http() {
    let root = Root::new("g");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    // A device pairs with the code of the page and drops three notes with the bridge program, into À trier.
    let (status, pairing) = a.admin_post("/pairing", &json!({}));
    assert_eq!(status, 200, "{pairing}");
    let triage = pairing["triage"].as_str().unwrap().to_string();
    let mut r = http()
        .post(&format!("{}/pair", a.base()))
        .header("Content-Type", "application/json")
        .send(
            json!({ "code": pairing["code"], "device": "phone" })
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let (status, paired) = read_json(&mut r);
    assert_eq!(status, 200, "{paired}");
    let config = root.path("phone.conf");
    {
        let mut f = std::fs::File::create(&config).unwrap();
        writeln!(
            f,
            "url = {}\nkey = {}",
            paired["mcp_url"].as_str().unwrap(),
            paired["key"].as_str().unwrap()
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
    }
    for i in 0..3 {
        let out = Command::new(exe("memcp-capture"))
            .arg("--config")
            .arg(&config)
            .arg("--outbox")
            .arg(root.path("outbox"))
            .args(["text", &words(800 + i, 7)])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(a.count_in(&triage), 3);
    // Two sorters (two connections) claim notes: never the same one.
    let mut one = a.client("sorter-one", "all");
    let mut two = a.client("sorter-two", "all");
    let ids = |v: &Value| -> Vec<String> {
        v["claimed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    };
    let first = ids(&one.call("triage_claim", json!({ "limit": 2 })).unwrap());
    let second = ids(&two.call("triage_claim", json!({ "limit": 5 })).unwrap());
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 1, "only what the first does not hold");
    assert!(first.iter().all(|i| !second.contains(i)));
    // The first gives one back; the second takes exactly it.
    let back = one
        .call("triage_release", json!({ "ids": [first[0]] }))
        .unwrap();
    assert_eq!(back["released"], json!(1));
    let third = ids(&two.call("triage_claim", json!({ "limit": 5 })).unwrap());
    assert!(
        third.contains(&first[0]) && third.contains(&second[0]),
        "{third:?}"
    );
    assert!(!third.contains(&first[1]));
    // A sorter sends a note on (retires it); it is not offered again to anyone.
    let r = two
        .call(
            "retire",
            json!({ "space": triage, "id": second[0], "reason": "sorted" }),
        )
        .unwrap();
    assert!(!r.to_string().contains("error"), "{r}");
    let rest = ids(&one.call("triage_claim", json!({ "limit": 10 })).unwrap());
    assert!(!rest.contains(&second[0]));
    assert!(rest.contains(&first[1]));
    // A key that cannot write the box is not offered the tool at all.
    let mut reader = a.client("reader", &format!("{triage}:r"));
    assert!(reader.call("triage_claim", json!({})).is_err());
}

// --- (h) busy: 503 with Retry-After, and a client that retries -----------------------------------------------------

#[test]
fn a_busy_node_says_when_to_come_back_and_a_client_retries() {
    let root = Root::new("h");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let (url, key) = a.connect("claude", "all");
    let mut c = Client::new(&url, &key);
    c.initialize().unwrap();
    // Sixteen requests from this address are in flight, and stay so: each promises a body that does not come.
    let mut holders = Vec::new();
    for _ in 0..16 {
        let mut s = TcpStream::connect(("127.0.0.1", a.port)).unwrap();
        write!(
            s,
            "POST /mcp/claude HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n{{"
        )
        .unwrap();
        holders.push(s);
    }
    // The places are taken when the request is read: wait until the node answers busy.
    let mut busy = None;
    until(20, "the node to say it is busy", || {
        let r = http()
            .get(&format!("{}/.well-known/memcp", a.base()))
            .call()
            .unwrap();
        if r.status().as_u16() == 503 {
            busy = r
                .headers()
                .get("retry-after")
                .map(|v| v.to_str().unwrap().to_string());
            true
        } else {
            false
        }
    });
    assert_eq!(busy.as_deref(), Some("1"), "a 503 carries Retry-After");
    // Told once, without a retry, the client reports the busy node as such.
    c.retry.tries = 1;
    assert!(matches!(
        c.call("spaces", json!({})),
        Err(memcp_client::Error::Unreachable(m)) if m.contains("503")
    ));
    // A client that calls now waits and goes through once the places are free again.
    c.retry.first = Duration::from_millis(100);
    c.retry.tries = 20;
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        drop(holders);
    });
    let started = Instant::now();
    let seen = c.call("spaces", json!({})).unwrap();
    assert!(seen.to_string().contains("personal"), "{seen}");
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "it did wait"
    );
    release.join().unwrap();
    // And the node is well afterwards.
    assert_eq!(
        http()
            .get(&format!("{}/.well-known/memcp", a.base()))
            .call()
            .unwrap()
            .status()
            .as_u16(),
        200
    );
}

// --- (i) backup, export, adopt, merge ----------------------------------------------------------------------------

fn only_entry(dir: &Path) -> PathBuf {
    let mut it = std::fs::read_dir(dir).unwrap().flatten();
    let first = it.next().expect("empty directory").path();
    assert!(
        it.next().is_none(),
        "more than one entry in {}",
        dir.display()
    );
    first
}

#[test]
fn backup_export_adopt_and_merge_make_the_same_memory_again() {
    let root = Root::new("i");
    let mut a = Node::init(&root, "alpha", true);
    a.serve();
    let mut c = a.client("claude", "all");
    fill(&mut c, &SPACES, 24, 900);
    // A backup taken while the node serves is a whole data directory that opens as one.
    let backups = root.path("backups");
    a.cli(&["backup", "--to", backups.to_str().unwrap()]);
    let copy = only_entry(&backups);
    let restored = Node {
        name: "restored".into(),
        data: copy,
        port: 0,
        server: Proc(None),
        console: None,
    };
    assert_eq!(restored.fingerprint(), a.fingerprint());
    let doctor = restored.cli(&["doctor"]);
    assert!(!doctor.contains("FAIL"), "{doctor}");
    // Export, check it, and take it into a node that has none of it.
    let exports = root.path("exports");
    a.cli(&["export", "--to", exports.to_str().unwrap()]);
    let export = only_entry(&exports);
    let out = a.cli_out(&["verify-export", export.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let d = Node::init(&root, "delta", false);
    d.cli(&["adopt", export.to_str().unwrap(), "--user", "owner"]);
    assert_eq!(d.notes(), a.notes());
    assert_eq!(d.fingerprint(), a.fingerprint());
    // A space that exists is never overwritten: adopting twice is refused and changes nothing.
    let again = d.cli_out(&["adopt", export.to_str().unwrap(), "--user", "owner"]);
    assert!(!again.status.success());
    assert_eq!(d.fingerprint(), a.fingerprint());
    // An export that was altered is refused before anything changes.
    let broken = root.path("exports-broken");
    a.cli(&["export", "--to", broken.to_str().unwrap()]);
    let altered = only_entry(&broken);
    let vault = std::fs::read_dir(altered.join("spaces"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&vault).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&vault, bytes).unwrap();
    assert!(
        !a.cli_out(&["verify-export", altered.to_str().unwrap()])
            .status
            .success()
    );
    // Merge the export into a node with notes of its own: the union, once.
    let e = Node::init(&root, "epsilon", true);
    bulk(&e.data, "personal", 5, 7_000);
    bulk(&e.data, "home", 4, 8_000);
    let plan = root.path("plan.json");
    e.cli(&[
        "merge",
        "--plan",
        export.to_str().unwrap(),
        "--user",
        "owner",
        "--out",
        plan.to_str().unwrap(),
    ]);
    assert!(plan.is_file());
    let backup_dir = root.path("merge-backup");
    e.cli(&[
        "merge",
        "--apply",
        plan.to_str().unwrap(),
        "--backup-to",
        backup_dir.to_str().unwrap(),
    ]);
    assert_eq!(e.count(), 24 + 9);
    assert!(backup_dir.is_dir());
    let after = e.fingerprint();
    e.cli(&[
        "merge",
        "--apply",
        plan.to_str().unwrap(),
        "--backup-to",
        backup_dir.to_str().unwrap(),
    ]);
    assert_eq!(e.fingerprint(), after, "merging twice changes nothing");
    // The notes of A are in E, and the nodes serve what they were given.
    let mine = a.notes();
    let theirs = e.notes();
    for (space, rows) in &mine {
        for (_, text) in rows {
            assert!(
                theirs[space].iter().any(|(_, t)| t == text),
                "{space}: {text}"
            );
        }
    }
}

// --- (j) a node killed in the middle of a sync resumes ------------------------------------------------------------

#[test]
fn a_node_killed_in_the_middle_of_a_sync_resumes_where_it_stopped() {
    let root = Root::new("j");
    let a = Node::init(&root, "alpha", true);
    bulk(&a.data, "personal", 6000, 10_000);
    let mut a = a;
    a.serve();
    let b = Node::init(&root, "beta", false);
    let peer = join_by_sync(&root, &mut a, &b, "beta");
    // The pass is started and killed (SIGKILL: no clean-up) as soon as part of the memory is there.
    let mut cmd = Command::new(exe("memcp-sync"));
    cmd.arg("--data")
        .arg(&b.data)
        .args(["--peer", &peer.address, "--name", &peer.name])
        .env("MEMCP_PEER_TOKEN", &peer.key);
    let mut pass = Proc::spawn(cmd);
    let mut cut = 0;
    until(60, "part of the memory to arrive", || {
        cut = if b
            .brain()
            .has_space(&Space::new("personal").unwrap())
            .unwrap()
        {
            b.count_in("personal")
        } else {
            0
        };
        cut > 0
    });
    pass.kill();
    let held = b.count_in("personal");
    assert!(held > 0, "something arrived");
    assert!(held < 6000, "it was cut in the middle ({held} of 6000)");
    // The store of B is whole after the kill (no half-written page), and the next pass finishes the work.
    assert!(b.cli_out(&["doctor"]).status.success());
    let (code, said) = b.sync(&peer, &[]);
    assert_eq!(code, 0, "{said}");
    assert_eq!(b.count_in("personal"), 6000);
    assert_eq!(b.fingerprint(), a.fingerprint());
    // It resumed rather than started over: it received at most what was missing.
    let received: usize = said.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(
        received <= 6000 - held + 500,
        "received {received} after holding {held}"
    );
    // The node that serves can be killed too, in the middle of a pass; the pass says so and the next one completes.
    let mut writer = a.client("claude", "all");
    fill(&mut writer, &SPACES, 30, 20_000);
    a.stop();
    let (code, _) = b.sync(&peer, &[]);
    assert_ne!(code, 0, "an absent node is not a success");
    a.serve();
    b.sync_ok(&peer);
    assert_eq!(b.fingerprint(), a.fingerprint());
}
