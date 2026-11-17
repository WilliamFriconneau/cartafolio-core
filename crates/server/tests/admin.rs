//! The admin page: who may sign in, what an administrator and a member may do,
//! and the guards (ingress headers trusted only on the ingress listener, CSRF header,
//! MCP tokens refused, tokens never beyond their person).

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::users;
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
    let dir = std::env::temp_dir().join(format!("memcp-admin-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[sp("home"), sp("finances")], Language::English)?;
    let mut app = App::new(brain);
    app.ingress_admins = vec!["Sam".into()];
    app.public_url = "https://memory.example.net".into();
    // The tests below drive the page with console tokens on the main listener.
    app.console_admin = true;
    Ok(Env { dir, app })
}

/// A request through the ingress listener, signed in as a Home Assistant user.
fn ha(e: &Env, method: &str, path: &str, user: (&str, &str), body: Value) -> Response {
    let mut headers = vec![
        ("x-remote-user-id".to_string(), user.0.to_string()),
        ("x-remote-user-name".to_string(), user.1.to_string()),
        ("x-remote-user-display-name".to_string(), user.1.to_string()),
    ];
    if method == "POST" {
        headers.push(("x-requested-with".into(), "memcp".into()));
    }
    e.app.handle(&Request {
        method: method.into(),
        path: path.into(),
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

fn direct(e: &Env, method: &str, path: &str, headers: &[(&str, &str)], body: Value) -> Response {
    e.app.handle(&Request {
        method: method.into(),
        path: path.into(),
        query: Default::default(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            .collect(),
        body: body.to_string().into_bytes(),
        ip: Some("10.0.0.9".into()),
        via_ingress: false,
        tailnet_login: None,
    })
}

const SAM: (&str, &str) = ("ha-sam", "Sam");
const ALEX: (&str, &str) = ("ha-alex", "Alex");

fn post(e: &Env, who: (&str, &str), path: &str, body: Value) -> Result<Value, Box<dyn Error>> {
    let r = ha(e, "POST", &format!("/admin/api/{path}"), who, body);
    if r.status != 200 {
        return Err(format!("{path}: {} {}", r.status, r.body_json()).into());
    }
    Ok(r.body_json())
}

/// A new login only files a request; Sam, an administrator from the list, approves it.
fn join(e: &Env, who: (&str, &str)) -> Res {
    state(e, SAM);
    let r = ha(e, "GET", "/admin/api/state", who, json!({}));
    assert_eq!(r.status, 403, "a new login waits: {}", r.body_json());
    post(e, SAM, "requests/approve", json!({ "ha_user": who.0 }))?;
    Ok(())
}

fn state(e: &Env, who: (&str, &str)) -> Value {
    ha(e, "GET", "/admin/api/state", who, json!({})).body_json()
}

/// Gives a person (made at their first sign-in) a vault of a name of their own: what an operator
/// does from the command line.
fn own(e: &Env, ha: (&str, &str), space: &str) -> Res {
    let _ = state(e, ha);
    let ctl = e.app.brain.open_control()?;
    let who = users::list_users(&ctl)?
        .into_iter()
        .find(|u| u.ha_user.as_deref() == Some(ha.0))
        .ok_or("no such person")?;
    e.app.brain.add_space_for(&sp(space), &who.id)?;
    Ok(())
}

#[test]
fn the_page_is_served_and_self_contained() -> Res {
    let e = env()?;
    for (r, label) in [
        (ha(&e, "GET", "/", SAM, json!({})), "ingress root"),
        (direct(&e, "GET", "/admin", &[], json!({})), "/admin"),
    ] {
        assert_eq!(r.status, 200, "{label}");
        let html = String::from_utf8(r.body.clone())?;
        assert!(
            html.contains("memcp") && !html.contains("http://") && !html.contains("https://"),
            "no external resource"
        );
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "Content-Security-Policy" && v.contains("default-src 'none'"))
        );
    }
    Ok(())
}

#[test]
fn a_new_home_assistant_login_files_a_request_and_nothing_else() -> Res {
    let e = env()?;
    // Sam is named in the administrators of the options: a person at once.
    let s = state(&e, SAM);
    assert_eq!(
        (s["me"]["name"].clone(), s["me"]["admin"].clone()),
        (json!("sam"), json!(true))
    );
    // Alex is not: he waits, and says why.
    let r = ha(&e, "GET", "/admin/api/state", ALEX, json!({}));
    assert_eq!(r.status, 403);
    assert!(
        r.body_json()["error"]
            .as_str()
            .is_some_and(|m| m.contains("waiting"))
    );
    let ctl = e.app.brain.open_control()?;
    assert!(
        users::user_by_name(&ctl, "alex")?.is_none(),
        "no person yet"
    );
    ha(&e, "GET", "/admin/api/state", ALEX, json!({}));
    let s = state(&e, SAM);
    let waiting = s["requests"].as_array().ok_or("requests")?;
    assert_eq!(waiting.len(), 1, "one request, not one per visit");
    assert_eq!(waiting[0]["ha_user"], "ha-alex");
    assert_eq!(waiting[0]["status"], "pending");

    // Approving makes him a member.
    post(&e, SAM, "requests/approve", json!({ "ha_user": "ha-alex" }))?;
    let a = state(&e, ALEX);
    assert_eq!(
        (a["me"]["name"].clone(), a["me"]["admin"].clone()),
        (json!("alex"), json!(false))
    );
    assert_eq!(a["requests"], json!([]), "a member does not see requests");
    assert!(
        users::user_by_name(&ctl, "alex")?.is_some_and(|u| u.ha_user.as_deref() == Some("ha-alex"))
    );
    assert_eq!(state(&e, SAM)["requests"], json!([]), "the request is gone");
    Ok(())
}

#[test]
fn only_an_administrator_answers_a_request_and_a_refusal_sticks() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    let ben: (&str, &str) = ("ha-ben", "Ben");
    assert_eq!(
        ha(&e, "GET", "/admin/api/state", ben, json!({})).status,
        403
    );
    // A member cannot approve or refuse.
    for path in ["requests/approve", "requests/refuse"] {
        assert!(
            post(&e, ALEX, path, json!({ "ha_user": "ha-ben" })).is_err(),
            "{path}"
        );
    }
    post(&e, SAM, "requests/refuse", json!({ "ha_user": "ha-ben" }))?;
    for _ in 0..2 {
        let r = ha(&e, "GET", "/admin/api/state", ben, json!({}));
        assert_eq!(r.status, 403);
        assert!(
            r.body_json()["error"]
                .as_str()
                .is_some_and(|m| m.contains("refused"))
        );
    }
    let s = state(&e, SAM);
    assert_eq!(s["requests"][0]["status"], "refused");
    assert!(post(&e, SAM, "requests/approve", json!({ "ha_user": "ha-ben" })).is_err());
    Ok(())
}

#[test]
fn ingress_headers_are_ignored_on_the_normal_listener() -> Res {
    let e = env()?;
    let forged = [
        ("x-remote-user-id", "ha-sam"),
        ("x-remote-user-name", "Sam"),
    ];
    let r = direct(&e, "GET", "/admin/api/state", &forged, json!({}));
    assert_eq!(r.status, 401, "headers from the network are not a login");
    // And the ingress listener serves nothing but the admin page.
    assert_eq!(ha(&e, "POST", "/mcp", SAM, json!({})).status, 404);
    Ok(())
}

#[test]
fn console_tokens_open_the_page_mcp_tokens_do_not() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    let console = access::console_token(&ctl, users::OWNER)?;
    let mcp = access::connect(&ctl, "agent", &Rights::owner(), None, false, false)?;
    let with = |t: &str| {
        direct(
            &e,
            "GET",
            "/admin/api/state",
            &[("authorization", &format!("Bearer {t}"))],
            json!({}),
        )
    };
    assert_eq!(with(&console).status, 200);
    assert_eq!(
        with(&mcp).status,
        401,
        "an MCP token never opens the console"
    );
    // And a console token never opens MCP.
    let r = direct(
        &e,
        "POST",
        "/mcp/anyone",
        &[("authorization", &format!("Bearer {console}"))],
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
    );
    assert_eq!(r.status, 401);
    // Not listed among assistant connections.
    assert!(
        access::list(&ctl)?
            .iter()
            .all(|c| !c.agent.starts_with("console-"))
    );
    Ok(())
}

#[test]
fn writes_need_the_csrf_header() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    let console = access::console_token(&ctl, users::OWNER)?;
    let auth = format!("Bearer {console}");
    let r = direct(
        &e,
        "POST",
        "/admin/api/connections",
        &[("authorization", &auth)],
        json!({ "agent": "csrf-check" }),
    );
    assert_eq!(r.status, 403);
    let r = direct(
        &e,
        "POST",
        "/admin/api/connections",
        &[("authorization", &auth), ("x-requested-with", "memcp")],
        json!({ "agent": "csrf-check" }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    Ok(())
}

#[test]
fn a_member_creates_their_own_space_and_tokens_but_nothing_else() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    let t = post(
        &e,
        ALEX,
        "connections",
        json!({ "agent": "alex-claude", "rights": "all" }),
    )?;
    assert!(t["token"].as_str().is_some_and(|t| t.starts_with("hcp_")));
    assert!(
        t["claude_code"]
            .as_str()
            .is_some_and(|c| c.contains("https://memory.example.net/mcp"))
    );
    let ctl = e.app.brain.open_control()?;
    let caller = access::authenticate(&ctl, t["token"].as_str().ok_or("token")?, 1_900_000_000)?
        .ok_or("refused")?;
    assert!(
        caller.can_write(&sp("alex-me")) && !caller.can_read(&sp("home")),
        "only what Alex reaches"
    );

    for (path, body) in [
        ("users", json!({ "name": "eve" })),
        ("spaces", json!({ "name": "other", "user": "sam" })),
        ("connections", json!({ "agent": "y", "rights": "home:rw" })),
    ] {
        let r = ha(
            &e,
            "POST",
            &format!("/admin/api/{path}"),
            ALEX,
            body.clone(),
        );
        assert!(
            r.status == 403 || r.status == 400,
            "{path} {body}: {}",
            r.status
        );
    }
    Ok(())
}

#[test]
fn a_disabled_home_assistant_user_is_locked_out() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    let ctl = e.app.brain.open_control()?;
    let alex = users::user_by_name(&ctl, "alex")?.ok_or("alex")?;
    users::set_disabled(&ctl, users::OWNER, &alex.id, true)?;
    assert_eq!(
        ha(&e, "GET", "/admin/api/state", ALEX, json!({})).status,
        401
    );
    Ok(())
}

#[test]
fn files_are_uploaded_and_opened_from_the_page_within_each_person_reach() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    let pdf = "JVBERi0xLjQgZmFrZQ=="; // "%PDF-1.4 fake"
    let up = post(
        &e,
        ALEX,
        "files",
        json!({ "space": "alex-me", "name": "lease.pdf", "note": "Flat lease", "data": pdf }),
    )?;
    let id = up["file"]["id"].as_str().ok_or("id")?.to_string();
    let s = state(&e, ALEX);
    assert!(
        s["files"]
            .as_array()
            .is_some_and(|f| f.iter().any(|f| f["name"] == "lease.pdf"))
    );
    let get = |who| {
        let mut r = ha(&e, "GET", "/admin/api/file", who, json!({}));
        if r.status == 200 {
            return r;
        }
        let mut req_q = std::collections::HashMap::new();
        req_q.insert("space".to_string(), "alex-me".to_string());
        req_q.insert("id".to_string(), id.clone());
        r = e.app.handle(&Request {
            method: "GET".into(),
            path: "/admin/api/file".into(),
            query: req_q,
            headers: [
                ("x-remote-user-id".to_string(), who.0.to_string()),
                ("x-remote-user-name".to_string(), who.1.to_string()),
            ]
            .into_iter()
            .collect(),
            body: vec![],
            ip: Some("172.30.32.2".into()),
            via_ingress: true,
            tailnet_login: None,
        });
        r
    };
    let r = get(ALEX);
    assert_eq!(r.status, 200);
    assert!(r.body.starts_with(b"%PDF"));
    assert!(
        r.headers
            .iter()
            .any(|(k, v)| k == "Content-Security-Policy" && v.starts_with("sandbox"))
    );
    assert_eq!(get(SAM).status, 404, "Sam does not reach Alex's space");
    let r = ha(
        &e,
        "POST",
        "/admin/api/files",
        SAM,
        json!({ "space": "alex-me", "name": "x.pdf", "data": pdf }),
    );
    assert_eq!(r.status, 403);
    Ok(())
}

#[test]
fn opening_a_vault_returns_the_address_and_a_token_limited_to_it() -> Res {
    let e = env()?;
    own(&e, SAM, "sam-notes")?;
    let r = post(&e, SAM, "spaces/open", json!({ "space": "sam-notes" }))?;
    assert_eq!(r["rights"], "sam-notes:rw");
    assert_eq!(
        r["mcp_url"],
        format!(
            "https://memory.example.net/mcp/{}",
            r["agent"].as_str().ok_or("agent")?
        )
    );
    let token = r["token"].as_str().ok_or("no token")?.to_string();
    let agent_name = r["agent"].as_str().ok_or("agent")?.to_string();
    assert!(token.starts_with("hcp_"));
    assert!(
        r["claude_code"]
            .as_str()
            .is_some_and(|c| c.contains("https://memory.example.net/mcp"))
    );

    // The token reaches that vault and nothing else.
    let auth = format!("Bearer {token}");
    let call = |space: &str| {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "recall", "arguments": { "query": "x", "space": space } } });
        direct(
            &e,
            "POST",
            &format!("/mcp/{agent_name}"),
            &[("authorization", auth.as_str())],
            body,
        )
        .body_json()
    };
    assert_eq!(call("sam-notes")["result"]["isError"], false);
    assert_eq!(call("home")["result"]["isError"], true);
    Ok(())
}

#[test]
fn a_member_cannot_open_a_vault_they_do_not_reach() -> Res {
    let e = env()?;
    own(&e, SAM, "sam-notes")?;
    // Alex signs in once, then asks for Sam's vault.
    let _ = state(&e, ALEX);
    assert!(post(&e, ALEX, "spaces/open", json!({ "space": "sam-notes" })).is_err());
    Ok(())
}

#[test]
fn the_page_shows_counters_for_each_vault_and_never_the_content() -> Res {
    let e = env()?;
    own(&e, SAM, "sam-notes")?;
    let s = state(&e, SAM);
    let v = s["spaces"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["name"] == "sam-notes"))
        .ok_or("vault missing")?;
    assert_eq!(v["stats"]["facts"], 0);
    assert_eq!(v["stats"]["files"], 0);
    assert!(v["stats"]["last_write"].is_null());
    Ok(())
}

fn bearer_get(e: &Env, path: &str, token: &str, extra: &[(&str, &str)]) -> Response {
    let auth = format!("Bearer {token}");
    let mut headers = vec![("authorization", auth.as_str())];
    headers.extend_from_slice(extra);
    direct(e, "GET", path, &headers, json!({}))
}

#[test]
fn the_admin_is_not_served_on_the_main_listener_unless_enabled() -> Res {
    let mut e = env()?;
    let ctl = e.app.brain.open_control()?;
    let console = access::console_token(&ctl, users::OWNER)?;
    assert_eq!(
        bearer_get(&e, "/admin/api/state", &console, &[]).status,
        200,
        "enabled"
    );
    e.app.console_admin = false;
    assert_eq!(direct(&e, "GET", "/admin", &[], json!({})).status, 404);
    assert_eq!(
        bearer_get(&e, "/admin/api/state", &console, &[]).status,
        404,
        "a valid console token opens nothing when the page is not served here"
    );
    // The Home Assistant ingress is unaffected.
    assert_eq!(state(&e, SAM)["me"]["name"], "sam");
    Ok(())
}

#[test]
fn the_admin_is_never_served_through_funnel() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    let console = access::console_token(&ctl, users::OWNER)?;
    let funnel = [("tailscale-funnel-request", "?1")];
    assert_eq!(
        bearer_get(&e, "/admin/api/state", &console, &funnel).status,
        404
    );
    let page = direct(&e, "GET", "/admin", &funnel, json!({}));
    assert_eq!(page.status, 404);
    assert_eq!(
        bearer_get(&e, "/admin/api/state", &console, &[]).status,
        200,
        "the same token works when the request did not come through Funnel"
    );
    Ok(())
}

#[test]
fn failed_console_tokens_are_counted_and_then_blocked() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    let console = access::console_token(&ctl, users::OWNER)?;
    for i in 0..10 {
        let guess = format!("hcp_{i:08x}_{}", "0".repeat(48));
        assert_eq!(bearer_get(&e, "/admin/api/state", &guess, &[]).status, 401);
    }
    let blocked = bearer_get(&e, "/admin/api/state", &console, &[]);
    assert_eq!(blocked.status, 429, "even the right token waits");
    assert!(blocked.headers.iter().any(|(k, _)| k == "Retry-After"));
    // The Home Assistant ingress has no token to guess and is not affected.
    assert_eq!(state(&e, SAM)["me"]["name"], "sam");
    Ok(())
}

#[test]
fn a_person_an_administrator_created_is_linked_at_first_login_without_a_request() -> Res {
    let e = env()?;
    state(&e, SAM);
    post(&e, SAM, "users", json!({ "name": "kim", "display": "Kim" }))?;
    let kim: (&str, &str) = ("ha-kim", "Kim");
    let k = state(&e, kim);
    assert_eq!(k["me"]["name"], "kim");
    assert_eq!(state(&e, SAM)["requests"], json!([]));
    Ok(())
}

#[test]
fn a_member_makes_vaults_from_the_catalogue_and_names_none() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    // The page offers the kinds of the catalogue, and the way each vault is shown.
    let a = state(&e, ALEX);
    let keys: Vec<&str> = a["catalogue"]
        .as_array()
        .ok_or("catalogue")?
        .iter()
        .filter_map(|k| k["key"].as_str())
        .collect();
    assert_eq!(
        keys,
        ["me", "health", "people", "home", "money", "work", "hobbies"]
    );
    // A member cannot name a vault by hand, nor use a kind that is not offered.
    assert!(post(&e, ALEX, "spaces", json!({ "name": "mine" })).is_err());
    assert!(post(&e, ALEX, "spaces", json!({ "kind": "notes" })).is_err());
    // From a kind, the vault is named after the person and the kind, and keeps its kind.
    let r = post(&e, ALEX, "spaces", json!({ "kind": "health" }))?;
    assert_eq!(r["space"], "alex-health");
    let a = state(&e, ALEX);
    let v = a["spaces"]
        .as_array()
        .and_then(|l| l.iter().find(|v| v["name"] == "alex-health"))
        .ok_or("vault")?;
    assert_eq!(v["kind"], "health");
    assert!(
        post(&e, ALEX, "spaces", json!({ "kind": "health" })).is_err(),
        "one of each kind"
    );
    Ok(())
}

/// A GET through the ingress with a query string.
fn ha_get(e: &Env, path: &str, user: (&str, &str), query: &[(&str, &str)]) -> Response {
    e.app.handle(&Request {
        method: "GET".into(),
        path: path.into(),
        query: query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        headers: [
            ("x-remote-user-id", user.0),
            ("x-remote-user-name", user.1),
            ("x-remote-user-display-name", user.1),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        body: vec![],
        ip: Some("172.30.32.2".into()),
        via_ingress: true,
        tailnet_login: None,
    })
}

#[test]
fn a_person_writes_their_own_guide_and_the_guides_of_their_vaults() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    // Their own, by the short name; and the guide of a vault they own.
    post(
        &e,
        ALEX,
        "guides/set",
        json!({ "scope": "person", "body": "Answer in short sentences." }),
    )?;
    post(
        &e,
        ALEX,
        "guides/set",
        json!({ "scope": "space:alex-me", "body": "This is about me." }),
    )?;
    let r = ha_get(&e, "/admin/api/guide", ALEX, &[("scope", "space:alex-me")]);
    assert_eq!(r.status, 200);
    let g = r.body_json();
    assert_eq!(g["current"]["body"], "This is about me.");
    assert_eq!(g["versions"], 1);
    // A new text keeps the old one in the history.
    post(
        &e,
        ALEX,
        "guides/set",
        json!({ "scope": "space:alex-me", "body": "This is about me, and my habits." }),
    )?;
    let g = ha_get(&e, "/admin/api/guide", ALEX, &[("scope", "space:alex-me")]).body_json();
    assert_eq!(g["versions"], 2);
    // The page knows which vaults have one, and where the person may write one.
    let a = state(&e, ALEX);
    assert_eq!(a["my_guide"], true);
    let v = a["spaces"]
        .as_array()
        .and_then(|l| l.iter().find(|v| v["name"] == "alex-me"))
        .ok_or("vault")?;
    assert_eq!(v["guide"], json!(true));
    // Clearing takes it out of force.
    let c = post(
        &e,
        ALEX,
        "guides/clear",
        json!({ "scope": "space:alex-me" }),
    )?;
    assert_eq!(c["cleared"], true);
    let g = ha_get(&e, "/admin/api/guide", ALEX, &[("scope", "space:alex-me")]).body_json();
    assert!(g["current"].is_null());
    assert_eq!(g["versions"], 2, "nothing is erased");
    Ok(())
}

#[test]
fn nobody_writes_the_guide_of_a_vault_or_a_person_that_is_not_theirs() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, SAM, "spaces", json!({ "kind": "work" }))?;
    let sam = {
        let ctl = e.app.brain.open_control()?;
        users::user_by_name(&ctl, "sam")?.ok_or("sam")?.id
    };
    let set = |who: (&str, &str), scope: &str, body: &str| {
        ha(
            &e,
            "POST",
            "/admin/api/guides/set",
            who,
            json!({ "scope": scope, "body": body }),
        )
    };
    // Alex does not own Sam's vault, and is not Sam.
    assert_eq!(
        set(ALEX, "space:sam-work", "I give orders here.").status,
        403
    );
    assert_eq!(
        set(ALEX, &format!("person:{sam}"), "I speak for Sam.").status,
        403
    );
    assert_eq!(
        ha_get(&e, "/admin/api/guide", ALEX, &[("scope", "space:sam-work")]).status,
        403,
        "nor read it"
    );
    // An administrator may write one for someone else (the operator), recorded as its author.
    assert_eq!(
        set(SAM, "space:alex-work", "x").status,
        400,
        "that vault does not exist"
    );
    post(&e, ALEX, "spaces", json!({ "kind": "work" }))?;
    assert_eq!(
        set(SAM, "space:alex-work", "Set by the operator.").status,
        200
    );
    let g = ha_get(
        &e,
        "/admin/api/guide",
        ALEX,
        &[("scope", "space:alex-work")],
    )
    .body_json();
    assert_eq!(g["current"]["body"], "Set by the operator.");
    // Bad scopes and bad texts.
    assert_eq!(set(ALEX, "world", "x").status, 400);
    assert_eq!(set(ALEX, "space:Bad Name", "x").status, 400);
    assert_eq!(
        set(ALEX, "person", &"y".repeat(20_000)).status,
        200,
        "no size limit"
    );
    assert_eq!(set(ALEX, "person", "bell \u{7}").status, 400);
    assert_eq!(set(ALEX, "person", "   ").status, 400);
    // A key that opens MCP never writes a guide, and neither does a form from another site.
    let ctl = e.app.brain.open_control()?;
    let mcp = access::connect(&ctl, "assistant", &Rights::owner(), None, false, false)?;
    let auth = format!("Bearer {mcp}");
    let r = direct(
        &e,
        "POST",
        "/admin/api/guides/set",
        &[
            ("authorization", auth.as_str()),
            ("x-requested-with", "memcp"),
        ],
        json!({ "scope": "person", "body": "planted" }),
    );
    assert_eq!(r.status, 401);
    let r = ha(
        &e,
        "POST",
        "/admin/api/guides/set",
        ALEX,
        json!({ "scope": "person", "body": "ok" }),
    );
    assert_eq!(r.status, 200);
    let no_csrf = e.app.handle(&Request {
        method: "POST".into(),
        path: "/admin/api/guides/set".into(),
        query: Default::default(),
        headers: [
            ("x-remote-user-id", "ha-alex"),
            ("x-remote-user-name", "Alex"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        body: json!({ "scope": "person", "body": "forged" })
            .to_string()
            .into_bytes(),
        ip: Some("172.30.32.2".into()),
        via_ingress: true,
        tailnet_login: None,
    });
    assert_eq!(no_csrf.status, 403);
    Ok(())
}

const BEN: (&str, &str) = ("ha-ben", "Ben");

#[test]
fn each_person_sees_the_events_of_their_own_connections_and_an_administrator_all() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    join(&e, BEN)?;
    for (who, vault, agent) in [(ALEX, "alex-me", "alex-key"), (BEN, "ben-me", "ben-key")] {
        post(&e, who, "spaces", json!({ "kind": "me" }))?;
        post(
            &e,
            who,
            "connections",
            json!({ "agent": agent, "rights": format!("{vault}:rw") }),
        )?;
        post(&e, who, "connections/revoke", json!({ "agent": agent }))?;
    }
    let who_in = |s: &Value| -> Vec<String> {
        s["activity"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["who"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let alex = who_in(&state(&e, ALEX));
    assert!(alex.contains(&"alex-key".to_string()), "{alex:?}");
    assert!(
        !alex.contains(&"ben-key".to_string()),
        "never another person's events"
    );
    let ben = who_in(&state(&e, BEN));
    assert!(ben.contains(&"ben-key".to_string()) && !ben.contains(&"alex-key".to_string()));
    let all = who_in(&state(&e, SAM));
    assert!(all.contains(&"alex-key".to_string()) && all.contains(&"ben-key".to_string()));
    // An event says what, when and from where; it carries no key.
    let s = state(&e, ALEX);
    let ev = s["activity"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["event"] == "revoke"))
        .ok_or("event")?;
    assert!(ev["at"].is_string() && ev["ip"].is_string());
    assert!(!s["activity"].to_string().contains("hcp_"), "never a key");
    assert!(
        s["activity"].as_array().map(Vec::len) <= Some(30),
        "bounded"
    );
    Ok(())
}

#[test]
fn only_what_deserves_a_look_is_listed_and_refused_keys_are_for_administrators() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    post(
        &e,
        ALEX,
        "connections",
        json!({ "agent": "alex-forgotten", "rights": "alex-me:r" }),
    )?;
    post(
        &e,
        SAM,
        "connections",
        json!({ "agent": "sam-fresh", "rights": "all" }),
    )?;
    post(
        &e,
        SAM,
        "connections",
        json!({ "agent": "sam-never", "rights": "all" }),
    )?;
    post(
        &e,
        SAM,
        "connections",
        json!({ "agent": "sam-silent", "rights": "all" }),
    )?;
    let ctl = e.app.brain.open_control()?;
    let now = memcp_core::util::now();
    ctl.execute(
        "UPDATE connections SET created_at = ?1 WHERE agent IN ('sam-never', 'alex-forgotten')",
        [now - 10 * 86_400],
    )?;
    ctl.execute(
        "UPDATE connections SET created_at = ?1, last_used_at = ?2 WHERE agent = 'sam-silent'",
        [now - 200 * 86_400, now - 90 * 86_400],
    )?;
    ctl.execute(
        "UPDATE connections SET last_used_at = ?1 WHERE agent = 'sam-fresh'",
        [now - 3600],
    )?;
    let idle = |s: &Value| -> Vec<String> {
        s["attention"]["idle"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["agent"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut sam = idle(&state(&e, SAM));
    sam.sort();
    assert_eq!(
        sam,
        ["alex-forgotten", "sam-never", "sam-silent"],
        "not the one in use, nor a new one"
    );
    let s = state(&e, SAM);
    let why = |n: &str| {
        s["attention"]["idle"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["agent"] == n))
            .map(|x| x["why"].clone())
    };
    assert_eq!(why("sam-never"), Some(json!("never used")));
    assert_eq!(why("sam-silent"), Some(json!("silent for a long time")));
    assert_eq!(
        idle(&state(&e, ALEX)),
        ["alex-forgotten"],
        "a member sees only their own"
    );
    // Keys refused in the last day: a number for an administrator, nothing for a member.
    for _ in 0..3 {
        let r = direct(
            &e,
            "POST",
            "/mcp/anyone",
            &[("authorization", "Bearer nope")],
            json!({}),
        );
        assert_eq!(r.status, 401);
    }
    assert_eq!(state(&e, SAM)["attention"]["refused_24h"], 3);
    assert_eq!(state(&e, ALEX)["attention"]["refused_24h"], 0);
    // A revoked connection is no longer listed.
    post(
        &e,
        SAM,
        "connections/revoke",
        json!({ "agent": "sam-never" }),
    )?;
    assert!(!idle(&state(&e, SAM)).contains(&"sam-never".to_string()));
    Ok(())
}

/// The entries of a tar: (path, bytes).
fn untar(tar: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    let mut at = 0;
    while at + 512 <= tar.len() && tar[at..at + 512].iter().any(|b| *b != 0) {
        let h = &tar[at..at + 512];
        let text = |r: std::ops::Range<usize>| {
            String::from_utf8_lossy(&h[r])
                .trim_end_matches('\0')
                .to_string()
        };
        assert_eq!(&h[257..262], b"ustar", "a ustar header");
        let sum: u32 = h
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u32::from(*b)
                }
            })
            .sum();
        assert_eq!(
            u32::from_str_radix(text(148..154).trim(), 8).unwrap_or(0),
            sum,
            "a good checksum"
        );
        let (prefix, name) = (text(345..500), text(0..100));
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let size = usize::from_str_radix(text(124..135).trim(), 8).unwrap_or(0);
        out.push((path, tar[at + 512..at + 512 + size].to_vec()));
        at += 512 + size.div_ceil(512) * 512;
    }
    out
}

#[test]
fn a_person_downloads_their_own_memory_and_nobody_elses() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    post(&e, SAM, "spaces", json!({ "kind": "work" }))?;
    let tool = |who: (&str, &str), space: &str, text: &str| -> Result<(), Box<dyn Error>> {
        let ctl = e.app.brain.open_control()?;
        let u = users::resolve(&ctl, if who == ALEX { "alex" } else { "sam" })?;
        let t = access::connect_for(
            &ctl,
            &u.id,
            &format!("w-{space}"),
            &Rights::parse(&format!("{space}:rw"))?,
            None,
            false,
            false,
        )?;
        let auth = format!("Bearer {t}");
        let r = direct(
            &e,
            "POST",
            &format!("/mcp/w-{space}"),
            &[("authorization", auth.as_str())],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "remember", "arguments": { "space": space, "text": text } } }),
        );
        assert_eq!(r.status, 200);
        Ok(())
    };
    tool(ALEX, "alex-me", "Alex likes long walks by the sea.")?;
    tool(SAM, "sam-work", "SAM-ONLY budget review on Thursday.")?;

    let r = ha_get(&e, "/admin/api/export", ALEX, &[]);
    assert_eq!(r.status, 200);
    let header = |n: &str| {
        r.headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    assert_eq!(header("Content-Type"), "application/x-tar");
    assert!(
        header("Content-Disposition").contains("memcp-export-alex-")
            && header("Content-Disposition").ends_with(".tar\"")
    );
    let entries = untar(&r.bytes());
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.iter().any(|n| n.ends_with("/manifest.json"))
            && names.iter().any(|n| n.ends_with("/spaces/alex-me.sqlite")),
        "{names:?}"
    );
    let manifest: Value = serde_json::from_slice(
        &entries
            .iter()
            .find(|(n, _)| n.ends_with("/manifest.json"))
            .ok_or("manifest")?
            .1,
    )?;
    assert_eq!(manifest["person"]["name"], "alex");
    let md = String::from_utf8_lossy(
        &entries
            .iter()
            .find(|(n, _)| n.ends_with("/readable/alex-me.md"))
            .ok_or("md")?
            .1,
    )
    .to_string();
    assert!(md.contains("Alex likes long walks by the sea."));
    // Nothing of Sam's, in any file, and no key.
    for (n, bytes) in &entries {
        let text = String::from_utf8_lossy(bytes);
        assert!(
            !text.contains("SAM-ONLY") && !text.contains("sam-work") && !text.contains("hcp_"),
            "{n}"
        );
    }
    // A parameter does not change whose memory it is, not even for an administrator.
    let r = ha_get(&e, "/admin/api/export", ALEX, &[("user", "sam")]);
    let m: Value = serde_json::from_slice(
        &untar(&r.bytes())
            .iter()
            .find(|(n, _)| n.ends_with("/manifest.json"))
            .ok_or("manifest")?
            .1,
    )?;
    assert_eq!(m["person"]["name"], "alex");
    let r = ha_get(&e, "/admin/api/export", SAM, &[("user", "alex")]);
    let m: Value = serde_json::from_slice(
        &untar(&r.bytes())
            .iter()
            .find(|(n, _)| n.ends_with("/manifest.json"))
            .ok_or("manifest")?
            .1,
    )?;
    assert_eq!(
        m["person"]["name"], "sam",
        "an administrator exports their own, not someone's else"
    );
    // Not signed in: refused. Someone who owns nothing: a clear message.
    assert_eq!(
        direct(&e, "GET", "/admin/api/export", &[], json!({})).status,
        401
    );
    join(&e, BEN)?;
    let r = ha_get(&e, "/admin/api/export", BEN, &[]);
    assert!(
        r.status >= 400
            && r.body_json()["error"]
                .as_str()
                .is_some_and(|m| m.contains("owns no space")),
        "{}",
        r.body_json()
    );
    // Nothing left behind in the temporary folder.
    let leftovers = std::fs::read_dir(std::env::temp_dir())?
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with("memcp-export-"))
        .count();
    assert_eq!(leftovers, 0, "the working copy is removed");
    Ok(())
}

#[test]
fn a_connection_is_renamed_by_its_person_and_only_by_them() -> Res {
    let e = env()?;
    let t = post(&e, SAM, "connections", json!({ "agent": "other-nm99" }))?;
    assert!(t["token"].as_str().is_some());
    post(
        &e,
        SAM,
        "connections/rename",
        json!({ "agent": "other-nm99", "name": "opencode-mac" }),
    )?;
    let s = state(&e, SAM);
    let names: Vec<&str> = s["connections"]
        .as_array()
        .ok_or("connections")?
        .iter()
        .filter_map(|c| c["agent"].as_str())
        .collect();
    assert!(
        names.contains(&"opencode-mac") && !names.contains(&"other-nm99"),
        "{names:?}"
    );
    // The key is the same.
    let ctl = e.app.brain.open_control()?;
    assert!(
        access::authenticate(&ctl, t["token"].as_str().ok_or("token")?, 1_900_000_000)?.is_some()
    );
    // A name that is not readable by the rules, or already used, or someone else's connection: refused.
    assert!(
        post(
            &e,
            SAM,
            "connections/rename",
            json!({ "agent": "opencode-mac", "name": "Bad Name!" })
        )
        .is_err()
    );
    post(&e, SAM, "connections", json!({ "agent": "second" }))?;
    assert!(
        post(
            &e,
            SAM,
            "connections/rename",
            json!({ "agent": "second", "name": "opencode-mac" })
        )
        .is_err()
    );
    let _ = state(&e, ALEX);
    assert!(
        post(
            &e,
            ALEX,
            "connections/rename",
            json!({ "agent": "second", "name": "mine" })
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn nobody_takes_their_own_administration_away_by_a_stray_click() -> Res {
    let e = env()?;
    let s = state(&e, SAM);
    assert_eq!(s["me"]["admin"], true, "Sam is an administrator here");
    let id = s["me"]["id"].as_str().ok_or("id")?.to_string();
    assert!(post(&e, SAM, "users/update", json!({ "id": id, "admin": false })).is_err());
    assert!(
        post(
            &e,
            SAM,
            "users/update",
            json!({ "id": id, "disabled": true })
        )
        .is_err()
    );
    assert!(post(&e, SAM, "users/unlink", json!({ "id": id })).is_err());
    // Nothing changed: still an administrator.
    assert_eq!(state(&e, SAM)["me"]["admin"], true);
    Ok(())
}

#[test]
fn a_connection_changes_its_vaults_with_the_same_key_and_only_for_its_person() -> Res {
    let e = env()?;
    own(&e, SAM, "sam-notes")?;
    own(&e, SAM, "sam-more")?;
    let t = post(
        &e,
        SAM,
        "connections",
        json!({ "agent": "assistant", "rights": "sam-notes:rw" }),
    )?;
    let token = t["token"].as_str().ok_or("token")?;
    // A vault is added, another cut: the key keeps working and the rights are the new ones.
    post(
        &e,
        SAM,
        "connections/regrant",
        json!({ "agent": "assistant", "rights": "sam-more:r" }),
    )?;
    let ctl = e.app.brain.open_control()?;
    assert!(access::authenticate(&ctl, token, 1_900_000_000)?.is_some());
    let s = state(&e, SAM);
    let c = s["connections"]
        .as_array()
        .ok_or("connections")?
        .iter()
        .find(|c| c["agent"] == "assistant")
        .ok_or("assistant")?;
    assert_eq!(c["rights"], "sam-more:r");
    // Nothing requested, or someone else's connection: refused.
    assert!(
        post(
            &e,
            SAM,
            "connections/regrant",
            json!({ "agent": "assistant", "rights": "" })
        )
        .is_err()
    );
    let _ = state(&e, ALEX);
    assert!(
        post(
            &e,
            ALEX,
            "connections/regrant",
            json!({ "agent": "assistant", "rights": "all" })
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn the_owner_approves_what_an_app_proposed_as_it_reads_and_nobody_else_does() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    const BOB: (&str, &str) = ("ha-bob", "Bob");
    join(&e, BOB)?;
    let c = {
        let ctl = e.app.brain.open_control()?;
        let u = users::resolve(&ctl, "alex")?;
        let t = access::connect_for(
            &ctl,
            &u.id,
            "healthapp",
            &Rights::parse("alex-me/guide:rw")?,
            None,
            false,
            false,
        )?;
        access::authenticate(&ctl, &t, 1_900_000_000)?.ok_or("refused")?
    };
    let propose = |text: &str| -> Res {
        memcp_core::tools::call(&e.app.brain, &c, "guide_propose", &json!({ "text": text }))
            .map_err(|x| format!("{x:?}"))?;
        Ok(())
    };
    propose("Write one record per measure.")?;
    let a = state(&e, ALEX);
    let g = a["app_guides"].as_array().ok_or("list")?;
    assert_eq!(g.len(), 1);
    assert_eq!(g[0]["standing"], "waiting");
    assert_eq!(g[0]["app"], "healthapp");
    let hash = g[0]["hash"].as_str().ok_or("hash")?.to_string();
    let body = |h: &str| json!({ "space": "alex-me", "app": "healthapp", "hash": h });
    let act = |who, path: &str, b: Value| ha(&e, "POST", &format!("/admin/api/{path}"), who, b);
    // Not the owner's, not an administrator's: refused. And a text not read: refused.
    assert_eq!(act(BOB, "app-guides/approve", body(&hash)).status, 403);
    assert_eq!(act(ALEX, "app-guides/approve", body("stale")).status, 400);
    assert_eq!(state(&e, ALEX)["app_guides"][0]["standing"], "waiting");
    assert_eq!(act(ALEX, "app-guides/approve", body(&hash)).status, 200);
    assert_eq!(state(&e, ALEX)["app_guides"][0]["standing"], "approved");
    // The app rewrites: it waits again.
    propose("Something else.")?;
    assert_eq!(state(&e, ALEX)["app_guides"][0]["standing"], "changed");
    assert_eq!(act(BOB, "app-guides/withdraw", body(&hash)).status, 403);
    assert_eq!(act(ALEX, "app-guides/withdraw", body(&hash)).status, 200);
    assert_eq!(state(&e, ALEX)["app_guides"][0]["standing"], "waiting");
    // Bob sees nothing of Alex's.
    assert!(
        state(&e, BOB)["app_guides"]
            .as_array()
            .ok_or("list")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn the_timeline_counts_what_each_vault_received_per_slice() -> Res {
    let e = env()?;
    state(&e, SAM);
    own(&e, SAM, "work")?;
    let conn = e.app.brain.open_space(&sp("work"))?;
    for text in ["first note", "second note"] {
        memcp_core::facts::remember(
            &conn,
            &sp("work"),
            "test",
            memcp_core::facts::NewFact::agent(text, "test"),
            &[],
        )?;
    }
    let get = |q: &str| {
        let mut r = ha(&e, "GET", "/admin/api/timeline", SAM, json!({}));
        if !q.is_empty() {
            r = e.app.handle(&Request {
                method: "GET".into(),
                path: "/admin/api/timeline".into(),
                query: [("hours".to_string(), q.to_string())].into_iter().collect(),
                headers: [
                    ("x-remote-user-id".to_string(), SAM.0.to_string()),
                    ("x-remote-user-name".to_string(), SAM.1.to_string()),
                    ("x-remote-user-display-name".to_string(), SAM.1.to_string()),
                ]
                .into_iter()
                .collect(),
                body: vec![],
                ip: Some("172.30.32.2".into()),
                via_ingress: true,
                tailnet_login: None,
            });
        }
        assert_eq!(r.status, 200, "{}", r.body_json());
        r.body_json()
    };
    let t = get("");
    assert_eq!(t["hours"], 24);
    let work = t["spaces"]
        .as_array()
        .ok_or("spaces")?
        .iter()
        .find(|v| v["name"] == "work")
        .ok_or("work vault")?;
    let counts = work["counts"].as_array().ok_or("counts")?;
    assert_eq!(counts.len(), 48, "forty-eight slices");
    let total: i64 = counts.iter().filter_map(Value::as_i64).sum();
    assert_eq!(total, 2, "both notes, in the last slices");
    // The scale is bounded, whatever is asked.
    assert_eq!(get("100000")["hours"], 720);
    assert_eq!(get("0")["hours"], 1);
    assert_eq!(get("1")["bucket_secs"], 75);
    Ok(())
}

#[test]
fn starting_a_memory_makes_the_default_spaces_once_and_names_the_person() -> Res {
    let e = env()?;
    let _ = state(&e, SAM);
    let r = ha(
        &e,
        "POST",
        "/admin/api/start",
        SAM,
        json!({ "name": "Sam Q" }),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    for s in ["personal", "health", "home", "finances", "hobbies", "work"] {
        assert!(e.app.brain.has_space(&sp(s))?, "{s}");
    }
    // Again: nothing is made twice, nothing fails.
    let again = ha(
        &e,
        "POST",
        "/admin/api/start",
        SAM,
        json!({ "name": "Sam Q" }),
    );
    assert_eq!(again.status, 200);
    assert_eq!(again.body_json()["made"], json!([]));
    assert_eq!(state(&e, SAM)["me"]["display"], json!("Sam Q"));
    Ok(())
}

#[test]
fn the_page_reads_the_live_peers_and_the_nodes_found() -> Res {
    let e = env()?;
    let _ = state(&e, SAM);
    let r = ha(&e, "GET", "/admin/api/sync/status", SAM, json!({}));
    assert_eq!(r.status, 200);
    assert!(r.body_json()["peers"].is_array());
    let d = ha(&e, "GET", "/admin/api/discover", SAM, json!({}));
    assert_eq!(d.status, 200);
    assert!(d.body_json()["nodes"].is_array());
    // A join to a bad address is refused with words, and nothing is started.
    let j = ha(
        &e,
        "POST",
        "/admin/api/join-memory",
        SAM,
        json!({ "address": "not an address" }),
    );
    assert_eq!(j.status, 400);
    assert!(state(&e, SAM)["node"]["version"].is_string());
    Ok(())
}

#[test]
fn an_internal_connection_is_kept_apart_and_only_its_owner_marks_it() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    join(&e, BEN)?;
    post(&e, ALEX, "spaces", json!({ "kind": "me" }))?;
    post(
        &e,
        ALEX,
        "connections",
        json!({ "agent": "alex-tool", "rights": "alex-me:rw" }),
    )?;
    let listed = |s: &Value, internal: bool| -> bool {
        s["connections"].as_array().is_some_and(|a| {
            a.iter()
                .any(|c| c["agent"] == "alex-tool" && c["internal"] == internal)
        })
    };
    assert!(listed(&state(&e, ALEX), false));
    let other = ha(
        &e,
        "POST",
        "/admin/api/connections/internal",
        BEN,
        json!({ "agent": "alex-tool", "internal": true }),
    );
    assert_eq!(other.status, 403, "not someone else's connection");
    post(
        &e,
        ALEX,
        "connections/internal",
        json!({ "agent": "alex-tool", "internal": true }),
    )?;
    assert!(listed(&state(&e, ALEX), true), "flagged internal");
    post(
        &e,
        ALEX,
        "connections/internal",
        json!({ "agent": "alex-tool", "internal": false }),
    )?;
    assert!(listed(&state(&e, ALEX), false));
    Ok(())
}

#[test]
fn a_member_cannot_make_the_node_join_another_memory() -> Res {
    let e = env()?;
    join(&e, ALEX)?;
    let r = ha(
        &e,
        "POST",
        "/admin/api/join-memory",
        ALEX,
        json!({ "address": "https://elsewhere.example.net" }),
    );
    assert_eq!(r.status, 403, "{}", r.body_json());
    // An administrator is not refused for rights (the program that joins may be missing here: another answer).
    let r = ha(
        &e,
        "POST",
        "/admin/api/join-memory",
        SAM,
        json!({ "address": "not an address" }),
    );
    assert_eq!(r.status, 400);
    Ok(())
}
