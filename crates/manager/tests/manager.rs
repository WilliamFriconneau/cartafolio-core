//! The manager end to end, without a network: people created by Home Assistant logins, a key
//! that reaches only its own person, OAuth consent by tailnet login, and the conversion of an
//! installation that held several people in one directory.

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine as _;
use pensine_core::access::{self, Rights};
use pensine_core::text::Language;
use pensine_core::users;
use pensine_manager::{Manager, Options};
use pensine_server::http::{Request, Response, parse_query};
use pensine_server::net::Handler;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);
const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "a-long-random-verifier-string-of-at-least-43-characters-0123456789";

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let p =
            std::env::temp_dir().join(format!("pensine-manager-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        Dir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn options() -> Options {
    let mut tailnet_users = HashMap::new();
    tailnet_users.insert("alice@example.org".to_string(), "alice".to_string());
    tailnet_users.insert("lou@example.org".to_string(), "lou".to_string());
    Options {
        public_url: Some("https://memory.example.net".into()),
        login_url: Some("https://memory.example.net:8792".into()),
        tailnet_users,
        language: Some(Language::English),
        ..Default::default()
    }
}

fn rq(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    Request {
        method: method.into(),
        path: path.into(),
        query: parse_query(query),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            .collect(),
        body: body.as_bytes().to_vec(),
        ip: Some("203.0.113.9".into()),
        ..Default::default()
    }
}

/// A request through Home Assistant's ingress, as the trusted proxy forwards it.
fn ingress(id: &str, name: &str, method: &str, path: &str, body: &str) -> Request {
    let mut r = rq(
        method,
        path,
        &[
            ("x-remote-user-id", id),
            ("x-remote-user-name", name),
            ("x-remote-user-display-name", name),
            ("x-requested-with", "memcp"),
            ("content-type", "application/json"),
        ],
        body,
    );
    r.via_ingress = true;
    r
}

fn tailnet(login: &str, method: &str, path: &str, body: &str) -> Request {
    let mut r = rq(method, path, &[], body);
    r.tailnet_login = Some(login.into());
    r
}

thread_local! {
    /// Which connection each key of this test belongs to: its address is `/mcp/<name>`.
    static ADDRESSES: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn note(agent: &str, token: &str) {
    ADDRESSES.with(|m| m.borrow_mut().insert(token.to_string(), agent.to_string()));
}

fn mcp(token: &str, tool: &str, args: Value) -> Request {
    let agent = ADDRESSES
        .with(|m| m.borrow().get(token).cloned())
        .unwrap_or_else(|| "nobody".into());
    rq(
        "POST",
        &format!("/mcp/{agent}"),
        &[
            ("authorization", &format!("Bearer {token}")),
            ("content-type", "application/json"),
        ],
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } })
            .to_string(),
    )
}

fn tool_text(r: &Response) -> Value {
    let v = r.body_json();
    v["result"]["content"][0]["text"]
        .as_str()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(v)
}

fn key(m: &Manager, person: &str, agent: &str, rights: &str) -> Result<String, Box<dyn Error>> {
    let app = m.person_app(person).ok_or("no such person")?;
    let ctl = app.brain.open_control()?;
    let token = (if agent.starts_with("peer-") {
        access::connect_peer(&ctl, users::OWNER, agent, &Rights::parse(rights)?, false)
    } else {
        access::connect_for(
            &ctl,
            users::OWNER,
            agent,
            &Rights::parse(rights)?,
            None,
            false,
            false,
        )
    })?;
    note(agent, &token);
    Ok(token)
}

fn two_people(d: &Dir) -> Result<Manager, Box<dyn Error>> {
    let m = Manager::open(d.path(), options()).map_err(|e| e.1)?;
    let ctl = m.registry().brain.open_control()?;
    let _ = users::from_home_assistant(&ctl, "ha-w", "alice", "Alice", &[])?;
    m.ensure_people().map_err(|e| e.1)?;
    m.add_person("lou", "Lou", Some("ha-l"), false)
        .map_err(|e| e.1)?;
    Ok(m)
}

#[test]
fn discovery_aliases_match_on_the_manager() -> Res {
    let d = Dir::new("discovery-alias");
    let m = Manager::open(d.path(), options()).map_err(|e| e.1)?;
    let new = m.handle(&rq("GET", "/.well-known/pensine", &[], ""));
    assert_eq!(new.status, 200);
    for path in ["/.well-known/silice", "/.well-known/memcp"] {
        let alias = m.handle(&rq("GET", path, &[], ""));
        assert_eq!(alias.status, new.status);
        assert_eq!(alias.headers, new.headers);
        assert_eq!(alias.body, new.body);
    }
    assert_eq!(new.body_json()["product"], "pensine");
    assert_eq!(new.body_json()["service"], "memcp");
    Ok(())
}

#[test]
fn the_first_login_becomes_the_administrator_and_gets_a_directory_of_their_own() -> Res {
    let d = Dir::new("first");
    let m = Manager::open(d.path(), options()).map_err(|e| e.1)?;
    let r = m.handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""));
    assert_eq!(r.status, 200, "{}", r.body_json());
    let s = r.body_json();
    assert_eq!(s["installation"]["admin"], true, "{s}");
    assert!(
        s["installation"]["users"]
            .as_array()
            .is_some_and(|u| u.iter().any(|x| x["name"] == "alice" && x["admin"] == true)),
        "{s}"
    );
    assert!(d.path().join("people/alice/control.sqlite").exists());
    // The placeholder administrator of a fresh registry became Alice: nobody else, no stray directory.
    assert!(!d.path().join("people/owner").exists());
    assert_eq!(
        s["installation"]["users"].as_array().map(Vec::len),
        Some(1),
        "{s}"
    );
    // Inside their own directory the person is a plain member: no page of another person's, no
    // people to manage there.
    assert_eq!(s["me"]["admin"], false, "{s}");
    assert_eq!(s["spaces"], json!([]), "no vault until they make one");
    Ok(())
}

#[test]
fn a_second_login_waits_and_the_administrator_lets_it_in() -> Res {
    let d = Dir::new("second");
    let m = Manager::open(d.path(), options()).map_err(|e| e.1)?;
    m.handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""));
    // An unknown login waits.
    let wait = m.handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""));
    assert_eq!(wait.status, 403, "{}", wait.body_json());
    assert!(
        !d.path().join("people/lou").exists(),
        "nothing is made for a stranger"
    );
    // The administrator sees it, through the same page, and approves.
    let st = m.handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""));
    let reqs = st.body_json()["installation"]["requests"].clone();
    assert!(
        reqs.as_array()
            .is_some_and(|r| r.iter().any(|x| x["ha_user"] == "ha-l")),
        "{reqs}"
    );
    let ok = m.handle(&ingress(
        "ha-w",
        "alice",
        "POST",
        "/admin/api/requests/approve",
        &json!({ "ha_user": "ha-l" }).to_string(),
    ));
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    assert!(d.path().join("people/lou/control.sqlite").exists());
    // Lou is in, in a directory of their own, and sees nothing of the installation.
    let lou = m.handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""));
    assert_eq!(lou.status, 200, "{}", lou.body_json());
    assert!(lou.body_json().get("installation").is_none());
    let denied = m.handle(&ingress(
        "ha-l",
        "lou",
        "POST",
        "/admin/api/users",
        &json!({ "name": "eve" }).to_string(),
    ));
    assert_eq!(denied.status, 403);
    Ok(())
}

#[test]
fn a_key_reaches_its_own_person_and_nobody_else() -> Res {
    let d = Dir::new("keys");
    let m = two_people(&d)?;
    for p in ["alice", "lou"] {
        let app = m.person_app(p).ok_or("person")?;
        app.brain.add_vault(users::OWNER, p, "health")?;
    }
    let kw = key(&m, "alice", "w-claude", "all")?;
    let kl = key(&m, "lou", "l-claude", "all")?;
    let w = tool_text(&m.handle(&mcp(
        &kw,
        "remember",
        json!({ "text": "Alice's knee clicks on stairs" }),
    )));
    assert_eq!(w["result"], "created", "{w}");
    // Lou's key finds nothing of Alice's, and sees only Lou's vault.
    let found = tool_text(&m.handle(&mcp(&kl, "recall", json!({ "query": "knee stairs" }))));
    assert_eq!(found["results"], json!([]), "{found}");
    let stats = tool_text(&m.handle(&mcp(&kl, "stats", json!({}))));
    let names: Vec<&str> = stats
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_eq!(names.len(), 1, "{stats}");
    assert!(names[0].starts_with("lou"), "{names:?}");
    // An unknown key is refused like anywhere.
    let none = m.handle(&mcp("hcp_unknown_token", "stats", json!({})));
    assert_eq!(none.status, 401);
    Ok(())
}

fn challenge() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

fn authorize_path(client: &str) -> String {
    format!(
        "/oauth/authorize?response_type=code&client_id={client}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state=xyz",
        "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback",
        challenge()
    )
}

#[test]
fn claude_on_the_web_signs_in_as_the_person_the_tailnet_says_it_is() -> Res {
    let d = Dir::new("oauth");
    let m = two_people(&d)?;
    for p in ["alice", "lou"] {
        let app = m.person_app(p).ok_or("person")?;
        app.brain.add_vault(users::OWNER, p, "me")?;
    }
    // The client registers once, at the manager.
    let reg = m.handle(&rq(
        "POST",
        "/oauth/register",
        &[],
        &json!({ "client_name": "Claude", "redirect_uris": [CALLBACK] }).to_string(),
    ));
    assert_eq!(reg.status, 201, "{}", reg.body_json());
    let client = reg.body_json()["client_id"]
        .as_str()
        .ok_or("client")?
        .to_string();
    let q = authorize_path(&client);
    // A tailnet account gets the invitation form. The invitation selects Lou even from Alice’s browser.
    let form_page = m.handle(&tailnet("lou@example.org", "GET", &q, ""));
    assert_eq!(form_page.status, 200);
    assert!(String::from_utf8_lossy(&form_page.body).contains("invitation code"));
    let made = m.handle(&ingress(
        "ha-w",
        "alice",
        "POST",
        "/admin/api/users/invite",
        r#"{"name":"lou"}"#,
    ));
    let invite = made.body_json()["code"].as_str().ok_or("code")?.to_string();
    let with_code = q
        .split_once('?')
        .map(|(_, b)| format!("{b}&invite={invite}"))
        .ok_or("form")?;
    let post = |body: &str, origin: &str| {
        let mut request = tailnet("alice@example.org", "POST", "/oauth/authorize", body);
        request.headers.insert("origin".into(), origin.into());
        request
            .headers
            .insert("sec-fetch-site".into(), "same-origin".into());
        m.handle(&request)
    };
    assert_eq!(post(&with_code, "https://memory.example.net").status, 403);
    let page = post(&with_code, "https://memory.example.net:8792");
    assert_eq!(page.status, 200, "{}", String::from_utf8_lossy(&page.body));
    let html = String::from_utf8(page.body.clone())?;
    assert!(
        html.contains("lou-me") && !html.contains("alice-me"),
        "{html}"
    );
    let nonce = html
        .split("name=nonce value=")
        .nth(1)
        .and_then(|r| r.split('>').next())
        .ok_or("nonce")?
        .to_string();
    let form = q
        .split_once('?')
        .map(|(_, b)| format!("{b}&decision=allow&nonce={nonce}&v_lou-me=1"))
        .ok_or("form")?;
    let done = post(&form, "https://memory.example.net:8792");
    assert_eq!(done.status, 302, "{}", String::from_utf8_lossy(&done.body));
    let loc = done
        .headers
        .iter()
        .find(|(k, _)| k == "Location")
        .map(|(_, v)| v.clone())
        .ok_or("location")?;
    let code = loc
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?;
    // The token request comes from the internet, with no identity: the code says whose it is.
    let tok = m.handle(&rq(
        "POST",
        "/oauth/token",
        &[("content-type", "application/x-www-form-urlencoded")],
        &format!("grant_type=authorization_code&code={code}&redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback&client_id={client}&code_verifier={VERIFIER}"),
    ));
    assert_eq!(tok.status, 200, "{}", tok.body_json());
    let access = tok.body_json()["access_token"]
        .as_str()
        .ok_or("token")?
        .to_string();
    for person in ["alice", "lou"] {
        let app = m.person_app(person).ok_or("app")?;
        let ctl = app.brain.open_control()?;
        if let Some(who) = access::authenticate(&ctl, &access, pensine_core::util::now())? {
            note(&who.agent, &access);
        }
    }
    let stats = tool_text(&m.handle(&mcp(&access, "stats", json!({}))));
    let names: Vec<&str> = stats
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_eq!(names, vec!["lou-me"], "{stats}");
    // The refresh token finds its person too.
    let refresh = tok.body_json()["refresh_token"]
        .as_str()
        .ok_or("refresh")?
        .to_string();
    let again = m.handle(&rq(
        "POST",
        "/oauth/token",
        &[("content-type", "application/x-www-form-urlencoded")],
        &format!("grant_type=refresh_token&refresh_token={refresh}&client_id={client}"),
    ));
    assert_eq!(again.status, 200, "{}", again.body_json());
    Ok(())
}

#[test]
fn an_unknown_tailnet_login_is_given_nothing() -> Res {
    let d = Dir::new("stranger");
    let m = two_people(&d)?;
    let r = m.handle(&tailnet("eve@example.org", "GET", "/admin/api/state", ""));
    assert_eq!(r.status, 401, "{}", r.body_json());
    Ok(())
}

#[test]
fn a_path_limited_key_reaches_its_person_through_the_manager_and_stays_limited() -> Res {
    let d = Dir::new("path");
    let m = two_people(&d)?;
    let app = m.person_app("alice").ok_or("person")?;
    let health = app.brain.add_vault(users::OWNER, "alice", "health")?;
    let k = key(&m, "alice", "bevel", &format!("{health}/sources/bevel:rw"))?;
    let r = tool_text(&m.handle(&mcp(
        &k,
        "remember",
        json!({ "text": "Steps 2026-11-01: 9100", "origin": { "app": "bevel", "id": "s" } }),
    )));
    assert_eq!(r["result"], "created", "{r}");
    let outside = m.handle(&mcp(
        &k,
        "remember",
        json!({ "text": "Outside its path", "tags": ["conditions"] }),
    ));
    assert_eq!(outside.body_json()["result"]["isError"], true);
    Ok(())
}

#[test]
fn the_administrator_sees_each_person_with_their_connections_and_can_unlink_an_account() -> Res {
    let d = Dir::new("people");
    let m = two_people(&d)?;
    let app = m.person_app("lou").ok_or("lou")?;
    app.brain.add_vault(users::OWNER, "lou", "me")?;
    key(&m, "lou", "lou-claude", "all")?;
    key(&m, "lou", "lou-code", "all")?;
    let st = m.handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""));
    let people = st.body_json()["installation"]["users"].clone();
    let lou = people
        .as_array()
        .and_then(|p| p.iter().find(|x| x["name"] == "lou"))
        .ok_or("lou listed")?
        .clone();
    assert_eq!(lou["connections"], 2, "{lou}");
    assert_eq!(lou["spaces"], 1, "{lou}");
    assert_eq!(lou["home_assistant"], true);
    // Unlinking cuts the account, not the memory.
    let r = m.handle(&ingress(
        "ha-w",
        "alice",
        "POST",
        "/admin/api/users/unlink",
        &json!({ "id": lou["id"] }).to_string(),
    ));
    assert_eq!(r.status, 200, "{}", r.body_json());
    let after = m.handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""));
    let lou = after.body_json()["installation"]["users"]
        .as_array()
        .and_then(|p| p.iter().find(|x| x["name"] == "lou").cloned())
        .ok_or("lou still listed")?;
    assert_eq!(lou["home_assistant"], false);
    assert_eq!(lou["spaces"], 1, "the memory stays");
    // A member cannot do it.
    let denied = m.handle(&ingress(
        "ha-l",
        "lou",
        "POST",
        "/admin/api/users/unlink",
        &json!({ "id": lou["id"] }).to_string(),
    ));
    assert_eq!(denied.status, 403);
    Ok(())
}

#[test]
fn an_administrator_named_in_the_options_gets_their_person_back_with_a_new_account() -> Res {
    let d = Dir::new("relink");
    let mut o = options();
    o.admins = vec!["alice".into()];
    let m = Manager::open(d.path(), o).map_err(|e| e.1)?;
    let first = m.handle(&ingress("ha-old", "alice", "GET", "/admin/api/state", ""));
    assert_eq!(first.status, 200, "{}", first.body_json());
    // The account is deleted and made again: another identifier, the same login name.
    let again = m.handle(&ingress("ha-new", "alice", "GET", "/admin/api/state", ""));
    assert_eq!(again.status, 200, "{}", again.body_json());
    let reg = m.registry().brain.open_control()?;
    let all = users::list_users(&reg)?;
    let alices: Vec<_> = all.iter().filter(|u| u.name.starts_with("alice")).collect();
    assert_eq!(alices.len(), 1, "no second Alice: {all:?}");
    assert_eq!(alices[0].ha_user.as_deref(), Some("ha-new"));
    Ok(())
}

#[test]
fn a_recovery_key_brings_a_person_back_with_a_new_account() -> Res {
    let d = Dir::new("recovery");
    let m = two_people(&d)?;
    // Lou makes a key on their page; the page tells them, once.
    let made = m.handle(&ingress(
        "ha-l",
        "lou",
        "POST",
        "/admin/api/recovery/make",
        "{}",
    ));
    assert_eq!(made.status, 200, "{}", made.body_json());
    let key = made.body_json()["key"].as_str().ok_or("key")?.to_string();
    let st = m
        .handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""))
        .body_json();
    assert_eq!(st["safety"]["has_key"], true, "{st}");
    // Their account is deleted and made again: another identifier. It waits... and holds the key.
    let wait = m.handle(&ingress("ha-l2", "lou", "GET", "/admin/api/state", ""));
    assert_eq!(wait.status, 403);
    let wrong = m.handle(&ingress(
        "ha-l2",
        "lou",
        "POST",
        "/admin/api/recover",
        &json!({ "key": "AAAA-AAAA-AAAA-AAAA-AAAA" }).to_string(),
    ));
    assert_eq!(wrong.status, 400);
    let ok = m.handle(&ingress(
        "ha-l2",
        "lou",
        "POST",
        "/admin/api/recover",
        &json!({ "key": key }).to_string(),
    ));
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    let back = m.handle(&ingress("ha-l2", "lou", "GET", "/admin/api/state", ""));
    assert_eq!(back.status, 200, "{}", back.body_json());
    // The key is used up, and the old account no longer opens it.
    let again = m.handle(&ingress(
        "ha-x",
        "x",
        "POST",
        "/admin/api/recover",
        &json!({ "key": key }).to_string(),
    ));
    assert_eq!(again.status, 400);
    assert_eq!(
        m.handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""))
            .status,
        403
    );
    Ok(())
}

#[test]
fn an_heir_attaches_a_new_account_to_a_person_who_has_been_gone_long_enough() -> Res {
    let d = Dir::new("heir");
    let m = two_people(&d)?;
    let reg = m.registry().brain.open_control()?;
    let lou = users::user_by_name(&reg, "lou")?.ok_or("lou")?;
    let will = users::user_by_name(&reg, "alice")?.ok_or("alice")?;
    // Alice names Lou as heir.
    let set = m.handle(&ingress(
        "ha-w",
        "alice",
        "POST",
        "/admin/api/heir/set",
        &json!({ "heir": lou.id }).to_string(),
    ));
    assert_eq!(set.status, 200, "{}", set.body_json());
    // Alice's account is gone: a new one waits. Lou sees it, but too early.
    m.handle(&ingress("ha-w2", "alice", "GET", "/admin/api/state", ""));
    let st = m
        .handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""))
        .body_json();
    assert_eq!(st["safety"]["heir_of"][0]["name"], "alice", "{st}");
    assert_eq!(st["safety"]["heir_of"][0]["eligible"], false);
    let early = m.handle(&ingress(
        "ha-l",
        "lou",
        "POST",
        "/admin/api/requests/attach",
        &json!({ "ha_user": "ha-w2", "person": will.id }).to_string(),
    ));
    assert_eq!(early.status, 400, "too early");
    // Thirty days later (Alice was last seen long ago).
    reg.execute(
        "UPDATE users SET last_seen = last_seen - ?1, created_at = created_at - ?1 WHERE id = ?2",
        rusqlite::params![40 * 86_400, will.id],
    )?;
    let st = m
        .handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""))
        .body_json();
    assert_eq!(st["safety"]["heir_of"][0]["eligible"], true, "{st}");
    assert_eq!(st["safety"]["waiting"][0]["ha_user"], "ha-w2", "{st}");
    // Someone who is not the heir cannot.
    let not_heir = m.handle(&ingress(
        "ha-w2",
        "alice",
        "POST",
        "/admin/api/requests/attach",
        &json!({ "ha_user": "ha-w2", "person": will.id }).to_string(),
    ));
    assert_eq!(not_heir.status, 403, "an account that waits can do nothing");
    let ok = m.handle(&ingress(
        "ha-l",
        "lou",
        "POST",
        "/admin/api/requests/attach",
        &json!({ "ha_user": "ha-w2", "person": will.id }).to_string(),
    ));
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    let back = m.handle(&ingress("ha-w2", "alice", "GET", "/admin/api/state", ""));
    assert_eq!(back.status, 200, "{}", back.body_json());
    Ok(())
}

#[test]
fn by_default_every_new_home_assistant_account_gets_a_memory_of_its_own() -> Res {
    let d = Dir::new("auto");
    let mut o = options();
    o.auto_create_people = true;
    let m = Manager::open(d.path(), o).map_err(|e| e.1)?;
    let first = m.handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""));
    assert_eq!(first.body_json()["installation"]["admin"], true);
    // Another account opens it: no waiting, a member with a directory of their own.
    let lou = m.handle(&ingress("ha-l", "lou", "GET", "/admin/api/state", ""));
    assert_eq!(lou.status, 200, "{}", lou.body_json());
    assert!(
        lou.body_json().get("installation").is_none(),
        "not an administrator"
    );
    assert!(d.path().join("people/lou/control.sqlite").exists());
    // The administrator sees them, and nobody waits.
    let st = m
        .handle(&ingress("ha-w", "alice", "GET", "/admin/api/state", ""))
        .body_json();
    let names: Vec<&str> = st["installation"]["users"]
        .as_array()
        .ok_or("users")?
        .iter()
        .filter_map(|u| u["name"].as_str())
        .collect();
    assert!(names.contains(&"lou"), "{names:?}");
    assert_eq!(st["installation"]["requests"], json!([]));
    Ok(())
}

#[test]
fn a_device_pairs_as_a_peer_of_its_person_and_never_reaches_another() -> Res {
    let d = Dir::new("pairing");
    let m = two_people(&d)?;
    for p in ["alice", "lou"] {
        let app = m.person_app(p).ok_or("person")?;
        app.brain.add_vault(users::OWNER, p, "health")?;
    }
    // Lou asks her page for a code: no vault to choose, everything goes to her Inbox.
    let r = m.handle(&ingress("ha-l", "lou", "POST", "/admin/api/pairing", "{}"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let made = r.body_json();
    let code = made["code"].as_str().ok_or("code")?.to_string();
    assert_eq!(made["server"], "https://memory.example.net");
    assert_eq!(made["triage"], "lou-inbox");
    // The device, with no key, exchanges it on the public listener.
    let pair = |code: &str| {
        m.handle(&rq(
            "POST",
            "/pair",
            &[("content-type", "application/json")],
            &json!({ "code": code, "device": "Lou's Mac" }).to_string(),
        ))
    };
    let got = pair(&code.to_lowercase());
    assert_eq!(got.status, 200, "{}", String::from_utf8_lossy(&got.body));
    let got = got.body_json();
    let key = got["key"].as_str().ok_or("key")?;
    let agent = got["agent"].as_str().ok_or("agent")?;
    assert!(agent.starts_with("peer-lou-s-mac-"), "{agent}");
    assert_eq!(
        got["mcp_url"],
        format!("https://memory.example.net/mcp/{agent}")
    );
    let vaults = got["spaces"].as_array().ok_or("spaces")?;
    assert!(
        vaults
            .iter()
            .any(|v| v.as_str().unwrap_or("").starts_with("lou-inbox")),
        "{vaults:?}"
    );
    assert!(
        vaults
            .iter()
            .all(|v| !v.as_str().unwrap_or("").starts_with("alice-")),
        "{vaults:?}"
    );
    // A peer: it writes into Lou's Inbox without naming it, reads Lou's spaces, and never reaches Alice.
    let mut req = mcp(key, "remember", json!({ "text": "Une note du Mac." }));
    req.path = format!("/mcp/{agent}");
    let w = tool_text(&m.handle(&req));
    assert_eq!(w["result"], "created", "{w}");
    assert_eq!(w["fact"]["space"], "lou-inbox", "{w}");
    let mut stats = mcp(key, "stats", json!({}));
    stats.path = format!("/mcp/{agent}");
    let s = tool_text(&m.handle(&stats));
    assert!(
        s["lou-inbox"]["kind"] == "inbox" && s.get("alice-health").is_none(),
        "{s}"
    );
    // Once only; a wrong code is refused the same way, and an address that keeps guessing is held off.
    assert_eq!(pair(&code).status, 400);
    for _ in 0..12 {
        let _ = pair("ZZZZ-ZZZZ");
    }
    assert_eq!(pair("ZZZZ-ZZZZ").status, 429);
    Ok(())
}

#[test]
fn each_memory_of_the_installation_goes_by_its_name_and_says_how_it_is_reached() -> Res {
    let d = Dir::new("directory-name");
    let mut opts = options();
    opts.node_name = Some("Home Assistant".into());
    opts.listen = Some("0.0.0.0:8791".into());
    let m = Manager::open(d.path(), opts).map_err(|e| e.1)?;
    m.add_person("lou", "Lou", None, false).map_err(|e| e.1)?;
    let app = m.person_app("lou").ok_or("no app for lou")?;
    let node = pensine_core::directory::node_json(&app.brain, 0)?;
    assert_eq!(node["name"], "Home Assistant");
    let kinds: Vec<&str> = node["exposures"]
        .as_array()
        .ok_or("no exposures")?
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"lan"), "{kinds:?}");
    Ok(())
}

#[test]
fn a_peer_of_a_person_who_is_not_the_first_administrator_invites_a_device() -> Res {
    let d = Dir::new("invite-second");
    let m = two_people(&d)?;
    // Lou is not the first administrator; a peer of Lou's asks for a ticket.
    let peer = key(&m, "lou", "peer-lou-mac-0001", "all")?;
    let auth = format!("Bearer {peer}");
    let r = m.handle(&rq(
        "POST",
        "/mcp/lou/peer-lou-mac-0001/join/invite",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
        ],
        "{}",
    ));
    assert_eq!(r.status, 200, "{}", r.body_json());
    let ticket = r.body_json()["ticket"]
        .as_str()
        .ok_or("ticket")?
        .to_string();
    // The new device asks at the root with the ticket: it reaches Lou's installation, not Alice's.
    let asked = m.handle(&rq(
        "POST",
        "/join",
        &[("content-type", "application/json")],
        &json!({ "device": "Lou's iPhone", "ticket": ticket }).to_string(),
    ));
    assert_eq!(asked.status, 200, "{}", asked.body_json());
    let id = asked.body_json()["id"].as_str().ok_or("id")?.to_string();
    let secret = asked.body_json()["secret"]
        .as_str()
        .ok_or("secret")?
        .to_string();
    let polled = m.handle(&rq(
        "GET",
        &format!("/join/{id}"),
        &[("x-join-secret", &secret)],
        "",
    ));
    assert_eq!(polled.status, 200, "{}", polled.body_json());
    assert_eq!(
        polled.body_json()["state"],
        "approved",
        "{}",
        polled.body_json()
    );
    let agent = polled.body_json()["join"]["agent"]
        .as_str()
        .ok_or("agent")?
        .to_string();
    let lou = m.person_app("lou").ok_or("lou")?;
    let ctl = lou.brain.open_control()?;
    assert!(
        access::list(&ctl)?.iter().any(|c| c.agent == agent),
        "the device joined Lou's memory"
    );
    Ok(())
}

#[test]
fn a_peer_sees_a_device_asking_to_join_and_allows_it() -> Res {
    let d = Dir::new("peer-allows");
    let m = two_people(&d)?;
    let peer = key(&m, "alice", "peer-alice-mac-0001", "all")?;
    let auth = format!("Bearer {peer}");
    let asked = m.handle(&rq(
        "POST",
        "/join",
        &[("content-type", "application/json")],
        &json!({ "device": "Alice's iPhone" }).to_string(),
    ));
    assert_eq!(asked.status, 200, "{}", asked.body_json());
    let id = asked.body_json()["id"].as_str().ok_or("id")?.to_string();
    let secret = asked.body_json()["secret"]
        .as_str()
        .ok_or("secret")?
        .to_string();
    // The Mac, a peer of Alice, sees the request with its three words.
    let pending = m.handle(&rq(
        "GET",
        "/mcp/alice/peer-alice-mac-0001/join/pending",
        &[("authorization", &auth)],
        "",
    ));
    assert_eq!(pending.status, 200, "{}", pending.body_json());
    assert!(
        pending.body_json()["requests"]
            .as_array()
            .ok_or("list")?
            .iter()
            .any(|r| r["id"] == id.as_str())
    );
    // It allows it; the device gets a peer key of its own.
    let ok = m.handle(&rq(
        "POST",
        "/mcp/alice/peer-alice-mac-0001/join/approve",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
        ],
        &json!({ "id": id }).to_string(),
    ));
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    let polled = m.handle(&rq(
        "GET",
        &format!("/join/{id}"),
        &[("x-join-secret", &secret)],
        "",
    ));
    assert_eq!(
        polled.body_json()["state"],
        "approved",
        "{}",
        polled.body_json()
    );
    // An assistant's key (not a peer) is refused.
    let assistant = key(&m, "alice", "claude-test", "all")?;
    let refused = m.handle(&rq(
        "GET",
        "/mcp/alice/claude-test/join/pending",
        &[("authorization", &format!("Bearer {assistant}"))],
        "",
    ));
    assert_eq!(refused.status, 403);
    Ok(())
}

/// What a crash cut off in a person's memory is repaired when the manager opens it: a move of notes left half-done is
/// finished, and a file whose event the peers' log lacks gets it.
#[test]
fn the_manager_repairs_what_a_crash_cut_off_in_a_persons_memory() -> Res {
    use pensine_core::facts::{self, NewFact};
    use pensine_core::moves::{self, Step};
    use pensine_core::{Brain, Space, files};
    let d = Dir::new("recover");
    drop(two_people(&d)?);
    let b = Brain::open(&d.path().join("people/alice"))?;
    let owner: String =
        b.open_control()?
            .query_row("SELECT id FROM users ORDER BY id LIMIT 1", [], |r| r.get(0))?;
    let (from, to) = (Space::new("notes")?, Space::new("cars")?);
    b.add_space_for(&from, &owner)?;
    b.add_space_for(&to, &owner)?;
    let conn = b.open_space(&from)?;
    let mut n = NewFact::agent("2026-11-19, the car is serviced in May.", "agent:t");
    n.tags = vec!["car".into()];
    facts::remember(&conn, &from, "me", n, &[])?;
    moves::fail_at(Some(Step::AfterCopies));
    let cut = b.move_by_tag(&from, &to, "car", "me", false);
    moves::fail_at(None);
    assert!(cut.is_err(), "the move went through its failpoint");
    // A record an earlier version wrote without its event (the log is append-only: written as that version did).
    let bytes = b"a short text";
    let blobs = b.dir().join("files").join("notes");
    fs::create_dir_all(&blobs)?;
    let sha = pensine_core::chunks::digest(bytes);
    fs::write(blobs.join(&sha), bytes)?;
    conn.execute(
        "INSERT INTO files (id, name, mime, size, sha256, note, fact_id, author, created_at)
         VALUES ('file_old', 'memo.txt', 'text/plain', 12, ?1, '', NULL, 'device', 1)",
        [&sha],
    )?;
    drop(conn);
    // The manager starts again.
    let _m = Manager::open(d.path(), options()).map_err(|e| e.1)?;
    let conn = b.open_space(&from)?;
    let active: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE '%car is serviced%'",
        [],
        |r| r.get(0),
    )?;
    let pending: i64 = conn.query_row("SELECT count(*) FROM move_pending", [], |r| r.get(0))?;
    let there: i64 = b.open_space(&to)?.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE '%car is serviced%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!((active, pending, there), (0, 0, 1), "the move is finished");
    let logged: i64 = conn.query_row(
        "SELECT count(*) FROM sync_log WHERE eid = ?1",
        ["f:file_old"],
        |r| r.get(0),
    )?;
    assert!(files::info(&b, &from, "file_old")?.is_some());
    assert_eq!(logged, 1, "the file event is written");
    Ok(())
}

/// Without `--public-url`, the address handed out is the one the manager listens on; a given one still wins.
#[test]
fn without_a_public_address_the_manager_names_the_port_it_listens_on() -> Res {
    let d = Dir::new("listened");
    let mut o = options();
    o.public_url = None;
    o.listen = Some("0.0.0.0:8799".into());
    let m = Manager::open(d.path(), o).map_err(|e| e.1)?;
    assert_eq!(m.registry().public_url, "http://127.0.0.1:8799");
    drop(m);
    let d = Dir::new("listened-given");
    let mut o = options();
    o.listen = Some("0.0.0.0:8799".into());
    let m = Manager::open(d.path(), o).map_err(|e| e.1)?;
    assert_eq!(m.registry().public_url, "https://memory.example.net");
    Ok(())
}

#[test]
fn a_person_off_the_tailnet_connects_claude_with_a_code_the_administrator_makes() -> Res {
    let d = Dir::new("invite");
    let m = two_people(&d)?;
    for p in ["alice", "lou"] {
        let app = m.person_app(p).ok_or("person")?;
        app.brain.add_vault(users::OWNER, p, "me")?;
    }
    let reg = m.handle(&rq(
        "POST",
        "/oauth/register",
        &[],
        &json!({ "client_name": "Claude", "redirect_uris": [CALLBACK] }).to_string(),
    ));
    let client = reg.body_json()["client_id"]
        .as_str()
        .ok_or("client")?
        .to_string();
    let ask = r#"{"name":"lou"}"#;
    // Only an administrator makes a code: not Lou, not a stranger.
    let lou = ingress("ha-l", "lou", "POST", "/admin/api/users/invite", ask);
    assert_eq!(m.handle(&lou).status, 403);
    assert_eq!(
        m.handle(&rq("POST", "/admin/api/users/invite", &[], ask))
            .status,
        401
    );
    let alice = ingress("ha-w", "alice", "POST", "/admin/api/users/invite", ask);
    let made = m.handle(&alice);
    assert_eq!(made.status, 200, "{}", made.body_json());
    let code = made.body_json()["code"].as_str().ok_or("code")?.to_string();
    assert_eq!(
        m.handle(&ingress(
            "ha-w",
            "alice",
            "POST",
            "/admin/api/users/invite",
            r#"{"name":"nobody"}"#
        ))
        .status,
        404
    );
    // Lou, with no Tailscale, opens the consent page from the internet and enters the code.
    let q = authorize_path(&client);
    let form = |extra: &str| {
        q.split_once('?')
            .map(|(_, b)| format!("{b}{extra}"))
            .unwrap_or_default()
    };
    let site = [
        ("origin", "https://memory.example.net"),
        ("sec-fetch-site", "same-origin"),
    ];
    assert_eq!(m.handle(&rq("GET", &q, &[], "")).status, 200);
    let consent = m.handle(&rq(
        "POST",
        "/oauth/authorize",
        &site,
        &form(&format!("&invite={code}")),
    ));
    assert_eq!(
        consent.status,
        200,
        "{}",
        String::from_utf8_lossy(&consent.body)
    );
    let html = String::from_utf8(consent.body.clone())?;
    assert!(
        html.contains("lou-me") && !html.contains("alice-me"),
        "{html}"
    );
    let nonce = html
        .split("name=nonce value=")
        .nth(1)
        .and_then(|r| r.split('>').next())
        .ok_or("nonce")?
        .to_string();
    let done = m.handle(&rq(
        "POST",
        "/oauth/authorize",
        &site,
        &form(&format!("&decision=allow&nonce={nonce}&v_lou-me=1")),
    ));
    assert_eq!(done.status, 302, "{}", String::from_utf8_lossy(&done.body));
    let loc = done
        .headers
        .iter()
        .find(|(k, _)| k == "Location")
        .map(|(_, v)| v.clone())
        .ok_or("location")?;
    let auth = loc
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?;
    let tok = m.handle(&rq(
        "POST",
        "/oauth/token",
        &[("content-type", "application/x-www-form-urlencoded")],
        &format!("grant_type=authorization_code&code={auth}&redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback&client_id={client}&code_verifier={VERIFIER}"),
    ));
    assert_eq!(tok.status, 200, "{}", tok.body_json());
    let access = tok.body_json()["access_token"]
        .as_str()
        .ok_or("token")?
        .to_string();
    for person in ["alice", "lou"] {
        let app = m.person_app(person).ok_or("app")?;
        let ctl = app.brain.open_control()?;
        if let Some(who) = access::authenticate(&ctl, &access, pensine_core::util::now())? {
            note(&who.agent, &access);
        }
    }
    let stats = tool_text(&m.handle(&mcp(&access, "stats", json!({}))));
    let names: Vec<&str> = stats
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_eq!(names, vec!["lou-me"], "{stats}");
    // The code is spent.
    let again = m.handle(&rq(
        "POST",
        "/oauth/authorize",
        &site,
        &form(&format!("&invite={code}")),
    ));
    assert_eq!(again.status, 403);
    Ok(())
}

#[test]
fn the_invitation_endpoint_takes_an_administrators_console_token_and_never_funnel() -> Res {
    let d = Dir::new("invite-token");
    let m = two_people(&d)?;
    let ctl = m.registry().brain.open_control()?;
    let token_of = |name: &str| -> Result<String, Box<dyn Error>> {
        let u = users::user_by_name(&ctl, name)?.ok_or("person")?;
        Ok(access::console_token(&ctl, &u.id)?)
    };
    let ask = r#"{"name":"lou"}"#;
    let post = |token: &str, funnel: bool| {
        let mut headers = vec![("authorization", format!("Bearer {token}"))];
        if funnel {
            headers.push(("tailscale-funnel-request", "?1".to_string()));
        }
        let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        m.handle(&rq("POST", "/admin/api/users/invite", &h, ask))
    };
    // An administrator's token makes a code; a member's token does not; nor does a token through Funnel.
    let admin = token_of("alice")?;
    let made = post(&admin, false);
    assert_eq!(made.status, 200, "{}", made.body_json());
    assert_eq!(made.body_json()["code"].as_str().map(str::len), Some(14));
    assert_eq!(
        made.body_json()["connector_url"],
        "https://memory.example.net/mcp/lou-claude"
    );
    assert_eq!(post(&admin, true).status, 404, "never through Funnel");
    assert_eq!(
        post(&token_of("lou")?, false).status,
        401,
        "a member is not an administrator"
    );
    assert_eq!(post("not-a-token", false).status, 401);
    for _ in 0..10 {
        post("not-a-token", false);
    }
    assert_eq!(post(&admin, false).status, 429);
    let app = m.person_app("alice").ok_or("person")?;
    let vault = app.brain.add_vault(users::OWNER, "alice", "me")?;
    let key = key(&m, "alice", "valid-key", &format!("{vault}:r"))?;
    let mcp = rq(
        "POST",
        "/mcp/alice/valid-key",
        &[("authorization", &format!("Bearer {key}"))],
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    assert!(
        !m.registry()
            .blocked(pensine_core::util::now(), mcp.ip.as_deref().unwrap_or("?"))
    );
    assert_eq!(m.handle(&mcp).status, 200);
    Ok(())
}

#[test]
fn a_restored_manager_keeps_the_name_already_stored_in_its_memory() -> Res {
    let old = Dir::new("before-import");
    let restored = Dir::new("after-import");
    let first = Manager::open(old.path(), options()).map_err(|e| e.1)?;
    assert_eq!(first.node_name(), "Pensine");
    assert_eq!(
        pensine_core::directory::stored_own_name(&first.registry().brain)?.as_deref(),
        Some("Pensine"),
    );
    let mut explicit = options();
    explicit.node_name = Some("Atelier".into());
    drop(first);
    let named = Manager::open(old.path(), explicit).map_err(|e| e.1)?;
    assert_eq!(named.node_name(), "Atelier");
    let ctl = named.registry().brain.open_control()?;
    ctl.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(ctl);
    drop(named);

    // The add-on imports the manager's existing registry and data directories.
    let registry = restored.path().join("registry");
    fs::create_dir_all(&registry)?;
    for entry in fs::read_dir(old.path().join("registry"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), registry.join(entry.file_name()))?;
        }
    }
    for _ in 0..2 {
        let restarted = Manager::open(restored.path(), options()).map_err(|e| e.1)?;
        assert_eq!(restarted.node_name(), "Atelier");
    }
    Ok(())
}
