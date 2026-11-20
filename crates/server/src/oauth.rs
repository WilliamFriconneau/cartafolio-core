//! OAuth 2.1 for remote MCP clients (Claude on the web and its apps), which call
//! the server from their own infrastructure and need a public address.
//!
//! - Discovery: `/.well-known/oauth-protected-resource/mcp/<name>` and
//!   `/.well-known/oauth-authorization-server`; a 401 on `/mcp/<name>` points to them.
//! - Registration (RFC 7591): only redirect addresses on the allow list.
//! - Authorization: the consent page is served on the identity listener only
//!   (reached through Tailscale, which tells who is there), never on the public one.
//! - Token: authorization code with PKCE S256, then refresh tokens that rotate.
//!
//! An access token is a normal MCP connection of the person
//! (`oauth-<person>-<client>`). The consent page lists the vaults the person reaches
//! with none ticked: the connection gets exactly what was ticked, nothing if nothing
//! was, and never more than the
//! person reaches. It is revoked like any other, and a refresh keeps its rights.
//!
//! The identity listener believes the `Tailscale-User-Login` header of any request
//! from the loopback: every local account of the host can claim a tailnet
//! identity. Run it on a host whose local accounts you trust.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use memcp_core::Space;
use memcp_core::access::{self, Rights};
use memcp_core::text::hex;
use memcp_core::users::{self, User};
use memcp_core::util;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::http::{Request, Response};

/// Redirect addresses accepted by default: Claude's OAuth callback, ChatGPT's stable one (used when
/// the authorization server names its issuer in every response, RFC 9207, as this one does) and Le Chat's.
pub const DEFAULT_REDIRECTS: [&str; 4] = [
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
    "https://chatgpt.com/connector_platform_oauth_redirect",
    "https://callback.mistral.ai/v1/integrations_auth/oauth2_callback",
];

/// ChatGPT's callback of one connector, `https://chatgpt.com/connector/oauth/<id>`: accepted for any id
/// of letters, digits, `-` and `_`, and nothing else after it.
const CHATGPT_CALLBACK_PREFIX: &str = "https://chatgpt.com/connector/oauth/";

/// A redirect address this node accepts: one of its list, or the callback of one ChatGPT connector.
fn redirect_allowed(app: &App, uri: &str) -> bool {
    app.oauth_redirects.iter().any(|a| a == uri)
        || uri.strip_prefix(CHATGPT_CALLBACK_PREFIX).is_some_and(|id| {
            (1..=128).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}
const CODE_TTL: i64 = 300;
/// A consent page stays valid ten minutes.
const CONSENT_TTL: i64 = 600;
/// Client registrations per address and minute.
const REGISTER_PER_MIN: i64 = 5;
const MAX_CLIENTS: i64 = 200;
/// An invitation code is good for an hour, once.
const INVITE_TTL: i64 = 3600;
/// Invitations waiting at the same time.
const MAX_INVITES: usize = 20;
/// Crockford's base32: no I, L, O or U, so a code read aloud or typed on a phone is not misread.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Access tokens last 30 days; the client refreshes them.
const ACCESS_DAYS: i64 = 30;

fn sha(s: &str) -> Vec<u8> {
    Sha256::digest(s.as_bytes()).to_vec()
}

fn err(status: u16, code: &str, desc: &str) -> Response {
    Response::json(status, &json!({ "error": code, "error_description": desc }))
        .with("Cache-Control", "no-store")
}

/// `a=1&b=x%20y` with percent and `+` decoding.
pub fn form(body: &[u8]) -> std::collections::HashMap<String, String> {
    let dec = |s: &str| -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < b.len() => {
                    match u8::from_str_radix(
                        std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"),
                        16,
                    ) {
                        Ok(v) => {
                            out.push(v);
                            i += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|p| p.split_once('=').map(|(k, v)| (dec(k), dec(v))))
        .collect()
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `https://host:port` of an address.
fn origin_of(url: &str) -> &str {
    let rest = url.find("://").map_or(0, |i| i + 3);
    url[rest..].find('/').map_or(url, |i| &url[..rest + i])
}

pub fn protected_resource(app: &App, address: &str) -> Response {
    let resource = format!("{}/mcp/{address}", app.public_url);
    Response::json(
        200,
        &json!({
            "resource": resource,
            "authorization_servers": [app.public_url],
            "bearer_methods_supported": ["header"],
            "scopes_supported": ["memory"],
        }),
    )
}

pub fn authorization_server(app: &App) -> Response {
    // The consent page is the public one: it asks for an invitation code, and sends a person who is on the tailnet on to
    // the private page, where Tailscale says who they are. A client cannot be told two addresses, and the private one
    // cannot be reached by a person who is not on the tailnet.
    Response::json(
        200,
        &json!({
            "issuer": app.public_url,
            // Every authorization response, an error included, carries `iss` (RFC 9207).
            "authorization_response_iss_parameter_supported": true,
            "authorization_endpoint": format!("{}/oauth/authorize", app.public_url),
            "token_endpoint": format!("{}/oauth/token", app.public_url),
            "registration_endpoint": format!("{}/oauth/register", app.public_url),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
            "scopes_supported": ["memory"],
        }),
    )
}

/// The `WWW-Authenticate` value that sends an MCP client to discovery.
pub fn challenge(app: &App, address: &str) -> String {
    // The client follows the address given here, so the metadata of an address that names a
    // vault or a connection says that resource and no other.
    format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp/{address}\"",
        app.public_url
    )
}

/// What an authorization request's `resource` (RFC 8707, the address the person gave their
/// assistant) names after `/mcp/`: a vault, or the name of the connection to make. `None` for
/// the general address or no resource.
enum Named {
    Vault(Space),
    Connection(String),
}

fn resource_named(app: &App, resource: &str) -> Option<Named> {
    let rest = resource.strip_prefix(app.public_url.as_str())?;
    let seg = rest.strip_prefix("/mcp/")?.trim_end_matches('/');
    if let Some(v) = app
        .brain
        .spaces()
        .ok()
        .and_then(|l| l.into_iter().find(|s| s.as_str() == seg))
    {
        return Some(Named::Vault(v));
    }
    access::valid_agent(seg).then(|| Named::Connection(seg.to_string()))
}

/// `POST /oauth/register` (dynamic client registration).
pub fn register(app: &App, req: &Request) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return err(400, "invalid_client_metadata", "JSON expected");
    };
    let uris: Vec<String> = body
        .get("redirect_uris")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|u| u.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if uris.is_empty() || uris.len() > 5 || uris.iter().any(|u| !redirect_allowed(app, u)) {
        return err(
            400,
            "invalid_redirect_uri",
            "redirect address not on this node's allow list",
        );
    }
    let now = util::now();
    let ip = req.ip.as_deref().unwrap_or("?");
    if !app.rate_ok(now, &format!("oauth-register:{ip}"), REGISTER_PER_MIN) {
        return err(
            429,
            "slow_down",
            "too many registrations, try again in a minute",
        );
    }
    let Ok(ctl) = app.brain.open_control() else {
        return err(500, "server_error", "storage failed");
    };
    let count = |ctl: &Connection| -> i64 {
        ctl.query_row("SELECT count(*) FROM oauth_clients", [], |r| r.get(0))
            .unwrap_or(MAX_CLIENTS)
    };
    if count(&ctl) >= MAX_CLIENTS {
        // Clients never used make room, the oldest first: registering many, from many addresses, cannot keep a real
        // connector out (it registers and signs in within minutes).
        let _ = ctl.execute(
            "DELETE FROM oauth_clients WHERE client_id IN (
               SELECT client_id FROM oauth_clients
               WHERE client_id NOT IN (SELECT client_id FROM oauth_codes)
                 AND client_id NOT IN (SELECT client_id FROM oauth_refresh)
                 AND created_at < ?1
               ORDER BY created_at LIMIT 20)",
            [now - 600],
        );
    }
    if count(&ctl) >= MAX_CLIENTS {
        return err(
            400,
            "invalid_client_metadata",
            "too many registered clients",
        );
    }
    let name: String = body
        .get("client_name")
        .and_then(Value::as_str)
        .unwrap_or("MCP client")
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    let client_id = format!("hcpo_{}", hex(&util::random_bytes::<12>()));
    if ctl
        .execute(
            "INSERT INTO oauth_clients (client_id, name, redirect_uris, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![client_id, name, json!(uris).to_string(), now],
        )
        .is_err()
    {
        return err(500, "server_error", "storage failed");
    }
    Response::json(
        201,
        &json!({
            "client_id": client_id, "client_id_issued_at": now, "client_name": name,
            "redirect_uris": uris, "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"],
        }),
    )
    .with("Cache-Control", "no-store")
}

fn client(ctl: &Connection, id: &str) -> Option<(String, Vec<String>)> {
    ctl.query_row(
        "SELECT name, redirect_uris FROM oauth_clients WHERE client_id = ?1",
        [id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
    .map(|(n, u)| (n, serde_json::from_str(&u).unwrap_or_default()))
}

fn page(app: &App, status: u16, body: &str) -> Response {
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
         <title>memcp</title><style>body{{font:16px system-ui,sans-serif;max-width:28rem;margin:4rem auto;padding:0 1rem;color:#1d1d1b;background:#f6f5f2}}\
         @media(prefers-color-scheme:dark){{body{{color:#ecebe7;background:#141413}}}}button{{font:inherit;padding:.5rem 1rem;border-radius:8px;border:1px solid #2f5d8a;background:#2f5d8a;color:#fff}}\
         .mute{{color:#6b6a66}}</style>{body}"
    );
    Response {
        status,
        headers: vec![
            ("Content-Type".into(), "text/html; charset=utf-8".into()),
            (
                "Content-Security-Policy".into(),
                format!(
                    "default-src 'none'; style-src 'unsafe-inline'; form-action 'self' {}; frame-ancestors 'none'",
                    redirect_origins(app)
                ),
            ),
            ("Cache-Control".into(), "no-store".into()),
        ],
        body: html.into_bytes(),
        file: None,
    }
}

/// The approval form posts here, then is redirected to the client: the browser
/// checks `form-action` against both.
fn redirect_origins(app: &App) -> String {
    let mut o: Vec<&str> = app.oauth_redirects.iter().map(|u| origin_of(u)).collect();
    o.sort_unstable();
    o.dedup();
    o.join(" ")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The key an invitation code is kept under: the digest of its twelve letters, however it was typed (dashes, spaces,
/// lowercase, and the letters Crockford reads as digits). `None` when it cannot be a code.
fn invite_key(code: &str) -> Option<String> {
    let c: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            u => u,
        })
        .collect();
    (c.len() == 12 && c.bytes().all(|b| CODE_ALPHABET.contains(&b))).then(|| hex(&sha(&c)))
}

/// A one-time code that lets the person named here approve a connector from the public address, where Tailscale does
/// not tell who is there (a person who is not on the tailnet). It is shown once and kept only as a digest, in memory.
/// Returns the code, as `XXXX-XXXX-XXXX`, and when it ends; `None` when too many are waiting or the person is unknown.
pub fn invite_create(app: &App, person: &str, now: i64) -> Option<(String, i64)> {
    let ctl = app.brain.open_control().ok()?;
    let user = users::resolve(&ctl, person).ok().filter(|u| !u.disabled)?;
    // Five random bits per letter: 256 is a multiple of 32, so no letter is likelier than another.
    let code: String = util::random_bytes::<12>()
        .iter()
        .map(|b| CODE_ALPHABET[(b & 31) as usize] as char)
        .collect();
    let end = now + INVITE_TTL;
    {
        let mut m = app.invites.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, (_, exp)| *exp > now);
        if m.len() >= MAX_INVITES {
            return None;
        }
        m.insert(hex(&sha(&code)), (user.id.clone(), end));
    }
    access::auth_event(&ctl, Some(&user.name), None, "oauth_invite_made", "");
    Some((
        format!("{}-{}-{}", &code[0..4], &code[4..8], &code[8..12]),
        end,
    ))
}

/// The person an invitation code was made for. The code is spent: it opens one consent page.
fn invite_take(app: &App, code: &str, now: i64) -> Option<String> {
    let key = invite_key(code)?;
    let mut m = app.invites.lock().unwrap_or_else(|e| e.into_inner());
    m.retain(|_, (_, exp)| *exp > now);
    m.remove(&key).map(|(id, _)| id)
}

/// Whether this installation holds the invitation code of a consent form (the manager finds the person by it).
pub fn holds_invite(app: &App, body: &[u8]) -> bool {
    let q = form(body);
    let now = util::now();
    q.get("invite")
        .and_then(|c| invite_key(c))
        .is_some_and(|k| {
            app.invites
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&k)
                .is_some_and(|(_, exp)| *exp > now)
        })
}

/// Whether this installation holds the nonce of a consent form.
pub fn holds_consent(app: &App, body: &[u8]) -> bool {
    let q = form(body);
    let now = util::now();
    q.get("nonce").is_some_and(|n| {
        app.consents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(n)
            .is_some_and(|(_, exp)| *exp > now)
    })
}

/// A request to the consent page that is not on the tailnet: the person is the one an invitation code was made for. The
/// code is checked, and spent, only for a client and a redirect address that are registered (a bad link does not burn
/// it), and wrong codes are counted per address like wrong keys. The second request (the approval) carries the nonce
/// the first one was given.
fn invited_person(app: &App, req: &Request, now: i64) -> Option<User> {
    if req.method != "POST" {
        return None;
    }
    let q = form(&req.body);
    let ctl = app.brain.open_control().ok()?;
    let person = |id: &str| users::user(&ctl, id).ok().flatten().filter(|u| !u.disabled);
    if let Some(n) = q.get("nonce").filter(|n| !n.is_empty()) {
        let id = app
            .consents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(n)
            .filter(|(_, exp)| *exp > now)
            .map(|(id, _)| id.clone())?;
        return person(&id);
    }
    let code = q.get("invite")?;
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) {
        return None;
    }
    let (_, uris) = client(&ctl, q.get("client_id").map(String::as_str).unwrap_or(""))?;
    if !uris.iter().any(|u| Some(u) == q.get("redirect_uri")) {
        return None;
    }
    match invite_take(app, code, now).and_then(|id| person(&id)) {
        Some(u) => {
            access::auth_event(
                &ctl,
                Some(&u.name),
                req.ip.as_deref(),
                "oauth_invite_used",
                "",
            );
            Some(u)
        }
        None => {
            app.fail(now, &who);
            None
        }
    }
}

/// What the public consent page shows to someone with no code yet, or a wrong one.
fn invite_form(app: &App, req: &Request, now: i64) -> Response {
    let q = if req.method == "POST" {
        form(&req.body)
    } else {
        req.query.clone()
    };
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let Ok(ctl) = app.brain.open_control() else {
        return page(app, 500, "<p>Storage failed.</p>");
    };
    let known = client(&ctl, get("client_id"))
        .filter(|(_, uris)| uris.iter().any(|u| u == get("redirect_uri")));
    let Some((name, _)) = known else {
        return page(
            app,
            403,
            "<p>Open this page from one of your devices, with Tailscale on.</p>",
        );
    };
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    let (status, note) = if req.method != "POST" {
        (200, "")
    } else if app.blocked(now, &who) {
        (
            429,
            "<p><b>Too many tries.</b> Wait a little, then start again from your assistant.</p>",
        )
    } else {
        (
            403,
            "<p><b>This code is not valid, or it has expired.</b> Ask for a new one.</p>",
        )
    };
    let hidden: String = [
        "client_id",
        "redirect_uri",
        "state",
        "code_challenge",
        "code_challenge_method",
        "response_type",
        "scope",
        "resource",
    ]
    .iter()
    .map(|k| format!("<input type=hidden name={k} value=\"{}\">", esc(get(k))))
    .collect();
    // On your own devices with Tailscale on: the same request on the private page, where no code is needed.
    let private = app.login_url.as_deref().map_or(String::new(), |login| {
        let query: Vec<String> = [
            "response_type",
            "client_id",
            "redirect_uri",
            "state",
            "code_challenge",
            "code_challenge_method",
            "scope",
            "resource",
        ]
        .iter()
        .filter(|k| !get(k).is_empty())
        .map(|k| format!("{k}={}", enc(get(k))))
        .collect();
        format!(
            "<p class=mute>On one of your own devices with Tailscale on? <a href=\"{}/oauth/authorize?{}\">Continue here</a>, no code needed.</p>",
            esc(login),
            esc(&query.join("&"))
        )
    });
    page(
        app,
        status,
        &format!(
            "<h1>Connect {n}</h1><p>{n} asks to use a memory. Enter the invitation code you were given.</p>{note}\
             <form method=post>{hidden}<p><input name=invite autocomplete=off autocapitalize=characters \
             spellcheck=false placeholder=\"XXXX-XXXX-XXXX\" size=18 required autofocus></p><button>Continue</button></form>\
             <p class=mute>The code is good once, for an hour.</p>{private}",
            n = esc(&name)
        ),
    )
}

/// `GET` (consent page) and `POST` (approval) `/oauth/authorize`: on the identity listener, where Tailscale tells who
/// is there; on the public address, for a person who has an invitation code.
pub fn authorize(app: &App, req: &Request, who: Option<User>) -> Response {
    let now = util::now();
    let public = req.tailnet_login.is_none();
    let who = match who {
        Some(u) => Some(u),
        None if public => invited_person(app, req, now),
        None => None,
    };
    let Some(me) = who else {
        if public {
            return invite_form(app, req, now);
        }
        return page(
            app,
            403,
            "<p>Open this page from one of your devices, with Tailscale on.</p>",
        );
    };
    if req.method == "POST" {
        // The approval comes from the consent page itself, never from another site.
        let origin = req.header("origin");
        let expected = if public {
            Some(origin_of(&app.public_url))
        } else {
            app.login_url.as_deref().map(origin_of)
        };
        if req.header("sec-fetch-site") == "cross-site"
            || (!origin.is_empty() && Some(origin) != expected)
        {
            return page(
                app,
                403,
                "<p>This approval did not come from the consent page.</p>",
            );
        }
    }
    let q = if req.method == "POST" {
        form(&req.body)
    } else {
        req.query.clone()
    };
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let Ok(ctl) = app.brain.open_control() else {
        return page(app, 500, "<p>Storage failed.</p>");
    };
    let Some((name, uris)) = client(&ctl, get("client_id")) else {
        return page(app, 400, "<p>Unknown client.</p>");
    };
    let redirect = get("redirect_uri");
    if !uris.iter().any(|u| u == redirect) {
        return page(app, 400, "<p>This redirect address was not registered.</p>");
    }
    let state = get("state");
    let back = |extra: &str| {
        let sep = if redirect.contains('?') { '&' } else { '?' };
        Response::empty(302).with(
            "Location",
            &format!(
                "{redirect}{sep}{extra}&state={}&iss={}",
                enc(state),
                enc(&app.public_url)
            ),
        )
    };
    if get("response_type") != "code"
        || get("code_challenge_method") != "S256"
        || get("code_challenge").len() < 43
    {
        return back("error=invalid_request");
    }
    // The page lists what the person reaches, with nothing ticked: a connection gets only
    // what was ticked, and nothing at all if nothing was.
    let reach = users::access_of(&ctl, &me.id).unwrap_or_default();
    // A connector added with an address that names a vault is for that vault only.
    let named = resource_named(app, get("resource"));
    let scoped = match &named {
        Some(Named::Vault(v)) => Some(v.clone()),
        _ => None,
    };
    // An address that names a connection makes a connection of that name: it must be free, or this
    // person's own.
    let connection_name = match &named {
        Some(Named::Connection(n)) => Some(n.clone()),
        _ => None,
    };
    if let Some(n) = &connection_name {
        let owner = access::owner_of(&ctl, n).unwrap_or(None);
        if owner.as_deref().is_some_and(|o| o != me.id) {
            return page(
                app,
                403,
                "<p>This name is already used by another connection. Give the assistant another address.</p>",
            );
        }
        // A console key is never handed to a connector; and a connection made another way (an app's key, a key by hand,
        // a device) is not taken over by signing in at its address: that would cut the app off and give its rights away.
        let made_by_signing_in = ctl
            .query_row(
                "SELECT 1 FROM oauth_refresh WHERE agent = ?1 AND user_id = ?2 LIMIT 1",
                rusqlite::params![n, me.id],
                |_| Ok(()),
            )
            .is_ok();
        let kept = n.starts_with("console-") || n.starts_with(memcp_core::sync::PEER_PREFIX);
        if kept || (owner.is_some() && !made_by_signing_in) {
            return page(
                app,
                403,
                "<p>This name is already used by one of your connections that was not made by signing in. Give the assistant another address.</p>",
            );
        }
    }
    if let Some(v) = &scoped
        && !reach.contains_key(v)
    {
        return page(
            app,
            403,
            "<p>You do not have access to this space, so an assistant cannot be connected to it.</p>",
        );
    }
    let consent_page = |status: u16, note: &str| -> Response {
        let nonce = hex(&util::random_bytes::<16>());
        {
            let mut m = app.consents.lock().unwrap_or_else(|e| e.into_inner());
            m.retain(|_, (_, exp)| *exp > now);
            if m.len() > 1000 {
                return page(
                    app,
                    429,
                    "<p>Too many pending approvals, try again later.</p>",
                );
            }
            m.insert(nonce.clone(), (me.id.clone(), now + CONSENT_TTL));
        }
        let hidden: String = [
            "client_id",
            "redirect_uri",
            "state",
            "code_challenge",
            "code_challenge_method",
            "response_type",
            "scope",
            "resource",
        ]
        .iter()
        .map(|k| format!("<input type=hidden name={k} value=\"{}\">", esc(get(k))))
        .chain(std::iter::once(format!(
            "<input type=hidden name=nonce value={nonce}>"
        )))
        .collect();
        let boxes: String = if let Some(v) = &scoped {
            format!(
                "<p>This connector was added for the space <b>{v}</b>. It will reach that space and nothing else.</p><input type=hidden name=v_{v} value=1>"
            )
        } else {
            reach
                .iter()
                .map(|(space, grant)| {
                    format!(
                        "<p><label><input type=checkbox name=v_{s} value=1> {s}{ro}</label></p>",
                        s = space.as_str(),
                        ro = if grant.write {
                            ""
                        } else {
                            " <span class=mute>(read only for you)</span>"
                        }
                    )
                })
                .collect()
        };
        // One tick for all of it, the vaults made later included, offered where the connector was
        // not added for one vault.
        let called = connection_name.as_ref().map_or(String::new(), |n| {
            format!("<p>This connection will be called <b>{}</b>.</p>", esc(n))
        });
        let everything = if scoped.is_none() {
            "<p><label><input type=checkbox name=all value=1> <b>All my spaces</b>, and the ones I make later</label></p><p class=mute>Or only some:</p>"
        } else {
            ""
        };
        page(
            app,
            status,
            &format!(
                "<h1>Connect {}</h1><p>{} asks to use your memory. Tick what it may reach:</p>{note}\
                 {called}<form method=post>{hidden}{everything}{boxes}\
                 <p><label><input type=checkbox name=readonly value=1> read only</label></p>\
                 <input type=hidden name=decision value=allow><button>Allow</button></form>\
                 <p class=mute>Signed in as {}. You can revoke it any time from the page of your memory.</p>",
                esc(&name),
                esc(&name),
                esc(if me.display.is_empty() {
                    &me.name
                } else {
                    &me.display
                })
            ),
        )
    };
    // A code just opened the page: the form that carried it is answered with the consent page, as a GET is.
    let opened_by_code = public && q.contains_key("invite") && !q.contains_key("nonce");
    if req.method == "GET" || opened_by_code {
        return consent_page(200, "");
    }
    let chosen: Vec<String> = reach
        .iter()
        .filter(|(space, _)| match &scoped {
            // The vault of the address, and no other, whatever else the form carries.
            Some(v) => *space == v,
            None => q.contains_key(format!("v_{}", space.as_str()).as_str()),
        })
        .map(|(space, grant)| {
            format!(
                "{}:{}",
                space.as_str(),
                if grant.write { "rw" } else { "r" }
            )
        })
        .collect();
    let everything = scoped.is_none() && q.contains_key("all");
    if get("decision") == "allow" && chosen.is_empty() && !everything {
        return consent_page(
            400,
            "<p><b>Tick at least one space.</b> Without one, this assistant gets nothing.</p>",
        );
    }
    let fresh = app
        .consents
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(get("nonce"))
        .is_some_and(|(id, exp)| id == me.id && exp > now);
    if !fresh {
        return page(
            app,
            400,
            "<p>This page has expired. Start again from your assistant.</p>",
        );
    }
    if get("decision") != "allow" {
        return back("error=access_denied");
    }
    let code = hex(&util::random_bytes::<24>());
    if ctl
        .execute(
            "INSERT INTO oauth_codes (hash, client_id, user_id, redirect_uri, challenge, expires_at, rights, read_only, agent) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                sha(&code),
                get("client_id"),
                me.id,
                redirect,
                get("code_challenge"),
                now + CODE_TTL,
                if everything { "all".to_string() } else { chosen.join(",") },
                i64::from(q.contains_key("readonly")),
                connection_name.clone().unwrap_or_default()
            ],
        )
        .is_err()
    {
        return page(app, 500, "<p>Storage failed.</p>");
    }
    access::auth_event(
        &ctl,
        Some(&me.name),
        req.ip.as_deref(),
        "oauth_allow",
        &format!(
            "{name}: {}",
            if everything {
                "all".to_string()
            } else {
                chosen.join(",")
            }
        ),
    );
    back(&format!("code={code}"))
}

/// `oauth-<person>-<client>`: one connection per person and client.
fn agent_name(user: &User, client_id: &str) -> String {
    let who: String = user
        .name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(20)
        .collect();
    let tail = &client_id[client_id.len().saturating_sub(8)..];
    if who.is_empty() {
        format!("oauth-{tail}")
    } else {
        format!("oauth-{who}-{tail}")
    }
}

/// What the person ticked on the consent page.
struct Choice {
    /// `health:rw,work:r`; never empty here.
    rights: String,
    read_only: bool,
    /// The name the address gave the connection; empty: named after the client.
    agent: String,
}

/// An authorization code as stored, with what was chosen when it was approved.
struct CodeRow {
    client_id: String,
    user_id: String,
    redirect: String,
    expires: i64,
    challenge: String,
    choice: Choice,
}

/// The person's MCP connection for a client after a consent; returns its token. The
/// choice just made is the connection's rights: a client that is approved again with
/// another choice gets that one, on the same connection. Nothing is ever granted that
/// was not ticked, and never beyond what the person reaches.
///
/// Only a connection an OAuth consent made for this person is ever reused. A name held by any other connection (a
/// machine's, an app's, a key made by hand, another person's), whether it was there at the consent or made since, is left
/// alone and the client gets another name: renewing that connection would cut its own device off and hand its rights to
/// the client.
fn issue(
    ctl: &Connection,
    user: &User,
    client_id: &str,
    choice: &Choice,
) -> Result<(String, String), String> {
    let wanted = if choice.agent.is_empty() {
        agent_name(user, client_id)
    } else {
        choice.agent.clone()
    };
    let rights = Rights::parse(&choice.rights).map_err(|e| e.to_string())?;
    // `None`: the name is free; `Some(true)`: this person's connection made by OAuth; `Some(false)`: anything else.
    let mine = |name: &str| -> Result<Option<bool>, String> {
        ctl.query_row(
            "SELECT coalesce(user_id, 'owner') = ?2 AND kind = 'oauth' FROM connections WHERE agent = ?1",
            params![name, user.id],
            |r| r.get::<_, bool>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    };
    // A name held by a connection OAuth did not make, or by someone else: the same name with `-2`, `-3`… , the first
    // that is free or this person's own OAuth connection, so that the next consent finds the same one again.
    let mut agent = wanted.clone();
    let mut reuse = mine(&agent)?;
    let mut n = 1;
    while reuse == Some(false) {
        n += 1;
        if n > 99 {
            return Err(format!("no free name left after {wanted}"));
        }
        let tail = format!("-{n}");
        let base: String = wanted.chars().take(40 - tail.len()).collect();
        agent = format!("{}{tail}", base.trim_end_matches('-'));
        reuse = mine(&agent)?;
    }
    if reuse == Some(true) && access::regrant(ctl, &agent, &rights).map_err(|e| e.to_string())? {
        access::set_read_only(ctl, &agent, choice.read_only).map_err(|e| e.to_string())?;
        if let Some(token) =
            access::renew(ctl, &agent, &user.id, ACCESS_DAYS).map_err(|e| e.to_string())?
        {
            return Ok((agent, token));
        }
    }
    let token = access::connect_oauth(
        ctl,
        &user.id,
        &agent,
        &rights,
        Some(ACCESS_DAYS),
        choice.read_only,
        reuse == Some(true),
    )
    .map_err(|e| e.to_string())?;
    Ok((agent, token))
}

/// `POST /oauth/token`.
/// Whether this installation issued the code, or the refresh token, a token request carries.
/// Nothing is consumed or counted: a gateway asks each person's installation before choosing.
pub fn holds_grant(app: &App, body: &[u8]) -> bool {
    let f = form(body);
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let Ok(ctl) = app.brain.open_control() else {
        return false;
    };
    let (sql, secret) = match get("grant_type") {
        "authorization_code" => (
            "SELECT 1 FROM oauth_codes WHERE hash = ?1 AND used = 0",
            get("code"),
        ),
        "refresh_token" => (
            "SELECT 1 FROM oauth_refresh WHERE hash = ?1 AND revoked_at IS NULL",
            get("refresh_token"),
        ),
        _ => return false,
    };
    ctl.query_row(sql, [sha(secret)], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

/// Copies a registered client from one installation to another, so that the person who consents
/// can be any of them. The same id, so the client keeps using the one it was given.
pub fn copy_client(from: &App, to: &App, client_id: &str) -> bool {
    let (Ok(a), Ok(b)) = (from.brain.open_control(), to.brain.open_control()) else {
        return false;
    };
    let row: Option<(String, String, i64)> = a
        .query_row(
            "SELECT name, redirect_uris, created_at FROM oauth_clients WHERE client_id = ?1",
            [client_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((name, uris, at)) = row else {
        return false;
    };
    b.execute(
        "INSERT OR IGNORE INTO oauth_clients (client_id, name, redirect_uris, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![client_id, name, uris, at],
    )
    .is_ok()
}

/// `POST /oauth/token`, guarded like the key routes: a code or a refresh token that is refused is a guess, counted
/// per address, and an address that keeps guessing is held off.
pub fn token(app: &App, req: &Request) -> Response {
    let now = util::now();
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) {
        return err(429, "slow_down", "too many attempts").with("Retry-After", "900");
    }
    let r = exchange(app, req);
    if r.status == 400 || r.status == 401 {
        app.fail(now, &who);
    }
    r
}

fn exchange(app: &App, req: &Request) -> Response {
    let f = form(&req.body);
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let Ok(ctl) = app.brain.open_control() else {
        return err(500, "server_error", "storage failed");
    };
    let client_id = if get("client_id").is_empty() {
        // client_secret_basic: the id is in the Authorization header.
        req.header("authorization")
            .strip_prefix("Basic ")
            .and_then(|b| {
                base64::engine::general_purpose::STANDARD
                    .decode(b.trim())
                    .ok()
            })
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.split(':').next().map(String::from))
            .unwrap_or_default()
    } else {
        get("client_id").to_string()
    };
    if client(&ctl, &client_id).is_none() {
        return err(401, "invalid_client", "unknown client");
    }
    let now = util::now();
    let (user_id, renewing, choice): (String, Option<String>, Option<Choice>) = match get(
        "grant_type",
    ) {
        "authorization_code" => {
            let h = sha(get("code"));
            let row: Option<CodeRow> = ctl
                .query_row(
                    "SELECT client_id, user_id, redirect_uri, expires_at, challenge, rights, read_only, agent FROM oauth_codes WHERE hash = ?1 AND used = 0",
                    [&h],
                    |r| {
                        Ok(CodeRow {
                            client_id: r.get(0)?,
                            user_id: r.get(1)?,
                            redirect: r.get(2)?,
                            expires: r.get(3)?,
                            challenge: r.get(4)?,
                            choice: Choice {
                                rights: r.get(5)?,
                                read_only: r.get::<_, i64>(6)? != 0,
                                agent: r.get(7)?,
                            },
                        })
                    },
                )
                .optional()
                .ok()
                .flatten();
            let Some(CodeRow {
                client_id: cid,
                user_id: user,
                redirect,
                expires,
                challenge,
                choice,
            }) = row
            else {
                return err(400, "invalid_grant", "unknown or used code");
            };
            if choice.rights.is_empty() {
                return err(
                    400,
                    "invalid_grant",
                    "this approval chose no space: approve again and tick what it may reach",
                );
            }
            let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(get("code_verifier").as_bytes()));
            if cid != client_id
                || redirect != get("redirect_uri")
                || expires < now
                || computed != challenge
            {
                return err(400, "invalid_grant", "code expired or not for this client");
            }
            // Claimed once, even by two requests at the same time.
            let claimed = ctl
                .execute(
                    "UPDATE oauth_codes SET used = 1 WHERE hash = ?1 AND used = 0",
                    [&h],
                )
                .unwrap_or(0);
            if claimed != 1 {
                return err(400, "invalid_grant", "code already used");
            }
            (user, None, Some(choice))
        }
        "refresh_token" => {
            let h = sha(get("refresh_token"));
            let row: Option<(String, String, String)> = ctl
                .query_row(
                    "SELECT client_id, user_id, agent FROM oauth_refresh WHERE hash = ?1 AND revoked_at IS NULL",
                    [&h],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .ok()
                .flatten();
            let Some((_, user, agent)) = row.filter(|(cid, _, _)| *cid == client_id) else {
                return err(400, "invalid_grant", "unknown or revoked refresh token");
            };
            let claimed = ctl
                .execute(
                    "UPDATE oauth_refresh SET revoked_at = ?2 WHERE hash = ?1 AND revoked_at IS NULL",
                    params![h, now],
                )
                .unwrap_or(0);
            if claimed != 1 {
                return err(400, "invalid_grant", "refresh token already used");
            }
            (user, Some(agent), None)
        }
        _ => {
            return err(
                400,
                "unsupported_grant_type",
                "authorization_code or refresh_token",
            );
        }
    };
    let Ok(Some(user)) = users::user(&ctl, &user_id) else {
        return err(400, "invalid_grant", "unknown person");
    };
    if user.disabled {
        return err(400, "invalid_grant", "this person is disabled");
    }
    let issued = match renewing {
        // A refresh renews the token only: rights, read-only and revocation stay.
        Some(agent) => match access::renew(&ctl, &agent, &user.id, ACCESS_DAYS) {
            Ok(Some(t)) => Ok((agent, t)),
            Ok(None) => return err(400, "invalid_grant", "this connection was revoked"),
            Err(e) => Err(e.to_string()),
        },
        None => match choice {
            Some(c) => issue(&ctl, &user, &client_id, &c),
            None => return err(400, "invalid_grant", "no approval to issue from"),
        },
    };
    let (agent, access_token) = match issued {
        Ok(x) => x,
        Err(e) => return err(500, "server_error", &e),
    };
    let refresh = hex(&util::random_bytes::<32>());
    if ctl
        .execute(
            "INSERT INTO oauth_refresh (hash, client_id, user_id, agent, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![sha(&refresh), client_id, user.id, agent, now],
        )
        .is_err()
    {
        return err(500, "server_error", "storage failed");
    }
    Response::json(
        200,
        &json!({
            "access_token": access_token, "token_type": "Bearer", "expires_in": ACCESS_DAYS * 86_400,
            "refresh_token": refresh, "scope": "memory",
        }),
    )
    .with("Cache-Control", "no-store")
}
