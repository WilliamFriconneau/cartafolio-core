//! Findings of the core audit (docs/reviews/2026-11-17-core-audit.md), one test each, written before the fix.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use pensine_core::access::{self, Rights};
use pensine_core::text::Language;
use pensine_core::{Brain, Space};
use pensine_server::app::App;
use pensine_server::http::{Request, Response};
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);

fn sp(n: &str) -> Space {
    match Space::new(n) {
        Ok(s) => s,
        Err(e) => panic!("{n}: {e}"),
    }
}

struct Env {
    dir: PathBuf,
    app: App,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Result<Env, Box<dyn Error>> {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-audit-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[sp("health"), sp("work")], Language::English)?;
    Ok(Env {
        dir,
        app: App::new(brain),
    })
}

fn connect(e: &Env, agent: &str, rights: &str) -> Result<String, Box<dyn Error>> {
    let ctl = e.app.brain.open_control()?;
    Ok(access::connect(
        &ctl,
        agent,
        &Rights::parse(rights)?,
        None,
        false,
        false,
    )?)
}

/// A connection for a machine of the person, as a pairing or a join makes it.
fn machine(e: &Env, agent: &str, rights: &str) -> Result<String, Box<dyn Error>> {
    let ctl = e.app.brain.open_control()?;
    Ok(access::connect_peer(
        &ctl,
        pensine_core::users::OWNER,
        agent,
        &Rights::parse(rights)?,
        false,
    )?)
}

fn rq(method: &str, path: &str, token: &str, query: &[(&str, &str)], body: &str) -> Request {
    let mut headers = std::collections::HashMap::new();
    if !token.is_empty() {
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
    }
    Request {
        method: method.into(),
        path: path.into(),
        query: query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        headers,
        body: body.as_bytes().to_vec(),
        ip: Some("10.0.0.7".into()),
        via_ingress: false,
        tailnet_login: None,
    }
}

fn event(id: &str, tags: &[&str]) -> Value {
    json!({
        "eid": format!("c:{id}"), "hlc": format!("{:013}-0000-peer", pensine_core::util::now_ms()),
        "origin": "peer", "kind": "fact_created", "at": pensine_core::util::now(),
        "payload": { "id": id, "kind": "context", "text": format!("text {id}"), "source_kind": "agent",
                     "author": "peer", "created_at": pensine_core::util::now(), "tags": tags },
    })
}

fn sync(
    e: &Env,
    method: &str,
    agent: &str,
    op: &str,
    token: &str,
    space: &str,
    body: &str,
) -> Response {
    let q: Vec<(&str, &str)> = if space.is_empty() {
        vec![]
    } else {
        vec![("space", space)]
    };
    e.app.handle(&rq(
        method,
        &format!("/mcp/{agent}/sync/{op}"),
        token,
        &q,
        body,
    ))
}

#[test]
fn a_connection_limited_to_paths_cannot_use_the_sync_routes() -> Res {
    let e = env()?;
    let whole = machine(&e, "peer-whole", "work:rw")?;
    let narrow = machine(&e, "peer-narrow", "work/sources/app:rw")?;
    // The machine that reaches the space whole syncs it: it reads the log and writes through it.
    let body = json!({ "events": [event("f1", &["secret/index"])] }).to_string();
    let r = sync(&e, "POST", "peer-whole", "events", &whole, "work", &body);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        sync(&e, "GET", "peer-whole", "events", &whole, "work", "").status,
        200
    );
    // A connection limited to a path reaches neither: the log is the whole vault.
    assert_eq!(
        sync(&e, "GET", "peer-narrow", "events", &narrow, "work", "").status,
        403
    );
    let body = json!({ "events": [event("f2", &["index"])] }).to_string();
    assert_eq!(
        sync(&e, "POST", "peer-narrow", "events", &narrow, "work", &body).status,
        403
    );
    let listed = sync(&e, "GET", "peer-narrow", "spaces", &narrow, "", "").body_json();
    assert_eq!(listed["spaces"], json!([]), "{listed}");
    Ok(())
}

#[test]
fn a_request_may_not_make_the_server_buffer_a_gigabyte() {
    // The biggest body of any request is what an MCP call may carry (a 24 MB recording in base64), not more.
    const { assert!(pensine_server::net::LARGE_MAX <= 40 * 1024 * 1024) };
}

fn raw(port: u16, req: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("connect: {e}"));
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    s.write_all(req.as_bytes())
        .unwrap_or_else(|e| panic!("write: {e}"));
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn guessing_a_key_by_the_size_of_a_body_is_counted_like_any_wrong_key() -> Res {
    use std::io::{BufRead, BufReader};
    let e = env()?;
    let dir = e.dir.clone();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_memcp"))
        .arg("--data")
        .arg(&dir)
        .args(["serve", "--listen", "127.0.0.1:0"])
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut reader = BufReader::new(child.stderr.take().ok_or("no stderr")?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    std::thread::spawn(move || for _ in reader.lines() {});
    let port: u16 = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or("port")?;
    // A wrong key with a big declared body used to be answered 413 for ever, with no count: an oracle for the key.
    let big = "POST /mcp/x HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer wrong\r\nContent-Length: 5000000\r\nConnection: close\r\n\r\n";
    for _ in 0..10 {
        assert!(raw(port, big).starts_with("HTTP/1.1 413"));
    }
    let after = raw(
        port,
        "POST /mcp/x HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    let _ = child.kill();
    let _ = child.wait();
    assert!(after.starts_with("HTTP/1.1 429"), "{after}");
    Ok(())
}

#[test]
fn a_header_value_cannot_carry_a_line_break() {
    use pensine_server::net::header_is_safe;
    assert!(header_is_safe("Content-Type", "image/png"));
    assert!(header_is_safe(
        "Content-Disposition",
        "inline; filename=\"a b.png\""
    ));
    assert!(!header_is_safe(
        "Content-Type",
        "text/plain\r\nSet-Cookie: x=1"
    ));
    assert!(!header_is_safe("Content-Disposition", "inline\nX: y"));
    assert!(!header_is_safe("X\r\nY", "z"));
}

#[test]
fn the_check_phrase_uses_every_word_of_its_list() {
    // It used to draw from about 26 of the 64 words (a base32 letter modulo 64): 14 bits instead of 18.
    let mut words = std::collections::HashSet::new();
    for _ in 0..3000 {
        for w in pensine_server::join::phrase().split('-') {
            words.insert(w.to_string());
        }
    }
    assert_eq!(words.len(), 64);
}

#[test]
fn the_failure_guard_holds_an_ipv6_network_and_cannot_grow_without_bound() -> Res {
    let e = env()?;
    let now = pensine_core::util::now();
    for i in 0..10 {
        e.app.fail(now, &format!("2001:db8:1:2:{i:x}::1"));
    }
    assert!(e.app.blocked(now, "2001:db8:1:2:ffff::9"), "same /64");
    assert!(!e.app.blocked(now, "2001:db8:1:3::1"), "another /64");
    for i in 0..20_000 {
        e.app.fail(
            now,
            &format!("10.{}.{}.{}", i / 65536, (i / 256) % 256, i % 256),
        );
    }
    assert!(e.app.failures_held() <= 5000);
    Ok(())
}

#[test]
fn guessing_oauth_codes_is_counted_and_held_off() -> Res {
    let e = env()?;
    let mut last = 0;
    for _ in 0..12 {
        let mut r = rq(
            "POST",
            "/oauth/token",
            "",
            &[],
            "grant_type=authorization_code&code=x&client_id=nope",
        );
        r.ip = Some("10.9.9.9".into());
        last = e.app.handle(&r).status;
    }
    assert_eq!(last, 429);
    Ok(())
}

/// Every route of the exchange between nodes, with a body each accepts.
const SYNC_ROUTES: [(&str, &str, &str); 7] = [
    ("GET", "status", ""),
    ("GET", "peers", ""),
    ("POST", "peers", r#"{"node": {"name": "x"}, "peers": []}"#),
    ("GET", "spaces", ""),
    ("POST", "spaces", r#"{"space": "health", "archived_at": 1}"#),
    ("GET", "events", ""),
    ("POST", "events", r#"{"events": []}"#),
];

#[test]
fn a_connection_named_like_a_machine_gets_none_of_the_powers_of_one() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    // The name of a machine cannot be asked for, nor taken by a rename.
    assert!(
        access::connect_for(
            &ctl,
            "owner",
            "peer-mine",
            &Rights::owner(),
            None,
            false,
            false
        )
        .is_err()
    );
    connect(&e, "other", "work:r")?;
    assert!(access::rename(&ctl, "other", "peer-other").is_err());
    // A connection an older node let take such a name (by hand, by OAuth, by a rename): the name is all it has.
    let key = connect(&e, "helper", "all")?;
    ctl.execute(
        "UPDATE connections SET agent = 'peer-helper' WHERE agent = 'helper'",
        [],
    )?;
    let at = |method: &str, path: &str, body: &str| {
        e.app
            .handle(&rq(
                method,
                &format!("/mcp/peer-helper/{path}"),
                &key,
                &[],
                body,
            ))
            .status
    };
    assert_eq!(at("POST", "join/invite", "{}"), 403, "it mints no ticket");
    assert_eq!(at("GET", "join/pending", ""), 403);
    assert_eq!(at("POST", "join/approve", r#"{"id": "x"}"#), 403);
    for (method, op, body) in SYNC_ROUTES {
        let r = sync(&e, method, "peer-helper", op, &key, "work", body);
        assert_eq!(r.status, 403, "{method} sync/{op}");
    }
    // A machine the person let in (a pairing code) has them.
    let now = pensine_core::util::now();
    let (code, _) = pensine_core::pairing::create_for_peer(&e.app.brain, "owner", "owner", now)?;
    let paired = pensine_core::pairing::redeem(&ctl, &code, "laptop", now)?;
    let invite = e.app.handle(&rq(
        "POST",
        &format!("/mcp/{}/join/invite", paired.agent),
        &paired.token,
        &[],
        "{}",
    ));
    assert_eq!(invite.status, 200, "{}", invite.body_json());
    Ok(())
}

#[test]
fn an_assistant_key_reaches_no_route_of_the_exchange_between_nodes() -> Res {
    let e = env()?;
    // An assistant that reaches every space, at its own address: the exchange is not for it.
    let key = connect(&e, "assistant", "all")?;
    for (method, op, body) in SYNC_ROUTES {
        let r = sync(&e, method, "assistant", op, &key, "work", body);
        assert_eq!(r.status, 403, "{method} sync/{op}: {}", r.body_json());
    }
    // A machine limited to some spaces keeps the log of those in step, and is told nothing else.
    let narrow = machine(&e, "peer-narrow", "work:r")?;
    assert_eq!(
        sync(&e, "GET", "peer-narrow", "events", &narrow, "work", "").status,
        200
    );
    assert_eq!(
        sync(&e, "GET", "peer-narrow", "spaces", &narrow, "", "").status,
        200
    );
    for (method, op, body) in [
        SYNC_ROUTES[0],
        SYNC_ROUTES[1],
        SYNC_ROUTES[2],
        SYNC_ROUTES[4],
    ] {
        let r = sync(&e, method, "peer-narrow", op, &narrow, "work", body);
        assert_eq!(r.status, 403, "{method} sync/{op}");
    }
    // A full machine of the person is served everywhere.
    let full = machine(&e, "peer-full", "all")?;
    for (method, op, body) in SYNC_ROUTES {
        let r = sync(&e, method, "peer-full", op, &full, "work", body);
        assert_eq!(r.status, 200, "{method} sync/{op}: {}", r.body_json());
    }
    Ok(())
}
