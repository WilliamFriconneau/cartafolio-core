//! Findings of the core audit (docs/reviews/2026-11-17-core-audit.md), one test each, written before the fix.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::{Brain, Space};
use memcp_server::app::App;
use memcp_server::http::{Request, Response};
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
        "eid": format!("c:{id}"), "hlc": format!("{:013}-0000-peer", memcp_core::util::now_ms()),
        "origin": "peer", "kind": "fact_created", "at": memcp_core::util::now(),
        "payload": { "id": id, "kind": "context", "text": format!("text {id}"), "source_kind": "agent",
                     "author": "peer", "created_at": memcp_core::util::now(), "tags": tags },
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
    let whole = connect(&e, "whole", "work:rw")?;
    let narrow = connect(&e, "narrow", "work/sources/app:rw")?;
    // The whole connection syncs: it reads the log and writes through it.
    let body = json!({ "events": [event("f1", &["secret/index"])] }).to_string();
    let r = sync(&e, "POST", "whole", "events", &whole, "work", &body);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        sync(&e, "GET", "whole", "events", &whole, "work", "").status,
        200
    );
    // A connection limited to a path reaches neither: the log is the whole vault.
    assert_eq!(
        sync(&e, "GET", "narrow", "events", &narrow, "work", "").status,
        403
    );
    let body = json!({ "events": [event("f2", &["index"])] }).to_string();
    assert_eq!(
        sync(&e, "POST", "narrow", "events", &narrow, "work", &body).status,
        403
    );
    let listed = sync(&e, "GET", "narrow", "spaces", &narrow, "", "").body_json();
    assert_eq!(listed["spaces"], json!([]), "{listed}");
    Ok(())
}

#[test]
fn a_request_may_not_make_the_server_buffer_a_gigabyte() {
    // The biggest body of any request is what an MCP call may carry (a 24 MB recording in base64), not more.
    assert!(memcp_server::net::LARGE_MAX <= 40 * 1024 * 1024);
}

fn raw(port: u16, req: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).expect("write");
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
    let port: u16 = line.trim().rsplit(':').next().and_then(|p| p.parse().ok()).ok_or("port")?;
    // A wrong key with a big declared body used to be answered 413 for ever, with no count: an oracle for the key.
    let big = "POST /mcp/x HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer wrong\r\nContent-Length: 5000000\r\nConnection: close\r\n\r\n";
    for _ in 0..10 {
        assert!(raw(port, big).starts_with("HTTP/1.1 413"));
    }
    let after = raw(port, "POST /mcp/x HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    let _ = child.kill();
    let _ = child.wait();
    assert!(after.starts_with("HTTP/1.1 429"), "{after}");
    Ok(())
}

#[test]
fn a_header_value_cannot_carry_a_line_break() {
    use memcp_server::net::header_is_safe;
    assert!(header_is_safe("Content-Type", "image/png"));
    assert!(header_is_safe("Content-Disposition", "inline; filename=\"a b.png\""));
    assert!(!header_is_safe("Content-Type", "text/plain\r\nSet-Cookie: x=1"));
    assert!(!header_is_safe("Content-Disposition", "inline\nX: y"));
    assert!(!header_is_safe("X\r\nY", "z"));
}
