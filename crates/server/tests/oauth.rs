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
use memcp_server::oauth;
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
        asm["authorization_endpoint"], "https://memory.example.net/oauth/authorize",
        "the public page, which asks for a code and leads a tailnet person on to the private one"
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
    let public = e.app.handle(&rq("GET", &q, &[], "", None));
    assert_eq!(public.status, 200, "public listener: a form for a code");
    let html = String::from_utf8(public.body.clone())?;
    assert!(html.contains("invitation code") && !html.contains("Allow"));
    // A person on the tailnet is led to the private page with the same request.
    assert!(
        html.contains("href=\"https://memory.example.net:8792/oauth/authorize?")
            && html.contains(&format!("client_id={client}")),
        "{html}"
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
    // The form may post to the page and be redirected to Claude, ChatGPT or Le Chat.
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
        csp.contains(
            "form-action 'self' https://callback.mistral.ai https://chatgpt.com https://claude.ai https://claude.com;"
        ),
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

#[test]
fn an_address_with_the_name_of_a_machine_makes_no_connector() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query_for(&client, "https://memory.example.net/mcp/peer-laptop");
    let (status, html) = page_of(&e, &q);
    assert_eq!(status, 403);
    assert!(!html.contains("name=nonce"), "{html}");
    Ok(())
}

/// The redirect's code after an approval of `q` with these ticks.
fn approved(e: &Env, q: &str, extra: &str) -> Result<String, Box<dyn Error>> {
    let (_, html) = page_of(e, q);
    let n = nonce_from(&html)?;
    let r = decide_with(e, q, &n, "allow", &[], extra);
    assert_eq!(r.status, 302);
    Ok(location(&r)
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?
        .to_string())
}

fn exchange(e: &Env, client: &str, code: &str) -> Result<String, Box<dyn Error>> {
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let t = token(e, &format!("grant_type=authorization_code&code={code}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}")).body_json();
    Ok(t["access_token"].as_str().ok_or("access")?.to_string())
}

#[test]
fn a_consent_never_takes_over_a_connection_it_did_not_make() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    let alive = |key: &str| {
        access::authenticate(&ctl, key, util_now())
            .ok()
            .flatten()
            .map(|c| c.agent)
    };
    // The name an address asked for is taken, between the consent and the exchange of its code, by a key made another
    // way (an app's, a device's).
    let client = register(&e)?;
    let code = approved(
        &e,
        &authorize_query_for(&client, "https://memory.example.net/mcp/laptop"),
        "&v_health=1",
    )?;
    let app_key = access::connect(
        &ctl,
        "laptop",
        &access::Rights::parse("personal:rw")?,
        None,
        false,
        false,
    )?;
    let got = exchange(&e, &client, &code)?;
    assert_eq!(
        alive(&app_key).as_deref(),
        Some("laptop"),
        "the app keeps its key"
    );
    let named = alive(&got).ok_or("the client's key works")?;
    assert_eq!(named, "laptop-2");
    assert_eq!(
        access::get(&ctl, "laptop")?.ok_or("laptop")?.rights,
        "personal:rw"
    );
    // The name a client gets by default is held by a key the person made by hand.
    let client = register(&e)?;
    let default = format!("oauth-owner-{}", &client[client.len() - 8..]);
    let by_hand = access::connect(
        &ctl,
        &default,
        &access::Rights::parse("personal:r")?,
        None,
        false,
        false,
    )?;
    let got = exchange(
        &e,
        &client,
        &approved(&e, &authorize_query(&client), "&all=1")?,
    )?;
    assert_eq!(
        alive(&by_hand),
        Some(default.clone()),
        "the key by hand still works"
    );
    assert_eq!(alive(&got), Some(format!("{default}-2")));
    assert_eq!(
        access::get(&ctl, &default)?.ok_or("kept")?.rights,
        "personal:r"
    );
    // The next consent of the same client finds that connection and renews it, as before.
    let again = exchange(
        &e,
        &client,
        &approved(&e, &authorize_query(&client), "&all=1")?,
    )?;
    assert_eq!(alive(&again), Some(format!("{default}-2")));
    assert_eq!(alive(&got), None, "renewed: the former key stops");
    Ok(())
}

fn registers(e: &Env, uri: &str) -> u16 {
    e.app
        .handle(&rq(
            "POST",
            "/oauth/register",
            &[],
            &json!({ "client_name": "assistant", "redirect_uris": [uri] }).to_string(),
            None,
        ))
        .status
}

#[test]
fn chatgpt_and_le_chat_register_with_their_own_callbacks() -> Res {
    let e = env()?;
    // ChatGPT's stable address, its address per connector, and Le Chat's.
    for ok in [
        "https://chatgpt.com/connector_platform_oauth_redirect",
        "https://chatgpt.com/connector/oauth/AbC_12-x9",
        "https://callback.mistral.ai/v1/integrations_auth/oauth2_callback",
    ] {
        assert_eq!(registers(&e, ok), 201, "{ok}");
    }
    // Nothing that only looks like them.
    for bad in [
        "https://chatgpt.com/connector/oauth/",
        "https://chatgpt.com/connector/oauth/a/b",
        "https://chatgpt.com/connector/oauth/a?next=https://evil.example",
        "https://chatgpt.com/connector/oauth/..",
        "https://chatgpt.com/connector/oauth/a%2F..",
        "https://chatgpt.com.evil.example/connector/oauth/a",
        "http://chatgpt.com/connector/oauth/a",
        "https://chatgpt.com/connector_platform_oauth_redirect/x",
    ] {
        assert_eq!(registers(&e, bad), 400, "{bad}");
    }
    Ok(())
}

#[test]
fn every_authorization_response_names_its_issuer() -> Res {
    let e = env()?;
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
    assert_eq!(asm["authorization_response_iss_parameter_supported"], true);
    assert_eq!(asm["issuer"], "https://memory.example.net");
    let iss = "iss=https%3A%2F%2Fmemory.example.net";
    let client = register(&e)?;
    // A code.
    let q = authorize_query(&client);
    let n = nonce(&e, &q)?;
    let ok = decide(
        &e,
        &q,
        &n,
        "allow",
        &[("origin", "https://memory.example.net:8792")],
    );
    assert!(location(&ok).contains(iss), "{}", location(&ok));
    // A refusal.
    let n = nonce(&e, &q)?;
    let no = decide(
        &e,
        &q,
        &n,
        "deny",
        &[("origin", "https://memory.example.net:8792")],
    );
    let loc = location(&no);
    assert!(
        loc.contains("error=access_denied") && loc.contains(iss),
        "{loc}"
    );
    // A bad request.
    let bad = e.app.handle(&rq(
        "GET",
        &q.replace("code_challenge_method=S256", "code_challenge_method=plain"),
        &[],
        "",
        Some("owner@example.org"),
    ));
    let loc = location(&bad);
    assert!(
        loc.contains("error=invalid_request") && loc.contains(iss),
        "{loc}"
    );
    Ok(())
}

// --- a person who is not on the tailnet approves with an invitation code ---

fn invite(e: &Env) -> Result<String, Box<dyn Error>> {
    Ok(oauth::invite_create(&e.app, "owner", util_now())
        .ok_or("an invitation")?
        .0)
}

fn form_of(q: &str, extra: &str) -> String {
    q.split_once('?')
        .map(|(_, b)| format!("{b}{extra}"))
        .unwrap_or_default()
}

fn public_post(e: &Env, body: &str, headers: &[(&str, &str)]) -> Response {
    e.app
        .handle(&rq("POST", "/oauth/authorize", headers, body, None))
}

const SITE: [(&str, &str); 1] = [("origin", "https://memory.example.net")];

#[test]
fn a_person_off_the_tailnet_approves_with_an_invitation_code() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    let code = invite(&e)?;
    assert_eq!(code.len(), 14, "XXXX-XXXX-XXXX");
    // Typed any way: lowercase, spaces for dashes.
    let typed = code.to_lowercase().replace('-', "+");
    let consent = public_post(&e, &form_of(&q, &format!("&invite={typed}")), &SITE);
    assert_eq!(
        consent.status,
        200,
        "{}",
        String::from_utf8_lossy(&consent.body)
    );
    let html = String::from_utf8(consent.body.clone())?;
    assert!(
        html.contains("Allow") && html.contains("Signed in as"),
        "{html}"
    );
    assert!(!html.contains(&code), "the page never repeats the code");
    let n = nonce_from(&html)?;
    // The approval carries the nonce the page gave, not the code.
    let done = public_post(
        &e,
        &form_of(&q, &format!("&decision=allow&nonce={n}&v_personal=1")),
        &SITE,
    );
    assert_eq!(done.status, 302, "{}", String::from_utf8_lossy(&done.body));
    let loc = location(&done);
    assert!(
        loc.starts_with(CALLBACK) && loc.contains("state=xyz"),
        "{loc}"
    );
    let c = loc
        .split("code=")
        .nth(1)
        .and_then(|c| c.split('&').next())
        .ok_or("code")?;
    let cb = "https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback";
    let t = token(
        &e,
        &format!("grant_type=authorization_code&code={c}&redirect_uri={cb}&client_id={client}&code_verifier={VERIFIER}"),
    )
    .body_json();
    assert!(t["access_token"].as_str().is_some(), "{t}");
    Ok(())
}

#[test]
fn an_invitation_code_is_good_once_and_wrong_ones_are_counted() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    let code = invite(&e)?;
    // A redirect address that is not registered does not spend the code.
    let bad = form_of(&q, &format!("&invite={code}")).replace("claude.ai", "evil.example");
    assert_eq!(public_post(&e, &bad, &SITE).status, 403);
    let ok = form_of(&q, &format!("&invite={code}"));
    assert_eq!(public_post(&e, &ok, &SITE).status, 200);
    assert_eq!(public_post(&e, &ok, &SITE).status, 403, "spent");
    // Wrong codes are counted per address; the address is held off, even for a good code.
    for _ in 0..12 {
        public_post(&e, &form_of(&q, "&invite=AAAA-AAAA-AAAA"), &SITE);
    }
    let fresh = invite(&e)?;
    let held = public_post(&e, &form_of(&q, &format!("&invite={fresh}")), &SITE);
    assert_eq!(held.status, 429);
    Ok(())
}

#[test]
fn an_expired_code_and_an_unknown_person_give_nothing() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    assert!(oauth::invite_create(&e.app, "nobody", util_now()).is_none());
    let (old, _) = oauth::invite_create(&e.app, "owner", util_now() - 7200).ok_or("invite")?;
    let r = public_post(&e, &form_of(&q, &format!("&invite={old}")), &SITE);
    assert_eq!(r.status, 403, "expired");
    // Not twelve letters of the alphabet: nothing to look up.
    assert_eq!(
        public_post(&e, &form_of(&q, "&invite=not-a-code"), &SITE).status,
        403
    );
    // No more than twenty wait at the same time.
    let e2 = env()?;
    for _ in 0..20 {
        assert!(oauth::invite_create(&e2.app, "owner", util_now()).is_some());
    }
    assert!(oauth::invite_create(&e2.app, "owner", util_now()).is_none());
    Ok(())
}

#[test]
fn the_public_approval_needs_its_nonce_and_its_own_site() -> Res {
    let e = env()?;
    let client = register(&e)?;
    let q = authorize_query(&client);
    let code = invite(&e)?;
    let consent = public_post(&e, &form_of(&q, &format!("&invite={code}")), &SITE);
    let n = nonce_from(&String::from_utf8(consent.body.clone())?)?;
    let allow = form_of(&q, &format!("&decision=allow&nonce={n}&v_personal=1"));
    assert_eq!(
        public_post(&e, &allow, &[("origin", "https://evil.example")]).status,
        403
    );
    assert_eq!(
        public_post(&e, &allow, &[("sec-fetch-site", "cross-site")]).status,
        403
    );
    // No nonce and no code, or a nonce nobody gave: the form again, never a code.
    let none = form_of(&q, "&decision=allow&v_personal=1");
    assert_eq!(public_post(&e, &none, &SITE).status, 403);
    let wrong = form_of(
        &q,
        "&decision=allow&nonce=00000000000000000000000000000000&v_personal=1",
    );
    assert_eq!(public_post(&e, &wrong, &SITE).status, 403);
    assert_eq!(public_post(&e, &allow, &SITE).status, 302);
    assert_eq!(
        public_post(&e, &allow, &SITE).status,
        403,
        "a nonce works once"
    );
    Ok(())
}
