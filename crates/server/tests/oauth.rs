//! OAuth 2.1 as a remote MCP client (Claude on the web) goes through it:
//! discovery, registration, consent on the tailnet, code with PKCE, refresh.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine as _;
use memcp_core::access;
use memcp_core::text::Language;
use memcp_core::users;
use memcp_core::{Brain, Space};
use memcp_server::app::App;
use memcp_server::http::{Request, Response, parse_query};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);
const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "a-long-random-verifier-string-of-at-least-43-characters-0123456789";

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
    let dir = std::env::temp_dir().join(format!("memcp-oauth-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let sp = |s: &str| Space::new(s).map_err(|e| e.to_string());
    let brain = Brain::init(&dir, &[sp("personal")?, sp("health")?], Language::English)?;
    let mut app = App::new(brain);
    app.public_url = "https://memory.example.net".into();
    app.login_url = Some("https://memory.example.net:8792".into());
    app.tailnet_users
        .insert("owner@example.org".into(), "owner".into());
    Ok(Env { dir, app })
}

fn rq(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
    tailnet: Option<&str>,
) -> Request {
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
        via_ingress: false,
        tailnet_login: tailnet.map(String::from),
    }
}

fn location(r: &Response) -> String {
    r.headers
        .iter()
        .find(|(k, _)| k == "Location")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

fn challenge() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

fn register(e: &Env) -> Result<String, Box<dyn Error>> {
    let r = e.app.handle(&rq(
        "POST",
        "/oauth/register",
        &[],
        &json!({ "client_name": "Claude", "redirect_uris": [CALLBACK] }).to_string(),
        None,
    ));
    assert_eq!(r.status, 201, "{}", r.body_json());
    Ok(r.body_json()["client_id"]
        .as_str()
        .ok_or("client_id")?
        .to_string())
}

fn authorize_query(client: &str) -> String {
    format!(
        "/oauth/authorize?response_type=code&client_id={client}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state=xyz",
        "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback",
        challenge()
    )
}

/// The consent page's one-time nonce.
fn nonce(e: &Env, q: &str) -> Result<String, Box<dyn Error>> {
    let page = e
        .app
        .handle(&rq("GET", q, &[], "", Some("owner@example.org")));
    assert_eq!(page.status, 200);
    let html = String::from_utf8(page.body.clone())?;
    assert!(html.contains("Allow"));
    Ok(html
        .split("name=nonce value=")
        .nth(1)
        .and_then(|r| r.split('>').next())
        .ok_or("no nonce")?
        .to_string())
}

fn decide(e: &Env, q: &str, nonce: &str, decision: &str, headers: &[(&str, &str)]) -> Response {
    decide_with(e, q, nonce, decision, headers, "&v_personal=1")
}

/// The approval with exactly these extra form fields (the ticked vaults and options).
fn decide_with(
    e: &Env,
    q: &str,
    nonce: &str,
    decision: &str,
    headers: &[(&str, &str)],
    extra: &str,
) -> Response {
    let body = q
        .split_once('?')
        .map(|(_, b)| format!("{b}&decision={decision}&nonce={nonce}{extra}"))
        .unwrap_or_default();
    e.app.handle(&rq(
        "POST",
        "/oauth/authorize",
        headers,
        &body,
        Some("owner@example.org"),
    ))
}

/// Consent on the tailnet, then the code from the redirect.
fn code(e: &Env, client: &str) -> Result<String, Box<dyn Error>> {
    code_with(e, client, "&v_personal=1")
}

fn code_with(e: &Env, client: &str, extra: &str) -> Result<String, Box<dyn Error>> {
    let q = authorize_query(client);
    let n = nonce(e, &q)?;
    let r = decide_with(
        e,
        &q,
        &n,
        "allow",
        &[("origin", "https://memory.example.net:8792")],
        extra,
    );
    assert_eq!(r.status, 302);
    let loc = location(&r);
    assert!(
        loc.starts_with(CALLBACK) && loc.contains("state=xyz"),
        "{loc}"
    );
    Ok(loc
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("no code")?
        .to_string())
}

fn token(e: &Env, body: &str) -> Response {
    e.app.handle(&rq(
        "POST",
        "/oauth/token",
        &[("content-type", "application/x-www-form-urlencoded")],
        body,
        None,
    ))
}

/// The address of the connection an access token belongs to: `/mcp/<its name>`.
fn address_of(e: &Env, access: &str) -> String {
    let name = e
        .app
        .brain
        .open_control()
        .ok()
        .and_then(|ctl| {
            access::authenticate(&ctl, access, util_now())
                .ok()
                .flatten()
        })
        .map_or_else(|| "nobody".to_string(), |c| c.agent);
    format!("/mcp/{name}")
}

fn util_now() -> i64 {
    memcp_core::util::now()
}

fn ping(e: &Env, access: &str) -> u16 {
    let auth = format!("Bearer {access}");
    e.app
        .handle(&rq(
            "POST",
            &address_of(e, access),
            &[("authorization", &auth)],
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string(),
            None,
        ))
        .status
}

#[test]
fn discovery_starts_from_the_401() -> Res {
    let e = env()?;
    let r = e.app.handle(&rq("POST", "/mcp/claudeai", &[], "{}", None));
    assert_eq!(r.status, 401);
    let www = r
        .headers
        .iter()
        .find(|(k, _)| k == "WWW-Authenticate")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        www.contains(
            "resource_metadata=\"https://memory.example.net/.well-known/oauth-protected-resource/mcp/claudeai\""
        ),
        "{www}"
    );
    let pr = e
        .app
        .handle(&rq(
            "GET",
            "/.well-known/oauth-protected-resource/mcp/claudeai",
            &[],
            "",
            None,
        ))
        .body_json();
    assert_eq!(pr["resource"], "https://memory.example.net/mcp/claudeai");
    let asm = e
        .app
        .handle(&rq(
            "GET",
            "/.well-known/oauth-authorization-server",
            &[],
            "",
            None,
        ))
        .body_json();
    assert_eq!(
        asm["authorization_endpoint"],
        "https://memory.example.net:8792/oauth/authorize"
    );
    assert_eq!(
        asm["token_endpoint"],
        "https://memory.example.net/oauth/token"
    );
    assert_eq!(asm["code_challenge_methods_supported"], json!(["S256"]));
    Ok(())
}

#[test]
fn only_allowed_redirects_register() -> Res {
    let e = env()?;
    let r = e.app.handle(&rq(
        "POST",
        "/oauth/register",
        &[],
        &json!({ "redirect_uris": ["https://evil.example/cb"] }).to_string(),
        None,
    ));
    assert_eq!(r.status, 400);
    register(&e)?;
    Ok(())
}

#[test]
fn the_consent_page_needs_the_tailnet_and_a_known_person() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    assert_eq!(
        e.app.handle(&rq("GET", &q, &[], "", None)).status,
        403,
        "public listener"
    );
    assert_eq!(
        e.app.handle(&rq("GET", &q, &[], "", Some(""))).status,
        403,
        "no tailnet login"
    );
    assert_eq!(
        e.app
            .handle(&rq("GET", &q, &[], "", Some("stranger@example.org")))
            .status,
        403,
        "unknown login"
    );
    // A denial goes back with an error.
    let n = nonce(&e, &q)?;
    let r = decide(&e, &q, &n, "deny", &[]);
    assert!(location(&r).contains("error=access_denied"));
    Ok(())
}

#[test]
fn an_approval_needs_the_page_it_came_from() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    assert_eq!(decide(&e, &q, "", "allow", &[]).status, 400, "no nonce");
    let n = nonce(&e, &q)?;
    let other_site = [("origin", "https://evil.example")];
    assert_eq!(decide(&e, &q, &n, "allow", &other_site).status, 403);
    let cross = [("sec-fetch-site", "cross-site")];
    assert_eq!(decide(&e, &q, &n, "allow", &cross).status, 403);
    assert_eq!(decide(&e, &q, &n, "allow", &[]).status, 302);
    assert_eq!(
        decide(&e, &q, &n, "allow", &[]).status,
        400,
        "a nonce works once"
    );
    // The form may post to the page and be redirected to Claude.
    let page = e
        .app
        .handle(&rq("GET", &q, &[], "", Some("owner@example.org")));
    let csp = page
        .headers
        .iter()
        .find(|(k, _)| k == "Content-Security-Policy")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        csp.contains("form-action 'self' https://claude.ai https://claude.com;"),
        "{csp}"
    );
    Ok(())
}

/// A code exchanged for tokens: (access, refresh).
fn tokens(e: &Env, client: &str) -> Result<(String, String), Box<dyn Error>> {
    tokens_with(e, client, "&v_personal=1")
}

fn tokens_with(e: &Env, client: &str, extra: &str) -> Result<(String, String), Box<dyn Error>> {
    let c = code_with(e, client, extra)?;
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let t = token(
        e,
        &format!(
            "grant_type=authorization_code&code={c}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}"
        ),
    )
    .body_json();
    Ok((
        t["access_token"].as_str().ok_or("access")?.to_string(),
        t["refresh_token"].as_str().ok_or("refresh")?.to_string(),
    ))
}

fn refresh(e: &Env, client: &str, r: &str) -> Response {
    token(
        e,
        &format!("grant_type=refresh_token&refresh_token={r}&client_id={client}"),
    )
}

#[test]
fn a_refresh_keeps_restrictions_and_revocation() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let (_, r) = tokens(&e, &client)?;
    let ctl = e.app.brain.open_control()?;
    let agent = format!("oauth-owner-{}", &client[client.len() - 8..]);
    assert!(
        access::set_read_only(&ctl, &agent, true)?,
        "named after the person"
    );
    let r2 = refresh(&e, &client, &r);
    assert_eq!(r2.status, 200);
    let list = access::list(&ctl)?;
    let c = list.iter().find(|c| c.agent == agent).ok_or("connection")?;
    assert!(c.read_only_forced, "read-only kept");
    let r2 = r2.body_json()["refresh_token"]
        .as_str()
        .ok_or("refresh")?
        .to_string();
    assert!(access::revoke(&ctl, &agent)?);
    assert_eq!(
        refresh(&e, &client, &r2).status,
        400,
        "revoked stays revoked"
    );
    Ok(())
}

#[test]
fn registrations_are_rate_limited() -> Res {
    let e = env()?;
    let body = json!({ "redirect_uris": [CALLBACK] }).to_string();
    let codes: Vec<u16> = (0..7)
        .map(|_| {
            e.app
                .handle(&rq("POST", "/oauth/register", &[], &body, None))
                .status
        })
        .collect();
    assert_eq!(codes.iter().filter(|c| **c == 201).count(), 5, "{codes:?}");
    assert_eq!(codes[6], 429);
    Ok(())
}

#[test]
fn code_with_pkce_then_refresh_rotates_everything() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let c = code(&e, &client)?;
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let wrong = token(
        &e,
        &format!(
            "grant_type=authorization_code&code={c}&redirect_uri={cb}&client_id={client}&code_verifier=wrong"
        ),
    );
    assert_eq!(wrong.status, 400, "PKCE checked");
    let c = code(&e, &client)?;
    let ok = token(
        &e,
        &format!(
            "grant_type=authorization_code&code={c}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}"
        ),
    );
    assert_eq!(ok.status, 200, "{}", ok.body_json());
    let t: Value = ok.body_json();
    let access = t["access_token"].as_str().ok_or("access")?.to_string();
    let refresh = t["refresh_token"].as_str().ok_or("refresh")?.to_string();
    assert_eq!(ping(&e, &access), 200);
    let again = token(
        &e,
        &format!(
            "grant_type=authorization_code&code={c}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}"
        ),
    );
    assert_eq!(again.status, 400, "a code works once");

    let r = token(
        &e,
        &format!("grant_type=refresh_token&refresh_token={refresh}&client_id={client}"),
    );
    assert_eq!(r.status, 200);
    let access2 = r.body_json()["access_token"]
        .as_str()
        .ok_or("access2")?
        .to_string();
    assert_eq!(ping(&e, &access2), 200);
    assert_eq!(ping(&e, &access), 401, "the old access token stops");
    let reuse = token(
        &e,
        &format!("grant_type=refresh_token&refresh_token={refresh}&client_id={client}"),
    );
    assert_eq!(reuse.status, 400, "a refresh token works once");
    Ok(())
}

#[test]
fn the_admin_page_on_the_tailnet_knows_the_owner() -> Res {
    let e = env()?;
    let r = e.app.handle(&rq(
        "GET",
        "/admin/api/state",
        &[],
        "",
        Some("owner@example.org"),
    ));
    assert_eq!(r.status, 200);
    assert_eq!(r.body_json()["me"]["admin"], true);
    assert_eq!(
        e.app
            .handle(&rq(
                "GET",
                "/admin/api/state",
                &[],
                "",
                Some("x@example.org")
            ))
            .status,
        401
    );
    assert_eq!(
        e.app
            .handle(&rq("POST", "/mcp/x", &[], "{}", Some("owner@example.org")))
            .status,
        404,
        "no MCP on the identity listener"
    );
    Ok(())
}

/// A `recall` through an access token, in one vault.
fn recall(e: &Env, access: &str, space: &str) -> Value {
    let auth = format!("Bearer {access}");
    e.app
        .handle(&rq(
            "POST",
            &address_of(e, access),
            &[("authorization", &auth)],
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "recall", "arguments": { "query": "x", "space": space } } })
            .to_string(),
            None,
        ))
        .body_json()
}

fn connection_of(e: &Env, client: &str) -> Result<access::ConnectionInfo, Box<dyn Error>> {
    let ctl = e.app.brain.open_control()?;
    let agent = format!("oauth-owner-{}", &client[client.len() - 8..]);
    Ok(access::list(&ctl)?
        .into_iter()
        .find(|c| c.agent == agent && c.revoked_at.is_none())
        .ok_or("no active connection")?)
}

#[test]
fn the_consent_page_lists_the_vaults_with_nothing_ticked() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let page = e.app.handle(&rq(
        "GET",
        &authorize_query(&client),
        &[],
        "",
        Some("owner@example.org"),
    ));
    let html = String::from_utf8(page.body)?;
    assert!(html.contains("name=v_personal") && html.contains("name=v_health"));
    assert!(html.contains("name=readonly"));
    assert!(
        !html.contains("name=upstream"),
        "no tools from elsewhere are offered: memory only"
    );
    assert!(!html.contains("checked"), "nothing is ticked beforehand");
    Ok(())
}

#[test]
fn nothing_ticked_grants_nothing() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    let n = nonce(&e, &q)?;
    let r = decide_with(&e, &q, &n, "allow", &[], "");
    assert_eq!(r.status, 400);
    assert!(String::from_utf8(r.body)?.contains("Tick at least one space"));
    let ctl = e.app.brain.open_control()?;
    let codes: i64 = ctl.query_row("SELECT count(*) FROM oauth_codes", [], |r| r.get(0))?;
    assert_eq!(codes, 0, "no code was issued");
    // The same page still works once something is ticked, with a fresh nonce.
    let n = nonce(&e, &q)?;
    assert_eq!(decide(&e, &q, &n, "allow", &[]).status, 302);
    Ok(())
}

#[test]
fn only_what_was_ticked_is_granted() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let (access, _) = tokens(&e, &client)?;
    let c = connection_of(&e, &client)?;
    assert_eq!(c.rights, "personal:rw");
    assert!(!c.read_only_forced);
    assert_eq!(recall(&e, &access, "personal")["result"]["isError"], false);
    assert_eq!(
        recall(&e, &access, "health")["result"]["isError"],
        true,
        "the vault that was not ticked stays closed"
    );
    Ok(())
}

#[test]
fn a_forged_upstream_tick_grants_nothing() -> Res {
    let e = env()?;
    let client = register(&e)?;
    tokens_with(
        &e,
        &client,
        "&v_personal=1&v_health=1&readonly=1&upstream=1",
    )?;
    let c = connection_of(&e, &client)?;
    assert_eq!(c.rights, "health:rw,personal:rw");
    assert!(c.read_only_forced);
    Ok(())
}

#[test]
fn approving_again_replaces_the_rights_on_the_same_connection() -> Res {
    let e = env()?;
    let client = register(&e)?;
    tokens(&e, &client)?;
    let (access, _) = tokens_with(&e, &client, "&v_health=1")?;
    let c = connection_of(&e, &client)?;
    assert_eq!(c.rights, "health:rw", "the new choice replaced the old one");
    let ctl = e.app.brain.open_control()?;
    let agents = access::list(&ctl)?
        .iter()
        .filter(|c| c.agent.starts_with("oauth-owner-") && c.revoked_at.is_none())
        .count();
    assert_eq!(agents, 1, "one connection per person and client");
    assert_eq!(recall(&e, &access, "health")["result"]["isError"], false);
    assert_eq!(recall(&e, &access, "personal")["result"]["isError"], true);
    Ok(())
}

#[test]
fn a_refresh_keeps_the_ticked_vaults() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let (_, r) = tokens(&e, &client)?;
    let r2 = refresh(&e, &client, &r);
    assert_eq!(r2.status, 200);
    let access = r2.body_json()["access_token"]
        .as_str()
        .ok_or("access")?
        .to_string();
    assert_eq!(connection_of(&e, &client)?.rights, "personal:rw");
    assert_eq!(recall(&e, &access, "health")["result"]["isError"], true);
    Ok(())
}

#[test]
fn a_code_that_chose_no_vault_is_refused() -> Res {
    // A code issued before the choice existed has no rights: it grants nothing.
    let e = env()?;
    let client = register(&e)?;
    let code = "legacy-code-0123456789";
    let ctl = e.app.brain.open_control()?;
    ctl.execute(
        "INSERT INTO oauth_codes (hash, client_id, user_id, redirect_uri, challenge, expires_at, rights)
         VALUES (?1, ?2, 'owner', ?3, ?4, ?5, '')",
        rusqlite::params![
            Sha256::digest(code.as_bytes()).to_vec(),
            client,
            CALLBACK,
            challenge(),
            memcp_core::util::now() + 300
        ],
    )?;
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let r = token(
        &e,
        &format!(
            "grant_type=authorization_code&code={code}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}"
        ),
    );
    assert_eq!(r.status, 400, "{}", r.body_json());
    assert!(
        r.body_json()["error_description"]
            .as_str()
            .is_some_and(|d| d.contains("chose no space"))
    );
    Ok(())
}

/// The authorization query of a connector added with this address (RFC 8707 `resource`).
fn authorize_query_for(client: &str, resource: &str) -> String {
    let enc: String = resource
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("{}&resource={enc}", authorize_query(client))
}

fn page_of(e: &Env, q: &str) -> (u16, String) {
    let r = e
        .app
        .handle(&rq("GET", q, &[], "", Some("owner@example.org")));
    (r.status, String::from_utf8_lossy(&r.body).to_string())
}

#[test]
fn an_address_that_names_a_vault_gives_a_connector_for_that_vault_only() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query_for(&client, "https://memory.example.net/mcp/health");
    let (status, html) = page_of(&e, &q);
    assert_eq!(status, 200);
    assert!(html.contains("added for the space <b>health</b>"), "{html}");
    assert!(
        html.contains("name=v_health") && !html.contains("name=v_personal"),
        "no other vault offered"
    );
    assert!(
        !html.contains("name=upstream"),
        "no upstream tools for a connector of one vault"
    );
    assert!(html.contains("name=readonly"));

    // The form is forged to ask for more: the server grants the vault of the address, no more.
    let n = nonce_from(&html)?;
    let r = decide_with(
        &e,
        &q,
        &n,
        "allow",
        &[("origin", "https://memory.example.net:8792")],
        "&v_personal=1&v_health=1&all=1",
    );
    assert_eq!(r.status, 302);
    let code = location(&r)
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?
        .to_string();
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let t = token(&e, &format!("grant_type=authorization_code&code={code}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}")).body_json();
    let access = t["access_token"].as_str().ok_or("access")?.to_string();
    let c = connection_of(&e, &client)?;
    assert_eq!(c.rights, "health:rw");
    assert_eq!(recall(&e, &access, "health")["result"]["isError"], false);
    assert_eq!(recall(&e, &access, "personal")["result"]["isError"], true);
    Ok(())
}

fn nonce_from(html: &str) -> Result<String, Box<dyn Error>> {
    Ok(html
        .split("name=nonce value=")
        .nth(1)
        .and_then(|r| r.split('>').next())
        .ok_or("no nonce")?
        .to_string())
}

#[test]
fn a_connector_of_one_vault_can_be_read_only_and_a_vault_not_reached_is_refused() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query_for(&client, "https://memory.example.net/mcp/personal");
    let (_, html) = page_of(&e, &q);
    let n = nonce_from(&html)?;
    let r = decide_with(&e, &q, &n, "allow", &[], "&readonly=1");
    assert_eq!(r.status, 302);
    let code = location(&r)
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?
        .to_string();
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    token(
        &e,
        &format!(
            "grant_type=authorization_code&code={code}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}"
        ),
    );
    let c = connection_of(&e, &client)?;
    assert_eq!(
        (c.rights.as_str(), c.read_only_forced),
        ("personal:rw", true)
    );

    // An address that is no vault's names the connection to make: the consent page says so.
    let q = authorize_query_for(&client, "https://memory.example.net/mcp/nowhere");
    let (status, html) = page_of(&e, &q);
    assert_eq!(status, 200);
    assert!(html.contains("will be called <b>nowhere</b>") && html.contains("name=nonce"));
    Ok(())
}

#[test]
fn the_general_address_keeps_the_picker_with_nothing_ticked() -> Res {
    let e = env()?;
    let client = register(&e)?;
    for resource in [
        "https://memory.example.net/mcp",
        "https://elsewhere.example/mcp/health",
        "",
    ] {
        let q = if resource.is_empty() {
            authorize_query(&client)
        } else {
            authorize_query_for(&client, resource)
        };
        let (status, html) = page_of(&e, &q);
        assert_eq!(status, 200, "{resource}");
        assert!(
            html.contains("name=v_personal") && html.contains("name=v_health"),
            "{resource}: the picker"
        );
        assert!(
            !html.contains("checked") && !html.contains("added for the vault"),
            "{resource}"
        );
    }
    Ok(())
}

#[test]
fn one_tick_gives_all_the_vaults_present_and_future() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let (access, _) = tokens_with(&e, &client, "&all=1")?;
    let c = connection_of(&e, &client)?;
    assert_eq!(c.rights, "all", "every vault, the ones made later too");
    assert_eq!(recall(&e, &access, "personal")["result"]["isError"], false);
    assert_eq!(recall(&e, &access, "health")["result"]["isError"], false);
    // Read only still applies to it.
    let client2 = register(&e)?;
    tokens_with(&e, &client2, "&all=1&readonly=1")?;
    assert!(connection_of(&e, &client2)?.read_only_forced);
    Ok(())
}

#[test]
fn an_address_that_names_a_connection_makes_that_connection_with_what_was_ticked() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query_for(&client, "https://memory.example.net/mcp/claudeai");
    let (status, html) = page_of(&e, &q);
    assert_eq!(status, 200);
    assert!(html.contains("will be called <b>claudeai</b>"));
    // Not a connector of one vault: the person ticks.
    assert!(html.contains("name=all") && html.contains("name=v_health"));
    let n = nonce_from(&html)?;
    let r = decide_with(&e, &q, &n, "allow", &[], "&v_health=1");
    assert_eq!(r.status, 302);
    let code = location(&r)
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?
        .to_string();
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let t = token(&e, &format!("grant_type=authorization_code&code={code}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}")).body_json();
    let ctl = e.app.brain.open_control()?;
    let named = access::list(&ctl)?
        .into_iter()
        .find(|c| c.agent == "claudeai")
        .ok_or("no connection named claudeai")?;
    assert_eq!(named.rights, "health:rw");
    // Its refresh keeps the name.
    let refresh = t["refresh_token"].as_str().ok_or("refresh")?;
    let t2 = token(
        &e,
        &format!("grant_type=refresh_token&refresh_token={refresh}&client_id={client}"),
    )
    .body_json();
    let access = t2["access_token"].as_str().ok_or("renewed")?.to_string();
    assert_eq!(
        access::list(&ctl)?
            .iter()
            .filter(|c| c.agent == "claudeai")
            .count(),
        1
    );
    assert_eq!(recall(&e, &access, "health")["result"]["isError"], false);
    Ok(())
}

#[test]
fn an_address_naming_a_connection_of_someone_else_or_a_vault_is_not_a_new_connection() -> Res {
    let e = env()?;
    let client = register(&e)?;
    // A name already used by another person's connection: refused.
    {
        let ctl = e.app.brain.open_control()?;
        let other = users::create_user(&ctl, users::OWNER, "kim", "Kim", false)?.id;
        e.app.brain.add_vault(&other, "kim", "work")?;
        access::connect_for(
            &ctl,
            &other,
            "taken",
            &access::Rights::parse("kim-work:rw")?,
            None,
            false,
            false,
        )?;
    }
    let q = authorize_query_for(&client, "https://memory.example.net/mcp/taken");
    let (status, html) = page_of(&e, &q);
    assert_eq!(status, 403);
    assert!(html.contains("already used by another connection") && !html.contains("name=nonce"));
    // One's own connection made another way (an app's key) is not taken over by signing in at its address,
    // and a console key's name never gives a connector anything.
    {
        let ctl = e.app.brain.open_control()?;
        access::connect(
            &ctl,
            "an-app",
            &access::Rights::parse("health/sources/app:rw")?,
            None,
            false,
            false,
        )?;
    }
    for name in ["an-app", "console-owner"] {
        let q = authorize_query_for(&client, &format!("https://memory.example.net/mcp/{name}"));
        let (status, html) = page_of(&e, &q);
        assert_eq!(status, 403, "{name}");
        assert!(
            html.contains("not made by signing in") && !html.contains("name=nonce"),
            "{name}: {html}"
        );
    }
    Ok(())
}
