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
