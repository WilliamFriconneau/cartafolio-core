//! Phase 3: probes that must hold. Keys (none, wrong, revoked, read-only, limited to a path), a peer of another person
//! at the sync routes, bodies too large or never sent, file names that try to leave their folder or add a header, the
//! guard against guessing keys. The check that no token reaches a log is made at the end of the run, over every node.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::node::{Answer, Env, Key, Node, R, Row, call, fail, read_json, row, words};

const PHASE: &str = "security";

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Sends raw bytes and reads what comes back within `secs` (the status line and headers are enough).
fn raw(port: u16, request: &[u8], secs: u64) -> R<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(fail)?;
    s.set_read_timeout(Some(Duration::from_secs(secs)))
        .map_err(fail)?;
    s.write_all(request).map_err(fail)?;
    let mut buf = vec![0u8; 8192];
    let mut got = Vec::new();
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if got.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    Ok(String::from_utf8_lossy(&got).into_owned())
}

fn status_of(head: &str) -> u16 {
    head.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn refused(a: &Answer) -> bool {
    !a.is_ok()
}

fn http_status(a: &Answer) -> u16 {
    match a {
        Answer::Http(s, _) => *s,
        Answer::Ok(_) | Answer::Refused(_) => 200,
        Answer::Down(_) => 0,
    }
}

fn short(a: &Answer) -> String {
    let s = match a {
        Answer::Ok(v) => format!("ok {v}"),
        Answer::Refused(r) => format!("refused: {r}"),
        Answer::Http(s, v) => format!("{s} {v}"),
        Answer::Down(e) => format!("no answer: {e}"),
    };
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(100)
        .collect()
}

/// The ids in a `recall` answer.
fn ids_in(a: &Answer) -> Vec<String> {
    match a {
        Answer::Ok(v) => v["results"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|r| r["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        _ => vec![],
    }
}

fn created_id(a: &Answer) -> R<String> {
    match a {
        Answer::Ok(v) => v["fact"]["id"]
            .as_str()
            .or_else(|| v["file"]["id"].as_str())
            .or_else(|| v["id"].as_str())
            .map(String::from)
            .ok_or_else(|| format!("no id in {v}")),
        other => Err(format!("not created: {other:?}")),
    }
}

pub fn run(env: &Env) -> R<Vec<Row>> {
    let mut rows = vec![];
    let mut n = Node::init(env, "secure", true)?;
    n.serve()?;
    let agent = crate::node::http();
    let full = n.connect("full", &["--all"])?;
    let ro = n.connect("reader", &["--all", "--read-only"])?;
    let gone = n.connect("gone", &["--all"])?;
    n.cli(&["revoke", "gone"])?;
    let scoped = n.connect("scoped", &["--grant", "work/projets/alpha:rw"])?;
    let triage = n.triage_box()?;
    let ok = |a: &Answer| a.is_ok();

    // Notes and a file the scoped key must not reach (projets/beta), and one it may (projets/alpha).
    let outside_word = "zorbaflux";
    let outside = created_id(&call(
        &agent,
        &full.url,
        Some(&full.key),
        "remember",
        &json!({ "space": "work", "text": format!("{outside_word} {}", words(31, 5)), "tags": ["projets/beta"] }),
    ))?;
    let inside = created_id(&call(
        &agent,
        &full.url,
        Some(&full.key),
        "remember",
        &json!({ "space": "work", "text": format!("quillomb {}", words(32, 5)), "tags": ["projets/alpha"] }),
    ))?;
    let file_out = call(
        &agent,
        &full.url,
        Some(&full.key),
        "file_put",
        &json!({ "space": "work", "name": "plan.txt", "note": format!("{outside_word} plan"), "tags": ["projets/beta"],
                 "data_base64": b64(b"a plan, outside the path") }),
    );
    let file_out = match &file_out {
        Answer::Ok(v) => v["file"]["id"]
            .as_str()
            .or_else(|| v["id"].as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    };
    let boxed = created_id(&call(
        &agent,
        &full.url,
        Some(&full.key),
        "remember",
        &json!({ "space": triage, "text": words(33, 6) }),
    ))?;

    // 1. No key, a wrong key, a revoked key: refused, at the MCP address and at the sync routes.
    let none = call(&agent, &full.url, None, "recall", &json!({ "query": "ka" }));
    let wrong = call(
        &agent,
        &full.url,
        Some("hcp_00000000_0123456789abcdef0123456789abcdef0123456789abcdef"),
        "recall",
        &json!({ "query": "ka" }),
    );
    crate::node::keep_token("0123456789abcdef0123456789abcdef0123456789abcdef");
    let revoked = call(
        &agent,
        &gone.url,
        Some(&gone.key),
        "recall",
        &json!({ "query": "ka" }),
    );
    let sync_none = {
        let mut r = agent
            .get(&format!("{}/sync/spaces", full.url))
            .call()
            .map_err(fail)?;
        read_json(&mut r).0
    };
    let held = [&none, &wrong, &revoked]
        .iter()
        .all(|a| http_status(a) == 401)
        && sync_none == 401;
    rows.push(row(
        PHASE,
        "no key / wrong key / revoked key refused",
        Some(held),
        format!(
            "{} / {} / {} (sync without a key: {sync_none})",
            http_status(&none),
            http_status(&wrong),
            http_status(&revoked)
        ),
    ));

    // 2. A read-only key writes nothing.
    let attempts = [
        ("remember", json!({ "space": "work", "text": words(40, 6) })),
        ("supersede", json!({ "id": inside, "text": words(41, 6) })),
        ("retire", json!({ "id": inside, "reason": "probe" })),
        ("tags_set", json!({ "id": inside, "add": ["probe"] })),
        (
            "file_put",
            json!({ "space": "work", "name": "x.txt", "data_base64": b64(b"x") }),
        ),
        (
            "living_set",
            json!({ "space": "work", "page": "next", "add": [{ "column": "now", "text": "x" }] }),
        ),
    ];
    let wrote: Vec<&str> = attempts
        .iter()
        .filter(|(t, a)| !refused(&call(&agent, &ro.url, Some(&ro.key), t, a)))
        .map(|(t, _)| *t)
        .collect();
    rows.push(row(
        PHASE,
        "a read-only key cannot write",
        Some(wrote.is_empty()),
        if wrote.is_empty() {
            format!("{} writes refused", attempts.len())
        } else {
            format!("written by: {}", wrote.join(", "))
        },
    ));

    // 3. A key limited to work/projets/alpha: nothing outside, neither read nor written.
    let mut leaks = vec![];
    let control = call(
        &agent,
        &scoped.url,
        Some(&scoped.key),
        "remember",
        &json!({ "space": "work", "text": words(50, 6), "tags": ["projets/alpha"] }),
    );
    let probes: Vec<(&str, Value)> = vec![
        (
            "remember",
            json!({ "space": "work", "text": words(51, 6), "tags": ["projets/beta"] }),
        ),
        (
            "remember",
            json!({ "space": "personal", "text": words(52, 6) }),
        ),
        (
            "file_put",
            json!({ "space": "work", "name": "y.txt", "tags": ["projets/beta"], "data_base64": b64(b"y") }),
        ),
        ("file_get", json!({ "id": file_out })),
        (
            "tags_set",
            json!({ "id": outside, "add": ["projets/alpha"] }),
        ),
        (
            "tags_set",
            json!({ "id": inside, "tags": ["projets/beta"] }),
        ),
        (
            "living_set",
            json!({ "space": "work", "page": "next", "add": [{ "column": "now", "text": "x" }] }),
        ),
        ("triage_send", json!({ "id": boxed, "to": "work" })),
        ("supersede", json!({ "id": outside, "text": words(53, 6) })),
        (
            "supersede",
            json!({ "id": inside, "text": words(54, 6), "tags": ["projets/beta"] }),
        ),
        ("retire", json!({ "id": outside, "reason": "probe" })),
    ];
    for (t, a) in &probes {
        let r = call(&agent, &scoped.url, Some(&scoped.key), t, a);
        if !refused(&r) {
            leaks.push(format!("{t} {a}: {}", short(&r)));
        }
    }
    let seen = call(
        &agent,
        &scoped.url,
        Some(&scoped.key),
        "recall",
        &json!({ "query": outside_word }),
    );
    if ids_in(&seen).contains(&outside)
        || matches!(&seen, Answer::Ok(v) if v.to_string().contains(outside_word))
    {
        leaks.push("recall returns a note outside the path".into());
    }
    let listed = call(&agent, &scoped.url, Some(&scoped.key), "stats", &json!({}));
    if matches!(&listed, Answer::Ok(v) if v.to_string().contains("projets/beta")) {
        leaks.push("stats names a path outside".into());
    }
    rows.push(row(
        PHASE,
        "a path-limited key stays inside its path",
        Some(leaks.is_empty() && ok(&control)),
        if leaks.is_empty() {
            format!(
                "{} probes refused; a write inside: {}",
                probes.len() + 2,
                if ok(&control) { "allowed" } else { "REFUSED" }
            )
        } else {
            leaks.join("; ")
        },
    ));

    // 4. A peer of another person at the sync routes.
    let (status, v) = n.admin_post("/users", &json!({ "name": "bob", "display": "Bob" }))?;
    if status != 200 {
        return Err(format!("a second person: {status} {v}"));
    }
    let bob = n.connect("peer-bobnode", &["--user", "bob", "--all"])?;
    let mut crossed = vec![];
    let get = |path: &str| -> R<(u16, Value)> {
        let mut r = agent
            .get(&format!("{}{path}", bob.url))
            .header("Authorization", &format!("Bearer {}", bob.key))
            .call()
            .map_err(fail)?;
        Ok(read_json(&mut r))
    };
    let post = |path: &str, body: &Value| -> R<(u16, Value)> {
        let mut r = agent
            .post(&format!("{}{path}", bob.url))
            .header("Authorization", &format!("Bearer {}", bob.key))
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(fail)?;
        Ok(read_json(&mut r))
    };
    let (st, made) = n.admin_post(
        "/spaces/create",
        &json!({ "label": "Garage", "parent": "home" }),
    )?;
    let garage = made["space"]["space"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if st != 200 || garage.is_empty() {
        return Err(format!("a sub-space: {st} {made}"));
    }
    let before = n.layout()?;
    let owners: Vec<String> = before.iter().map(|p| p.space.clone()).collect();
    let (_, listed) = get("/sync/spaces")?;
    for s in listed["spaces"].as_array().cloned().unwrap_or_default() {
        if owners
            .iter()
            .any(|o| Some(o.as_str()) == s["space"].as_str())
        {
            crossed.push(format!("lists {}", s["space"]));
        }
    }
    let (st, _) = get("/sync/events?space=work&after=0")?;
    if st == 200 {
        crossed.push("pulls work".into());
    }
    let (st, _) = get(&format!("/sync/events?space={triage}&after=0"))?;
    if st == 200 {
        crossed.push("pulls the owner's À trier".into());
    }
    let (st, _) = post("/sync/events?space=work", &json!({ "events": [] }))?;
    if st == 200 {
        crossed.push("pushes into work".into());
    }
    let (st, v) = post(
        "/sync/spaces",
        &json!({ "space": "work", "kind": "custom", "label": "Taken", "parent": null, "position": 0, "since": 1 }),
    )?;
    if st == 200 {
        crossed.push(format!("offers work: {v}"));
    }
    let (st, v) = post(
        "/sync/spaces",
        &json!({ "space": "personal", "archived_at": 4_000_000_000i64 }),
    )?;
    if st == 200 && v["result"] == json!("archived") {
        crossed.push("archives personal".into());
    }
    let (st, v) = post(
        "/sync/spaces",
        &json!({ "space": triage, "kind": "a-trier", "label": null, "parent": null, "position": 0, "since": 1 }),
    )?;
    if st == 200 && v["space"] == json!(triage) {
        crossed.push(format!("pairs with the owner's À trier: {v}"));
    }
    let (st, v) = post(
        "/sync/spaces",
        &json!({ "space": garage, "kind": "a-trier", "label": null, "parent": null, "position": 0, "since": 1 }),
    )?;
    if st == 200 {
        crossed.push(format!("takes the owner's {garage} as its box: {v}"));
    }
    let after = n.layout()?;
    if after != before {
        crossed.push("the owner's tree changed".into());
    }
    let still = n
        .notes_of("work")
        .map(|m| m.contains_key(&outside))
        .unwrap_or(false);
    if !still {
        crossed.push("a note of work is gone".into());
    }
    rows.push(row(
        PHASE,
        "a peer of another person cannot offer or pull spaces",
        Some(crossed.is_empty()),
        if crossed.is_empty() {
            "8 probes refused".to_string()
        } else {
            crossed.join("; ")
        },
    ));

    // 5. A huge declared body: refused before it is read, and the node lives on.
    let huge = format!(
        "POST /mcp/full HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 99999999999\r\n\r\n",
        full.key
    );
    let h1 = status_of(&raw(n.port, huge.as_bytes(), 10)?);
    let h2 = status_of(&raw(
        n.port,
        b"POST /mcp/full HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 1073741824\r\n\r\n{",
        10,
    )?);
    let alive = call(
        &agent,
        &full.url,
        Some(&full.key),
        "recall",
        &json!({ "query": "ka" }),
    );
    rows.push(row(
        PHASE,
        "a huge Content-Length is refused, the node lives",
        Some(h1 == 413 && h2 == 413 && ok(&alive) && n.alive()),
        format!(
            "with a key {h1}, without {h2}; then {}",
            if ok(&alive) { "answers" } else { "NO ANSWER" }
        ),
    ));

    // 5b. A request line that never ends: the node keeps a few KiB of it, not all.
    let pid = n.pid().unwrap_or(0);
    let rss0 = crate::node::rss_kib(pid).unwrap_or(0);
    let mut sent = 0usize;
    if let Ok(mut s) = TcpStream::connect(("127.0.0.1", n.port)) {
        let chunk = vec![b'a'; 1 << 20];
        let _ = s.write_all(b"GET /");
        for _ in 0..64 {
            if s.write_all(&chunk).is_err() {
                break;
            }
            sent += 1;
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let rss1 = crate::node::rss_kib(pid).unwrap_or(0);
    let grew = rss1.saturating_sub(rss0) / 1024;
    let alive = call(
        &agent,
        &full.url,
        Some(&full.key),
        "recall",
        &json!({ "query": "ka" }),
    );
    rows.push(row(
        PHASE,
        "a request line without an end does not fill the memory",
        Some(grew < 32 && ok(&alive)),
        format!("{sent} MiB of 64 taken before the node closed; memory +{grew} MiB"),
    ));

    // 6. File names that try to leave their folder or to add a header.
    let mut escaped = vec![];
    let names = [
        "../../../escape-bench.txt",
        "..\\..\\escape-bench2.txt",
        "evil\r\nX-Injected: yes\r\n.txt",
        "a\u{0}b.txt",
    ];
    for name in names {
        let r = call(
            &agent,
            &full.url,
            Some(&full.key),
            "file_put",
            &json!({ "space": "work", "name": name, "note": "probe", "data_base64": b64(b"probe") }),
        );
        let Answer::Ok(v) = &r else { continue };
        let id = v["file"]["id"]
            .as_str()
            .or_else(|| v["id"].as_str())
            .unwrap_or_default()
            .to_string();
        let head = raw(n.port, format!("GET /admin/api/file?space=work&id={id} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", n.console()?).as_bytes(), 10)?;
        if head
            .lines()
            .any(|l| l.to_ascii_lowercase().starts_with("x-injected"))
        {
            escaped.push(format!("{name:?} adds a header"));
        }
        let got = call(
            &agent,
            &full.url,
            Some(&full.key),
            "file_get",
            &json!({ "id": id }),
        );
        if !ok(&got) {
            escaped.push(format!("{name:?} kept but not given back: {}", short(&got)));
        }
    }
    for leaked in ["escape-bench.txt", "escape-bench2.txt"] {
        if find(&env.work, leaked, &n.data) {
            escaped.push(format!("{leaked} written outside the node's folder"));
        }
    }
    rows.push(row(
        PHASE,
        "file names with ../ or control characters stay inside",
        Some(escaped.is_empty()),
        if escaped.is_empty() {
            format!("{} names probed", names.len())
        } else {
            escaped.join("; ")
        },
    ));

    // 7. Repeated bad keys: the guard turns the address away.
    let mut turned_away_after = None;
    for i in 1..=30 {
        let a = call(
            &agent,
            &full.url,
            Some(&format!(
                "hcp_bad{i:05}_0000000000000000000000000000000000000000"
            )),
            "recall",
            &json!({ "query": "ka" }),
        );
        if http_status(&a) == 429 {
            turned_away_after = Some(i);
            break;
        }
    }
    let good_then = http_status(&call(
        &agent,
        &full.url,
        Some(&full.key),
        "recall",
        &json!({ "query": "ka" }),
    ));
    rows.push(row(PHASE, "rate limit on repeated bad keys", Some(turned_away_after.is_some() && n.alive()),
        match turned_away_after {
            Some(i) => format!("429 after {i} bad keys (5 failures earlier in this phase); a good key from the same address then gets {good_then}"),
            None => "never turned away in 30 tries".into(),
        }));
    n.kill9();

    // 8. 64 connections that declare a body and never send it, on a node of their own.
    rows.push(idle(env)?);
    Ok(rows)
}

/// Whether a file of that name exists in the bench's directory outside the node's own folder.
fn find(dir: &Path, name: &str, inside: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.starts_with(inside) {
            continue;
        }
        if p.file_name().is_some_and(|f| f == name) {
            return true;
        }
        if p.is_dir() && find(&p, name, inside) {
            return true;
        }
    }
    false
}

fn idle(env: &Env) -> R<Row> {
    let mut n = Node::init(env, "idle", true)?;
    n.serve()?;
    let agent = crate::node::http();
    let k: Key = n.connect("full", &["--all"])?;
    let mut held = vec![];
    // Bodies over 1 KiB: a smaller one is read by the HTTP layer before the request is handed on, and never takes a
    // place of the node.
    for _ in 0..64 {
        let mut s = TcpStream::connect(("127.0.0.1", n.port)).map_err(fail)?;
        s.write_all(b"POST /mcp/full HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 200000\r\n\r\n{")
            .map_err(fail)?;
        held.push(s);
    }
    std::thread::sleep(Duration::from_millis(500));
    // The person's assistant, with its key, while the 64 wait.
    let t0 = Instant::now();
    let during = crate::node::call_retry(&agent, &k, "recall", &json!({ "query": "ka" }), 5);
    let during_ms = t0.elapsed().as_millis();
    // After the server's read timeout, if any, the places come back without the attacker closing anything.
    let t1 = Instant::now();
    let mut later = during.clone();
    while !later.is_ok() && t1.elapsed() < Duration::from_secs(45) {
        std::thread::sleep(Duration::from_millis(500));
        later = call(
            &agent,
            &k.url,
            Some(&k.key),
            "recall",
            &json!({ "query": "ka" }),
        );
    }
    let freed_after = t1.elapsed().as_secs();
    drop(held);
    let alive = n.alive();
    n.kill9();
    Ok(row(
        PHASE,
        "64 idle connections declaring a body do not stop the node",
        Some(during.is_ok() && alive),
        if during.is_ok() {
            format!("a keyed call answered in {during_ms} ms while they waited")
        } else {
            format!(
                "a keyed call got {} for 5 s; {}",
                short(&during),
                if later.is_ok() {
                    format!("answered again {freed_after} s later with the 64 still open")
                } else {
                    "still refused 45 s later".into()
                }
            )
        },
    ))
}
