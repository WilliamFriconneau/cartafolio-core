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
//! (`oauth-<person>-<client>`). The consent page lists the spaces the person reaches
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

/// Redirect addresses accepted by default (Claude's OAuth callback).
pub const DEFAULT_REDIRECTS: [&str; 2] = [
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
];
const CODE_TTL: i64 = 300;
/// A consent page stays valid ten minutes.
const CONSENT_TTL: i64 = 600;
/// Client registrations per address and minute.
const REGISTER_PER_MIN: i64 = 5;
const MAX_CLIENTS: i64 = 200;
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
    let login = app.login_url.as_deref().unwrap_or(&app.public_url);
    Response::json(
        200,
        &json!({
            "issuer": app.public_url,
            "authorization_endpoint": format!("{login}/oauth/authorize"),
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
    // space or a connection says that resource and no other.
    format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp/{address}\"",
        app.public_url
    )
}

/// What an authorization request's `resource` (RFC 8707, the address the person gave their
/// assistant) names after `/mcp/`: a space, or the name of the connection to make. `None` for
/// the general address or no resource.
enum Named {
    Space(Space),
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
        return Some(Named::Space(v));
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
    if uris.is_empty()
        || uris.len() > 5
        || uris
            .iter()
            .any(|u| !app.oauth_redirects.iter().any(|a| a == u))
    {
        return err(
            400,
            "invalid_redirect_uri",
            "redirect address not on this server's allow list",
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

/// `GET` (consent page) and `POST` (approval) `/oauth/authorize`, identity listener only.
pub fn authorize(app: &App, req: &Request, who: Option<User>) -> Response {
    let Some(me) = who else {
        return page(
            app,
            403,
            "<p>Open this page from one of your devices, with Tailscale on.</p>",
        );
    };
    let now = util::now();
    if req.method == "POST" {
        // The approval comes from the consent page itself, never from another site.
        let origin = req.header("origin");
        let expected = app.login_url.as_deref().map(origin_of);
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
            &format!("{redirect}{sep}{extra}&state={}", enc(state)),
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
    // A connector added with an address that names a space is for that space only.
    let named = resource_named(app, get("resource"));
    let scoped = match &named {
        Some(Named::Space(v)) => Some(v.clone()),
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
        // a bridge) is not taken over by signing in at its address: that would cut the app off and give its rights away.
        let made_by_signing_in = ctl
            .query_row(
                "SELECT 1 FROM oauth_refresh WHERE agent = ?1 AND user_id = ?2 LIMIT 1",
                rusqlite::params![n, me.id],
                |_| Ok(()),
            )
            .is_ok();
        if n.starts_with("console-") || (owner.is_some() && !made_by_signing_in) {
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
        // One tick for all of it, the spaces made later included, offered where the connector was
        // not added for one space.
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
                 <p class=mute>Signed in as {}. You can revoke it any time from the admin page.</p>",
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
    if req.method == "GET" {
        return consent_page(200, "");
    }
    let chosen: Vec<String> = reach
        .iter()
        .filter(|(space, _)| match &scoped {
            // The space of the address, and no other, whatever else the form carries.
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
fn issue(
    ctl: &Connection,
    user: &User,
    client_id: &str,
    choice: &Choice,
) -> Result<(String, String), String> {
    let agent = if choice.agent.is_empty() {
        agent_name(user, client_id)
    } else {
        choice.agent.clone()
    };
    let rights = Rights::parse(&choice.rights).map_err(|e| e.to_string())?;
    let owner = access::owner_of(ctl, &agent).map_err(|e| e.to_string())?;
    if owner.as_deref() == Some(user.id.as_str())
        && access::regrant(ctl, &agent, &rights).map_err(|e| e.to_string())?
    {
        access::set_read_only(ctl, &agent, choice.read_only).map_err(|e| e.to_string())?;
        if let Some(token) =
            access::renew(ctl, &agent, &user.id, ACCESS_DAYS).map_err(|e| e.to_string())?
        {
            return Ok((agent, token));
        }
    }
    let token = access::connect_for(
        ctl,
        &user.id,
        &agent,
        &rights,
        Some(ACCESS_DAYS),
        choice.read_only,
        owner.is_some(),
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

pub fn token(app: &App, req: &Request) -> Response {
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
