//! Joining by request: a device asks, the person compares a phrase and answers, the key is handed once.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use memcp_core::text::Language;
use memcp_core::{Brain, Space};
use memcp_server::app::App;
use memcp_server::http::{Request, Response};
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);

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
    let dir = std::env::temp_dir().join(format!("memcp-join-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[Space::new("notes")?], Language::English)?;
    let mut app = App::new(brain);
    app.ingress_admins = vec!["Sam".into()];
    app.public_url = "https://memory.example.net".into();
    Ok(Env { dir, app })
}

/// A device on the network: no sign-in, no key.
fn device(e: &Env, ip: &str, method: &str, path: &str, secret: &str, body: Value) -> Response {
    let mut headers = vec![];
    if !secret.is_empty() {
        headers.push(("x-join-secret".to_string(), secret.to_string()));
    }
    e.app.handle(&Request {
        method: method.into(),
        path: path.into(),
        query: Default::default(),
        headers: headers.into_iter().collect(),
        body: body.to_string().into_bytes(),
        ip: Some(ip.into()),
        via_ingress: false,
        tailnet_login: None,
    })
}

/// The person, signed in through the ingress.
fn person(e: &Env, method: &str, path: &str, body: Value) -> Response {
    let mut headers = vec![
        ("x-remote-user-id".to_string(), "ha-sam".to_string()),
        ("x-remote-user-name".to_string(), "Sam".to_string()),
        ("x-remote-user-display-name".to_string(), "Sam".to_string()),
    ];
    if method == "POST" {
        headers.push(("x-requested-with".into(), "memcp".into()));
    }
    e.app.handle(&Request {
        method: method.into(),
        path: format!("/admin/api{path}"),
        query: Default::default(),
        headers: headers.into_iter().collect(),
        body: if method == "POST" {
            body.to_string().into_bytes()
        } else {
            vec![]
        },
        ip: Some("172.30.32.2".into()),
        via_ingress: true,
        tailnet_login: None,
    })
}

fn asked(e: &Env, ip: &str, name: &str) -> Value {
    device(e, ip, "POST", "/join", "", json!({ "device": name })).body_json()
}

#[test]
fn a_device_asks_the_person_compares_a_phrase_and_allows_and_the_key_comes_once() -> Res {
    let e = env()?;
    let _ = person(&e, "GET", "/state", json!({}));
    let ask = asked(&e, "192.168.1.30", "Alex's iPhone");
    let (id, secret, check) = (
        ask["id"].as_str().ok_or("id")?.to_string(),
        ask["secret"].as_str().ok_or("secret")?.to_string(),
        ask["check"].as_str().ok_or("check")?.to_string(),
    );
    assert_eq!(check.split('-').count(), 3, "three words: {check}");
    // The device waits.
    let w = device(
        &e,
        "192.168.1.30",
        "GET",
        &format!("/join/{id}"),
        &secret,
        json!({}),
    );
    assert_eq!(w.body_json()["state"], "waiting");
    // The person sees the same phrase.
    let list = person(&e, "GET", "/join/pending", json!({})).body_json();
    assert_eq!(list["requests"][0]["check"], check.as_str());
    assert_eq!(list["requests"][0]["device"], "Alex's iPhone");
    // And allows it.
    let ok = person(&e, "POST", "/join/approve", json!({ "id": id }));
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    let got = device(
        &e,
        "192.168.1.30",
        "GET",
        &format!("/join/{id}"),
        &secret,
        json!({}),
    );
    let j = got.body_json();
    assert_eq!(j["state"], "approved");
    assert!(
        j["join"]["key"]
            .as_str()
            .is_some_and(|k| k.starts_with("hcp_")),
        "{j}"
    );
    assert!(
        j["join"]["mcp_url"]
            .as_str()
            .is_some_and(|u| u.starts_with("https://memory.example.net/mcp/")),
        "{j}"
    );
    // The key comes once.
    let again = device(
        &e,
        "192.168.1.30",
        "GET",
        &format!("/join/{id}"),
        &secret,
        json!({}),
    );
    assert_eq!(again.status, 404);
    Ok(())
}

#[test]
fn a_denied_request_is_told_to_the_device_and_nothing_is_made() -> Res {
    let e = env()?;
    let _ = person(&e, "GET", "/state", json!({}));
    let ask = asked(&e, "192.168.1.31", "A stranger");
    let (id, secret) = (
        ask["id"].as_str().ok_or("id")?,
        ask["secret"].as_str().ok_or("s")?,
    );
    let no = person(&e, "POST", "/join/deny", json!({ "id": id }));
    assert_eq!(no.status, 200);
    let got = device(
        &e,
        "192.168.1.31",
        "GET",
        &format!("/join/{id}"),
        secret,
        json!({}),
    );
    assert_eq!(got.body_json()["state"], "denied");
    let st = person(&e, "GET", "/state", json!({})).body_json();
    assert_eq!(st["connections"].as_array().map(Vec::len), Some(0));
    Ok(())
}

#[test]
fn only_the_device_that_asked_can_poll_and_only_a_signed_in_person_can_answer() -> Res {
    let e = env()?;
    let _ = person(&e, "GET", "/state", json!({}));
    let ask = asked(&e, "192.168.1.32", "Laptop");
    let (id, secret) = (
        ask["id"].as_str().ok_or("id")?,
        ask["secret"].as_str().ok_or("s")?,
    );
    // Another device, with no secret or a wrong one, learns nothing.
    for s in ["", "wrong"] {
        assert_eq!(
            device(
                &e,
                "192.168.1.99",
                "GET",
                &format!("/join/{id}"),
                s,
                json!({})
            )
            .status,
            404
        );
    }
    assert_eq!(
        device(
            &e,
            "192.168.1.99",
            "GET",
            "/join/unknown",
            secret,
            json!({})
        )
        .status,
        404
    );
    // A device cannot answer for the person: no route, no sign-in.
    let r = device(
        &e,
        "192.168.1.32",
        "POST",
        "/admin/api/join/approve",
        "",
        json!({ "id": id }),
    );
    assert!(
        r.status == 401 || r.status == 404 || r.status == 403,
        "{}",
        r.status
    );
    let list = person(&e, "GET", "/join/pending", json!({})).body_json();
    assert_eq!(
        list["requests"].as_array().map(Vec::len),
        Some(1),
        "still waiting"
    );
    Ok(())
}

#[test]
fn requests_are_limited_per_address_and_in_all() -> Res {
    let e = env()?;
    let mut last = 200;
    for _ in 0..7 {
        last = device(
            &e,
            "192.168.1.40",
            "POST",
            "/join",
            "",
            json!({ "device": "x" }),
        )
        .status;
    }
    assert_eq!(last, 429, "one address cannot flood the person");
    let mut refused = 0;
    for i in 0..12 {
        let ip = format!("192.168.2.{i}");
        if device(&e, &ip, "POST", "/join", "", json!({ "device": "y" })).status == 429 {
            refused += 1;
        }
    }
    assert!(refused >= 1, "only so many requests wait at once");
    Ok(())
}

#[test]
fn a_device_the_person_allowed_in_advance_joins_at_once_and_only_once() -> Res {
    let e = env()?;
    let _ = person(&e, "GET", "/state", json!({}));
    // Nobody allowed it: it waits.
    let first = asked(&e, "192.168.1.31", "Sam's phone");
    assert!(first["check"].is_string());
    let ok = person(
        &e,
        "POST",
        "/join/allow-device",
        json!({ "device": "Sam's Phone", "minutes": 10 }),
    );
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    let ask = asked(&e, "192.168.1.32", "Sam's phone");
    let (id, secret) = (
        ask["id"].as_str().ok_or("id")?.to_string(),
        ask["secret"].as_str().ok_or("secret")?.to_string(),
    );
    let got = device(
        &e,
        "192.168.1.32",
        "GET",
        &format!("/join/{id}"),
        &secret,
        json!({}),
    );
    let j = got.body_json();
    assert_eq!(j["state"], "approved", "{j}");
    assert!(
        j["join"]["key"]
            .as_str()
            .is_some_and(|k| k.starts_with("hcp_"))
    );
    // The permission was used up: the next request waits for a person.
    let next = asked(&e, "192.168.1.33", "Sam's phone");
    let (id2, secret2) = (
        next["id"].as_str().ok_or("id")?.to_string(),
        next["secret"].as_str().ok_or("secret")?.to_string(),
    );
    let w = device(
        &e,
        "192.168.1.33",
        "GET",
        &format!("/join/{id2}"),
        &secret2,
        json!({}),
    );
    assert_eq!(w.body_json()["state"], "waiting");
    Ok(())
}

#[test]
fn a_peer_invites_another_device_with_a_one_time_ticket() -> Res {
    let e = env()?;
    let _ = person(&e, "GET", "/state", json!({}));
    // A first device joins as a peer, through the person.
    let ask = asked(&e, "192.168.1.40", "Sam's Mac");
    let id = ask["id"].as_str().ok_or("id")?.to_string();
    let secret = ask["secret"].as_str().ok_or("secret")?.to_string();
    person(&e, "POST", "/join/approve", json!({ "id": id }));
    let got = device(
        &e,
        "192.168.1.40",
        "GET",
        &format!("/join/{id}"),
        &secret,
        json!({}),
    )
    .body_json();
    let key = got["join"]["key"].as_str().ok_or("key")?.to_string();
    let agent = got["join"]["agent"].as_str().ok_or("agent")?.to_string();
    // The peer asks for a ticket with its own key.
    let invite = |key: &str, agent: &str| {
        e.app.handle(&Request {
            method: "POST".into(),
            path: format!("/mcp/{agent}/join/invite"),
            query: Default::default(),
            headers: [("authorization".to_string(), format!("Bearer {key}"))]
                .into_iter()
                .collect(),
            body: b"{}".to_vec(),
            ip: Some("192.168.1.40".into()),
            via_ingress: false,
            tailnet_login: None,
        })
    };
    let r = invite(&key, &agent);
    assert_eq!(r.status, 200, "{}", r.body_json());
    let ticket = r.body_json()["ticket"]
        .as_str()
        .ok_or("ticket")?
        .to_string();
    assert!(
        r.body_json()["expires_in"]
            .as_i64()
            .is_some_and(|n| n <= 600)
    );
    // A wrong key does not get one.
    assert_eq!(invite("hcp_wrong", &agent).status, 401);
    // The new device joins at once with the ticket, and gets a key of its own.
    let ask = device(
        &e,
        "192.168.1.41",
        "POST",
        "/join",
        "",
        json!({ "device": "Sam's iPad", "ticket": ticket }),
    );
    assert_eq!(ask.status, 200, "{}", ask.body_json());
    let (id2, secret2) = (
        ask.body_json()["id"].as_str().ok_or("id")?.to_string(),
        ask.body_json()["secret"]
            .as_str()
            .ok_or("secret")?
            .to_string(),
    );
    let j = device(
        &e,
        "192.168.1.41",
        "GET",
        &format!("/join/{id2}"),
        &secret2,
        json!({}),
    )
    .body_json();
    assert_eq!(j["state"], "approved", "{j}");
    assert_ne!(
        j["join"]["key"].as_str(),
        Some(key.as_str()),
        "a key of its own"
    );
    // The ticket is single use.
    let again = device(
        &e,
        "192.168.1.42",
        "POST",
        "/join",
        "",
        json!({ "device": "x", "ticket": ticket }),
    );
    assert_eq!(again.status, 403);
    Ok(())
}
