//! The admin page and its JSON API.
//!
//! Who is asking: a Home Assistant login (through the add-on's ingress
//! listener only, where the `X-Remote-User-*` headers come from the Supervisor),
//! or a console token (`Authorization: Bearer hcpc_…`). MCP tokens never open
//! this page. Administrators manage everyone; a member manages their own
//! spaces and tokens. A person reads and edits the notes of their own spaces here (`explore`), and
//! nobody else's: an administrator manages access, not content.
//!
//! Every write needs the `X-Requested-With: memcp` header: a page from
//! another site cannot set it without a CORS preflight, which is never allowed.

use memcp_core::access::{self, Rights};
use memcp_core::export;
use memcp_core::guide;
use memcp_core::users::{self, User};
use memcp_core::util::{self, iso};
use memcp_core::{Error, Space, files};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Request, Response};

const PAGE: &str = include_str!("admin.html");
const CSRF_HEADER: &str = "x-requested-with";
const CSRF_VALUE: &str = "memcp";

pub fn page() -> Response {
    Response {
        status: 200,
        headers: vec![
            ("Content-Type".into(), "text/html; charset=utf-8".into()),
            (
                "Content-Security-Policy".into(),
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'self'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'".into(),
            ),
            ("X-Content-Type-Options".into(), "nosniff".into()),
            ("Referrer-Policy".into(), "no-referrer".into()),
            ("Cache-Control".into(), "no-store".into()),
        ],
        body: PAGE.as_bytes().to_vec(),
    }
}

fn recover(app: &App, ctl: &Connection, req: &Request, now: i64) -> Response {
    let ha = req.header("x-remote-user-id");
    if ha.is_empty() {
        return err(400, "no account");
    }
    if req.header(CSRF_HEADER) != CSRF_VALUE {
        return err(403, "missing X-Requested-With: memcp");
    }
    if !app.rate_ok(now, &format!("recover:{ha}"), 5) {
        return err(429, "too many attempts, try again in a minute");
    }
    let key = serde_json::from_slice::<Value>(&req.body)
        .ok()
        .and_then(|b| b.get("key").and_then(Value::as_str).map(String::from))
        .unwrap_or_default();
    match users::claim_with_recovery_key(ctl, ha, &key) {
        Ok(Some(u)) => Response::json(200, &json!({ "user": user_json(&u) })),
        Ok(None) => err(400, "this key opens nothing"),
        Err(e) => fail(e),
    }
}

/// A text made safe for the value of a query parameter.
fn percent(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A QR code, as an SVG image the page shows inline (nothing is fetched to draw it).
fn qr_svg(text: &str) -> String {
    use qrcode::{QrCode, render::svg};
    QrCode::new(text.as_bytes())
        .map(|c| {
            c.render::<svg::Color>()
                .min_dimensions(220, 220)
                .quiet_zone(true)
                .build()
        })
        .unwrap_or_default()
}

fn err(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

fn fail(e: impl std::fmt::Display) -> Response {
    err(400, &e.to_string())
}

/// The person behind a request on the identity listener (Tailscale login mapped
/// with `--tailnet-user`), if any.
pub fn tailnet_person(app: &App, req: &Request) -> Option<User> {
    let login = req.tailnet_login.as_deref()?.to_lowercase();
    let name = app.tailnet_users.get(&login)?;
    let ctl = app.brain.open_control().ok()?;
    users::resolve(&ctl, name).ok()
}

/// The person behind the request, if any.
fn who(app: &App, ctl: &Connection, req: &Request, now: i64) -> Result<Option<User>, Error> {
    if req.tailnet_login.is_some() {
        return Ok(tailnet_person(app, req));
    }
    if req.via_ingress {
        let id = req.header("x-remote-user-id");
        if id.is_empty() {
            return Ok(None);
        }
        let name = req.header("x-remote-user-name");
        let display = req.header("x-remote-user-display-name");
        let name = if name.is_empty() { id } else { name };
        // An unknown login waits for an administrator; the approved ways in are unchanged.
        return Ok(
            match users::ha_login(
                ctl,
                id,
                name,
                display,
                &app.ingress_admins,
                app.auto_create_people,
            )? {
                users::HaLogin::Person(p) => Some(p),
                _ => None,
            },
        );
    }
    let token = crate::http::bearer_of(req.header("authorization"));
    let Some(user_id) =
        token.and_then(|t| access::authenticate_console(ctl, &t, now).ok().flatten())
    else {
        return Ok(None);
    };
    users::user(ctl, &user_id)
}

fn arg<'a>(body: &'a Value, key: &str) -> Result<&'a str, Response> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err(400, &format!("{key} is required")))
}

pub fn handle(app: &App, req: &Request, now: i64) -> Response {
    let Ok(ctl) = app.brain.open_control() else {
        return err(500, "storage failed");
    };
    // A console token is guessed at like any other secret: failures are counted per
    // address, with the same guard as the MCP endpoint.
    let by_token = !req.via_ingress && req.tailnet_login.is_none();
    // Counted apart from the MCP guard: a page that is merely opened (no token yet) must not lock the
    // assistants behind the same address out.
    let from = format!("console:{}", req.ip.as_deref().unwrap_or("?"));
    let token_given = crate::http::bearer_of(req.header("authorization")).is_some();
    if by_token && app.blocked(now, &from) {
        return err(429, "too many attempts").with("Retry-After", "900");
    }
    // An account the installation does not know yet, holding a recovery key, takes its person back.
    if req.via_ingress
        && req.method == "POST"
        && req
            .path
            .trim_end_matches('/')
            .ends_with("/admin/api/recover")
    {
        return recover(app, &ctl, req, now);
    }
    let me = match who(app, &ctl, req, now) {
        Ok(Some(u)) if !u.disabled => u,
        Ok(_) => {
            // Only a token that was given and is wrong is a guess.
            if by_token && token_given {
                app.fail(now, &from);
            }
            if req.via_ingress {
                match users::request_status(&ctl, req.header("x-remote-user-id")) {
                    Ok(Some(status)) if status == "pending" => {
                        return err(403, "your access request is waiting for an administrator");
                    }
                    Ok(Some(_)) => return err(403, "your access request was refused"),
                    _ => {}
                }
            }
            return err(
                401,
                "sign in through Home Assistant, or give a console token",
            )
            .with("WWW-Authenticate", "Bearer");
        }
        Err(e) => return fail(e),
    };
    let path = req.path.trim_end_matches('/');
    let path = path.strip_prefix("/admin/api").unwrap_or(path);
    if req.method == "GET" {
        return match path {
            "/state" => state(app, &ctl, &me),
            "/space" => crate::explore::overview(app, &ctl, &me, req, now),
            "/facts" => crate::explore::list(app, &ctl, &me, req, now),
            "/living" => {
                let args = serde_json::json!({
                    "space": req.query.get("space").cloned().unwrap_or_default(),
                    "page": req.query.get("page").cloned().unwrap_or_default(),
                });
                match living(app, &ctl, &me, "living_get", &args) {
                    Ok(v) => Response::json(200, &v),
                    Err(r) => r,
                }
            }
            "/journal" => crate::explore::journal(app, &ctl, &me, req, now),
            "/links" => crate::explore::graph(app, &ctl, &me, req),
            "/fact" => crate::explore::one(app, &ctl, &me, req),
            "/layout" => crate::explore::layout_get(app, &me),
            "/join/pending" => crate::join::pending(app, &me),
            "/sync/status" => match memcp_core::sync::status(&app.brain, now) {
                Ok(list) => Response::json(
                    200,
                    &json!({ "peers": list.iter().map(crate::sync_http::status_json).collect::<Vec<_>>() }),
                ),
                Err(e) => fail(e),
            },
            "/discover" => Response::json(200, &crate::replica::discover(app)),
            "/join-memory" => match crate::replica::join_state(
                app,
                req.query.get("id").map(String::as_str).unwrap_or(""),
            ) {
                Ok(v) => Response::json(200, &v),
                Err(m) => err(404, &m),
            },
            "/pairing/status" => crate::explore::pairing_status(&ctl, &me, req, now),
            "/timeline" => timeline(app, &ctl, &me, req, now),
            "/file" => file_bytes(app, &ctl, &me, req),
            "/guide" => guide_get(&ctl, &me, req),
            "/export" => export_get(app, &me),
            _ => err(404, "not found"),
        };
    }
    if req.method != "POST" {
        return Response::empty(405).with("Allow", "GET, POST");
    }
    if req.header(CSRF_HEADER) != CSRF_VALUE {
        return err(403, "missing X-Requested-With: memcp");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return err(400, "invalid JSON");
    };
    let done = if path.starts_with("/fact/") {
        crate::explore::write(app, &ctl, &me, path, &body)
    } else if path.starts_with("/join/") {
        crate::join::decide(app, &ctl, &me, path, &body)
    } else if path.starts_with("/space/") {
        crate::explore::space_write(app, &ctl, &me, path, &body)
    } else if path.starts_with("/layout/") {
        crate::explore::layout_write(app, &me, path, &body)
    } else if path == "/living" {
        living(app, &ctl, &me, "living_set", &body)
    } else {
        action(app, &ctl, &me, path, &body)
    };
    match done {
        Ok(v) => Response::json(200, &v),
        Err(r) => r,
    }
}

/// The person a console token is made for: anyone, for an administrator of the registry;
/// oneself otherwise.
fn console_target(ctl: &Connection, me: &User, body: &Value) -> Result<User, Response> {
    match body
        .get("user")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        None => Ok(me.clone()),
        Some(u) => {
            let t = users::resolve(ctl, u).map_err(fail)?;
            if t.id != me.id && !me.admin {
                return Err(err(403, "only an administrator acts for someone else"));
            }
            Ok(t)
        }
    }
}

/// The living boards of the person's own spaces, read and changed on the page with the same tools an assistant uses:
/// the person acts with their own rights (the spaces they own), as the connection `page`.
pub(crate) fn living(
    app: &App,
    ctl: &Connection,
    me: &User,
    tool: &str,
    args: &Value,
) -> Result<Value, Response> {
    let grants = memcp_core::users::access_of(ctl, &me.id).map_err(|e| err(500, &e.to_string()))?;
    let caller = memcp_core::access::Caller {
        id: "page".into(),
        agent: "page".into(),
        user_id: me.id.clone(),
        all_spaces: false,
        known: grants.keys().cloned().collect(),
        grants,
        paths: Default::default(),
        read_only_forced: false,
        no_person_guide: true,
        rate_per_min: memcp_core::access::DEFAULT_RATE_PER_MIN,
        max_text: memcp_core::access::DEFAULT_MAX_TEXT,
    };
    memcp_core::tools::call(&app.brain, &caller, tool, args).map_err(|e| err(400, &e.to_string()))
}

fn admin_only(me: &User) -> Result<(), Response> {
    if me.admin {
        Ok(())
    } else {
        Err(err(403, "administrators only"))
    }
}

fn action(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    body: &Value,
) -> Result<Value, Response> {
    let actor = me.id.as_str();
    match path {
        "/users" => {
            admin_only(me)?;
            let u = users::create_user(
                ctl,
                actor,
                arg(body, "name")?,
                body.get("display").and_then(Value::as_str).unwrap_or(""),
                body.get("admin").and_then(Value::as_bool).unwrap_or(false),
            )
            .map_err(fail)?;
            Ok(json!({ "user": user_json(&u) }))
        }
        "/guides/set" => {
            let scope = guide_scope(ctl, me, arg(body, "scope")?)?;
            let g = guide::set(ctl, actor, &scope, arg(body, "body")?).map_err(fail)?;
            Ok(json!({ "scope": g.scope, "chars": g.body.chars().count() }))
        }
        "/guides/clear" => {
            let scope = guide_scope(ctl, me, arg(body, "scope")?)?;
            let cleared = guide::clear(ctl, actor, &scope).map_err(fail)?;
            Ok(json!({ "cleared": cleared }))
        }
        "/app-guides/approve" => {
            let space = guide_owner(ctl, me, arg(body, "space")?)?;
            guide::approve_app_guide(
                &app.brain,
                ctl,
                &me.id,
                &space,
                arg(body, "app")?,
                arg(body, "hash")?,
            )
            .map_err(fail)?;
            Ok(json!({ "approved": true }))
        }
        "/app-guides/withdraw" => {
            let space = guide_owner(ctl, me, arg(body, "space")?)?;
            guide::withdraw_app_guide(ctl, actor, &space, arg(body, "app")?).map_err(fail)?;
            Ok(json!({ "withdrawn": true }))
        }
        "/requests/approve" => {
            admin_only(me)?;
            let ha = arg(body, "ha_user")?;
            let admin = body.get("admin").and_then(Value::as_bool).unwrap_or(false);
            let person = users::approve_request(ctl, actor, ha, admin).map_err(fail)?;
            Ok(json!({ "user": user_json(&person) }))
        }
        "/requests/refuse" => {
            admin_only(me)?;
            let done = users::refuse_request(ctl, actor, arg(body, "ha_user")?).map_err(fail)?;
            Ok(json!({ "refused": done }))
        }
        "/users/update" => {
            admin_only(me)?;
            let id = arg(body, "id")?;
            // Nobody takes their own administration away, or disables themselves, by a stray click.
            if id == me.id
                && (body.get("admin").and_then(Value::as_bool) == Some(false)
                    || body.get("disabled").and_then(Value::as_bool) == Some(true))
            {
                return Err(err(
                    400,
                    "you cannot remove your own administration or disable yourself: ask another administrator",
                ));
            }
            if let Some(d) = body.get("display").and_then(Value::as_str) {
                users::set_display(ctl, id, d).map_err(fail)?;
            }
            if let Some(a) = body.get("admin").and_then(Value::as_bool) {
                users::set_admin(ctl, actor, id, a).map_err(fail)?;
            }
            if let Some(d) = body.get("disabled").and_then(Value::as_bool) {
                users::set_disabled(ctl, actor, id, d).map_err(fail)?;
            }
            Ok(json!({ "ok": true }))
        }
        "/recovery/make" => {
            // A key that lets a new account take this memory over, shown once.
            let key = users::make_recovery_key(ctl, &me.id).map_err(fail)?;
            Ok(json!({ "key": key }))
        }
        "/heir/set" => {
            // Who may attach this memory to another account after a long absence (or nobody).
            let heir = body
                .get("heir")
                .and_then(Value::as_str)
                .filter(|h| !h.is_empty());
            users::set_heir(ctl, &me.id, heir).map_err(fail)?;
            Ok(json!({ "ok": true }))
        }
        "/requests/attach" => {
            let person = users::attach_request(
                ctl,
                &me.id,
                arg(body, "ha_user")?,
                arg(body, "person")?,
                util::now(),
            )
            .map_err(fail)?;
            Ok(json!({ "user": user_json(&person) }))
        }
        "/users/unlink" => {
            // Cuts the link to a Home Assistant account (it was deleted, say). Their memory stays
            // where it is; the next login of that name takes it up again.
            admin_only(me)?;
            let id = arg(body, "id")?;
            if id == me.id {
                return Err(err(400, "you cannot cut the link to your own account"));
            }
            users::set_ha_user(ctl, id, None).map_err(fail)?;
            Ok(json!({ "ok": true }))
        }
        "/users/console-token" => {
            let t = console_target(ctl, me, body)?;
            let token = access::console_token(ctl, &t.id).map_err(fail)?;
            Ok(json!({ "token": token }))
        }
        "/start" => {
            let name = body.get("name").and_then(Value::as_str).unwrap_or("");
            crate::replica::start(app, ctl, me, name).map_err(fail)
        }
        "/join-memory" => crate::replica::join_start(
            app,
            body.get("address").and_then(Value::as_str).unwrap_or(""),
        )
        .map_err(|m| err(400, &m)),
        "/spaces" => {
            // A vault comes from a kind of the catalogue, named after the person.
            let space = app
                .brain
                .add_vault(&me.id, &me.name, arg(body, "kind")?)
                .map_err(fail)?;
            Ok(json!({ "space": space.as_str(), "owner": me.id }))
        }
        "/pairing" => {
            // A code a bridge exchanges once, within ten minutes, for a key to the person's « À trier » only: a device
            // never chooses a vault, the janitor or the person sorts what it sends.
            let (code, triage) =
                memcp_core::pairing::create_for_triage(&app.brain, &me.id, &me.name, util::now())
                    .map_err(fail)?;
            // What a phone scans: one link, `memcp://join?server=…&code=…`, so that nothing is typed.
            let link = format!(
                "memcp://join?server={}&code={}",
                percent(&app.public_url),
                percent(&code)
            );
            Ok(json!({
                "code": code,
                "server": app.public_url,
                "expires_in": memcp_core::pairing::CODE_TTL,
                "triage": triage,
                "link": link,
                "qr": qr_svg(&link),
            }))
        }
        "/triage/send" => {
            // The person decides where a capture of « À trier » goes: the inbox of one of their vaults.
            let from = memcp_core::triage::of(&app.brain, &me.id)
                .map_err(fail)?
                .ok_or_else(|| fail(Error::Invalid("no « À trier » yet".into())))?;
            let to = Space::new(arg(body, "space")?).map_err(fail)?;
            if !users::access_of(ctl, &me.id)
                .map_err(fail)?
                .get(&to)
                .is_some_and(|g| g.write)
            {
                return Err(fail(Error::Invalid(format!(
                    "{to} is not one of your vaults"
                ))));
            }
            let id = memcp_core::triage::send(&app.brain, &from, arg(body, "id")?, &to, &me.id)
                .map_err(fail)?;
            Ok(json!({ "ok": true, "id": id, "space": to.as_str() }))
        }
        "/triage/send-many" => {
            // Several captures of « À trier » at once, to the inbox of one vault. Each one is sent on its own:
            // one that fails stays where it is and is named in the answer, the others still go.
            let from = memcp_core::triage::of(&app.brain, &me.id)
                .map_err(fail)?
                .ok_or_else(|| fail(Error::Invalid("no « À trier » yet".into())))?;
            let to = Space::new(arg(body, "space")?).map_err(fail)?;
            if !users::access_of(ctl, &me.id)
                .map_err(fail)?
                .get(&to)
                .is_some_and(|g| g.write)
            {
                return Err(fail(Error::Invalid(format!(
                    "{to} is not one of your vaults"
                ))));
            }
            let ids: Vec<&str> = body
                .get("ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if ids.is_empty() || ids.len() > 500 {
                return Err(err(400, "ids: between 1 and 500 captures expected"));
            }
            let (mut sent, mut failed) = (0usize, Vec::<String>::new());
            for id in ids {
                match memcp_core::triage::send(&app.brain, &from, id, &to, &me.id) {
                    Ok(_) => sent += 1,
                    Err(_) => failed.push(id.to_string()),
                }
            }
            Ok(
                json!({ "ok": failed.is_empty(), "sent": sent, "failed": failed, "space": to.as_str() }),
            )
        }
        "/spaces/open" => {
            // Opens one vault to an assistant: a connection limited to it.
            let space = Space::new(arg(body, "space")?).map_err(fail)?;
            let write = body.get("access").and_then(Value::as_str) != Some("r");
            let (agent, token) =
                access::open_space_connection(ctl, &me.id, &space, write, None).map_err(fail)?;
            Ok(json!({
                "agent": agent,
                "token": token,
                "rights": format!("{space}:{}", if write { "rw" } else { "r" }),
                "mcp_url": format!("{}/mcp/{agent}", app.public_url),
                "mcp_url_with_key": format!("{}/mcp/{agent}?key={token}", app.public_url),
                "claude_code": format!("claude mcp add --transport http {agent} {}/mcp/{agent} --header \"Authorization: Bearer {token}\"", app.public_url),
            }))
        }
        "/connections" => {
            let rights = Rights::parse(body.get("rights").and_then(Value::as_str).unwrap_or("all"))
                .map_err(fail)?;
            let days = body.get("days").and_then(Value::as_i64);
            let read_only = body
                .get("read_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let agent = arg(body, "agent")?;
            // The name of a connection this person revoked can be taken again.
            let reuse = access::list(ctl)
                .map_err(fail)?
                .iter()
                .any(|c| c.agent == agent && c.user_id == me.id && c.revoked_at.is_some());
            let token = access::connect_for(ctl, &me.id, agent, &rights, days, read_only, reuse)
                .map_err(fail)?;
            Ok(json!({
                "token": token,
                "mcp_url": format!("{}/mcp/{agent}", app.public_url),
                "mcp_url_with_key": format!("{}/mcp/{agent}?key={token}", app.public_url),
                "claude_code": format!("claude mcp add --transport http {agent} {}/mcp/{agent} --header \"Authorization: Bearer {token}\"", app.public_url),
            }))
        }
        "/connections/rename" => {
            let agent = arg(body, "agent")?;
            if access::owner_of(ctl, agent).map_err(fail)?.as_deref() != Some(me.id.as_str()) {
                return Err(err(403, "not your connection"));
            }
            access::rename(ctl, agent, arg(body, "name")?).map_err(fail)?;
            Ok(json!({ "ok": true }))
        }
        "/connections/regrant" => {
            // Changes what an active connection reaches (vaults and paths, read or write) without
            // touching its token or its address: the assistant has nothing to redo.
            let agent = arg(body, "agent")?;
            if access::owner_of(ctl, agent).map_err(fail)?.as_deref() != Some(me.id.as_str()) {
                return Err(err(403, "not your connection"));
            }
            let rights = Rights::parse(arg(body, "rights")?).map_err(fail)?;
            if !access::regrant(ctl, agent, &rights).map_err(fail)? {
                return Err(err(404, "no active connection by this name"));
            }
            if let Some(ro) = body.get("read_only").and_then(Value::as_bool) {
                access::set_read_only(ctl, agent, ro).map_err(fail)?;
            }
            Ok(json!({ "ok": true, "rights": rights.label() }))
        }
        "/connections/person-guide" => {
            // Whether this connection is told the person's own guide when it connects (a work assistant need not be).
            let agent = arg(body, "agent")?;
            if access::owner_of(ctl, agent).map_err(fail)?.as_deref() != Some(me.id.as_str()) {
                return Err(err(403, "not your connection"));
            }
            let told = body.get("told").and_then(Value::as_bool).unwrap_or(true);
            access::set_no_person_guide(ctl, agent, !told).map_err(fail)?;
            Ok(json!({ "ok": true }))
        }
        "/connections/revoke" => {
            let agent = arg(body, "agent")?;
            let owner = access::owner_of(ctl, agent).map_err(fail)?;
            if owner.as_deref() != Some(me.id.as_str()) {
                return Err(err(403, "not your connection"));
            }
            access::revoke(ctl, agent).map_err(fail)?;
            Ok(json!({ "ok": true }))
        }
        "/files" => {
            use base64::Engine as _;
            let space = Space::new(arg(body, "space")?).map_err(fail)?;
            let reach = users::access_of(ctl, &me.id).map_err(fail)?;
            if !reach.get(&space).is_some_and(|g| g.write) {
                return Err(err(403, "you cannot write to this space"));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(arg(body, "data")?.trim())
                .map_err(|_| err(400, "data: not valid base64"))?;
            let note = body.get("note").and_then(Value::as_str).unwrap_or("");
            let tags: Vec<String> = body
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let tags = memcp_core::facts::normalize_tags(&tags).map_err(fail)?;
            let f = files::put_tagged(
                &app.brain,
                &space,
                &me.name,
                arg(body, "name")?,
                note,
                &bytes,
                &tags,
            )
            .map_err(fail)?;
            Ok(json!({ "file": f.to_json() }))
        }
        "/files/retire" => {
            let space = Space::new(arg(body, "space")?).map_err(fail)?;
            let reach = users::access_of(ctl, &me.id).map_err(fail)?;
            if !reach.get(&space).is_some_and(|g| g.write) {
                return Err(err(403, "you cannot write to this space"));
            }
            let f = files::retire(
                &app.brain,
                &space,
                &me.name,
                arg(body, "id")?,
                arg(body, "reason")?,
            )
            .map_err(fail)?;
            Ok(json!({ "file": f.to_json() }))
        }
        _ => Err(err(404, "not found")),
    }
}

/// The bytes of a file the person can read, served inline and sandboxed.
fn file_bytes(app: &App, ctl: &Connection, me: &User, req: &Request) -> Response {
    let (Some(space), Some(id)) = (req.query.get("space"), req.query.get("id")) else {
        return err(400, "space and id are required");
    };
    let Ok(space) = Space::new(space) else {
        return err(400, "invalid space");
    };
    let readable = users::access_of(ctl, &me.id)
        .map(|m| m.contains_key(&space))
        .unwrap_or(false);
    if !readable {
        return err(404, "unknown file");
    }
    match files::get(&app.brain, &space, id) {
        Ok(Some((f, bytes))) => Response {
            status: 200,
            headers: vec![
                ("Content-Type".into(), f.mime.clone()),
                (
                    "Content-Disposition".into(),
                    format!(
                        "{}; filename=\"{}\"",
                        if f.mime == files::XLSX {
                            "attachment"
                        } else {
                            "inline"
                        },
                        f.name.replace(['"', '\\'], "_")
                    ),
                ),
                (
                    "Content-Security-Policy".into(),
                    "sandbox; default-src 'none'; img-src 'self'".into(),
                ),
                ("X-Content-Type-Options".into(), "nosniff".into()),
                ("Cache-Control".into(), "no-store".into()),
            ],
            body: bytes,
        },
        Ok(None) => err(404, "unknown file"),
        Err(e) => fail(e),
    }
}

/// The scope of a guide a person may write: their own (`person`, or `person:ID` for an
/// administrator), or that of a vault they own (an administrator may write any, as operator).
/// Never reachable by an MCP connection: this is the admin page.
fn guide_scope(ctl: &Connection, me: &User, raw: &str) -> Result<String, Response> {
    let scope = if raw == "person" {
        guide::person_scope(&me.id)
    } else {
        raw.to_string()
    };
    if let Some(id) = scope.strip_prefix("person:") {
        if id != me.id && !me.admin {
            return Err(err(
                403,
                "only the person, or an administrator, writes this guide",
            ));
        }
    } else if let Some(name) = scope.strip_prefix("space:") {
        let space = Space::new(name).map_err(fail)?;
        if !me.admin && !users::is_space_owner(ctl, &space, &me.id).map_err(fail)? {
            return Err(err(
                403,
                "only the owner of a vault, or an administrator, writes its guide",
            ));
        }
    } else {
        return Err(err(400, "scope: person, person:ID or space:NAME"));
    }
    Ok(scope)
}

/// The vault whose app guides a person may approve: its owner, or an administrator.
fn guide_owner(ctl: &Connection, me: &User, name: &str) -> Result<Space, Response> {
    let space = Space::new(name).map_err(fail)?;
    if !me.admin && !users::is_space_owner(ctl, &space, &me.id).map_err(fail)? {
        return Err(err(
            403,
            "only the owner of a vault, or an administrator, approves its app guides",
        ));
    }
    Ok(space)
}

/// The guides apps proposed in the vaults this person may approve for.
fn app_guides_json(
    app: &App,
    ctl: &Connection,
    me: &User,
    spaces: &[Space],
) -> Result<Vec<Value>, Error> {
    let mut out = vec![];
    for s in spaces {
        if !me.admin && !users::is_space_owner(ctl, s, &me.id)? {
            continue;
        }
        for g in guide::app_guides(&app.brain, ctl, s)? {
            out.push(json!({
                "space": s.as_str(), "app": g.app, "text": g.text, "hash": g.hash,
                "standing": g.standing.word(), "updated_at": iso(g.updated_at),
                "approved_at": g.approved_at.map(iso),
            }));
        }
    }
    Ok(out)
}

/// `GET /guide?scope=...`: the text in force and how many versions it has had.
fn guide_get(ctl: &Connection, me: &User, req: &Request) -> Response {
    let raw = req.query.get("scope").map(String::as_str).unwrap_or("");
    let scope = match guide_scope(ctl, me, raw) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let build = || -> Result<Value, Error> {
        let now = guide::current(ctl, &scope)?;
        let versions = guide::history(ctl, &scope)?.len();
        Ok(json!({
            "scope": scope,
            "versions": versions,
            "current": now.map(|g| json!({
                "body": g.body, "author": g.author, "written_at": iso(g.created_at),
            })),
        }))
    };
    match build() {
        Ok(v) => Response::json(200, &v),
        Err(e) => fail(e),
    }
}

/// A connection never used after this long, or silent for this long, is worth a look.
const IDLE_NEVER_USED_DAYS: i64 = 7;
const IDLE_SILENT_DAYS: i64 = 30;

/// The latest events of the audit log that concern this person: their connections and
/// what they did. An administrator sees all of it. Never a token, never memory content.
fn activity(
    ctl: &Connection,
    me: &User,
    agents: &std::collections::HashSet<String>,
) -> Result<Vec<Value>, Error> {
    let mut st = ctl.prepare(
        "SELECT ts, coalesce(agent, ''), coalesce(ip, ''), event, coalesce(detail, '')
         FROM auth_audit ORDER BY ts DESC, rowid DESC LIMIT 400",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = vec![];
    for row in rows {
        let (ts, agent, ip, event, detail) = row?;
        if me.admin || agents.contains(&agent) || agent == me.name {
            out.push(
                json!({ "at": iso(ts), "who": agent, "ip": ip, "event": event, "detail": detail }),
            );
        }
        if out.len() == 30 {
            break;
        }
    }
    Ok(out)
}

/// What deserves a look rather than a decision: connections never used or silent for long,
/// and (for an administrator) the keys refused in the last day.
fn attention(ctl: &Connection, me: &User, now: i64) -> Result<Value, Error> {
    let mut idle = vec![];
    for c in access::list(ctl)? {
        if c.revoked_at.is_some() || !(me.admin || c.user_id == me.id) {
            continue;
        }
        let never = c.last_used_at.is_none() && now - c.created_at > IDLE_NEVER_USED_DAYS * 86_400;
        let silent = c
            .last_used_at
            .is_some_and(|t| now - t > IDLE_SILENT_DAYS * 86_400);
        if never || silent {
            idle.push(json!({
                "agent": c.agent, "created_at": iso(c.created_at),
                "last_used_at": c.last_used_at.map(iso),
                "why": if never { "never used" } else { "silent for a long time" },
            }));
        }
    }
    let refused: i64 = if me.admin {
        ctl.query_row(
            "SELECT count(*) FROM auth_audit WHERE event = 'mcp_refused' AND ts > ?1",
            [now - 86_400],
            |r| r.get(0),
        )?
    } else {
        0
    };
    Ok(json!({ "idle": idle, "refused_24h": refused }))
}

/// `GET /export`: the signed-in person's own memory as one tar (see the `export` module).
/// Always the caller's own: an administrator does not export other people's memory here, and
/// no parameter changes whose it is.
fn export_get(app: &App, me: &User) -> Response {
    let work = std::env::temp_dir().join(format!(
        "memcp-export-{}",
        memcp_core::text::hex(&util::random_bytes::<8>())
    ));
    let built = (|| -> Result<(Vec<u8>, String), Error> {
        let dir = export::export_person(&app.brain, me, &work, util::now())?;
        export::verify_export(&dir)?;
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("memcp-export")
            .to_string();
        let tar = crate::tar::pack(&dir, &name, util::now().max(0) as u64)
            .map_err(|e| Error::Invalid(format!("the archive could not be made: {e}")))?;
        Ok((tar, name))
    })();
    let _ = std::fs::remove_dir_all(&work);
    match built {
        Ok((body, name)) => Response {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "application/x-tar".into()),
                (
                    "Content-Disposition".into(),
                    format!("attachment; filename=\"{name}.tar\""),
                ),
                ("Cache-Control".into(), "no-store".into()),
            ],
            body,
        },
        Err(e) => fail(e),
    }
}

/// Counters of a vault, never its content: active facts, active files, last write.
fn vault_stats(app: &App, space: &Space) -> Value {
    let Ok(conn) = app.brain.open_space(space) else {
        return Value::Null;
    };
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let last = conn
        .query_row("SELECT max(created_at) FROM facts", [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .ok()
        .flatten();
    json!({
        "facts": count("SELECT count(*) FROM facts WHERE status = 'active'"),
        "files": count("SELECT count(*) FROM files WHERE status = 'active'"),
        "last_write": last.map(iso),
    })
}

/// Number of bars on the timeline of a vault.
const TIMELINE_BUCKETS: i64 = 48;
const TIMELINE_MAX_HOURS: i64 = 24 * 30;

/// How many facts each vault of the person received, per slice of the last `hours` hours
/// (`?hours=`, 1 to 720, 24 by default): the slices are `hours * 75` seconds wide, 48 of them,
/// the last one ending now. Counts only, never content. A fact counts when it was added, whatever
/// became of it since.
fn timeline(app: &App, ctl: &Connection, me: &User, req: &Request, now: i64) -> Response {
    let hours = req
        .query
        .get("hours")
        .and_then(|h| h.parse::<i64>().ok())
        .unwrap_or(24)
        .clamp(1, TIMELINE_MAX_HOURS);
    let width = hours * 3600 / TIMELINE_BUCKETS;
    let from = now - width * TIMELINE_BUCKETS;
    let build = || -> Result<Value, Error> {
        let reach = users::access_of(ctl, &me.id)?;
        let triage = memcp_core::triage::of(&app.brain, &me.id)?;
        let mut vaults = vec![];
        for s in app.brain.spaces()? {
            if !reach.contains_key(&s) || Some(&s) == triage.as_ref() {
                continue;
            }
            let mut counts = vec![0_i64; TIMELINE_BUCKETS as usize];
            let conn = app.brain.open_space(&s)?;
            let mut st = conn.prepare(
                "SELECT (created_at - ?1) / ?2, count(*) FROM facts
                 WHERE created_at >= ?1 GROUP BY 1",
            )?;
            let rows = st.query_map([from, width], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (b, n) = row?;
                // A fact of this very second (or a clock a little ahead) lands in the last slice.
                let last = TIMELINE_BUCKETS as usize - 1;
                if let Ok(i) = usize::try_from(b) {
                    counts[i.min(last)] += n;
                }
            }
            vaults.push(json!({ "name": s.as_str(), "counts": counts }));
        }
        Ok(json!({ "hours": hours, "from": iso(from), "to": iso(now),
            "bucket_secs": width, "vaults": vaults }))
    };
    match build() {
        Ok(v) => Response::json(200, &v),
        Err(e) => fail(e),
    }
}

fn user_json(u: &User) -> Value {
    json!({
        "id": u.id, "name": u.name, "display": u.display, "admin": u.admin,
        "home_assistant": u.ha_user.is_some(), "created_at": iso(u.created_at), "disabled": u.disabled,
    })
}

/// What the page shows: the person's own memory, and, for an administrator, the people.
fn state(app: &App, ctl: &Connection, me: &User) -> Response {
    let build = || -> Result<Value, Error> {
        let all_users = users::list_users(ctl)?;
        let visible = |id: &str| id == me.id;
        let reach = users::access_of(ctl, &me.id)?;
        let triage = memcp_core::triage::of(&app.brain, &me.id)?;
        let spaces: Vec<Value> = app
            .brain
            .spaces()?
            .into_iter()
            .filter(|s| reach.contains_key(s) && Some(s) != triage.as_ref())
            .map(|s| {
                let kind = app.brain.space_kind(&s).unwrap_or_default();
                let has_guide = guide::current(ctl, &guide::space_scope(&s))
                    .ok()
                    .flatten()
                    .is_some();
                json!({ "name": s.as_str(), "kind": kind, "guide": has_guide,
                    "stats": vault_stats(app, &s) })
            })
            .collect();
        let connections: Vec<Value> = access::list(ctl)?
            .into_iter()
            .filter(|c| visible(&c.user_id))
            .map(|c| {
                json!({ "agent": c.agent, "user": c.user_id, "rights": c.rights,
                    "read_only": c.read_only_forced, "revoked": c.revoked_at.is_some(),
                    "person_guide": !c.no_person_guide,
                    "created_at": iso(c.created_at), "last_used_at": c.last_used_at.map(iso),
                    "expires_at": c.expires_at.map(iso) })
            })
            .collect();
        let mut kept: Vec<Value> = vec![];
        for s in reach.keys() {
            for f in files::list(&app.brain, s, None, 30)? {
                kept.push(f.to_json());
            }
        }
        kept.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
        Ok(json!({
            "files": kept,
            "me": user_json(me),
            "app_guides": app_guides_json(app, ctl, me, &reach.keys().cloned().collect::<Vec<_>>())?,
            "my_guide": guide::current(ctl, &guide::person_scope(&me.id))?.is_some(),
            "activity": activity(ctl, me, &connections.iter().filter_map(|c| c["agent"].as_str().map(String::from)).collect())?,
            "attention": attention(ctl, me, util::now())?,
            // Names are what the login claims: shown as untrusted text.
            "requests": if me.admin {
                users::list_requests(ctl)?.iter().map(|r| json!({
                    "ha_user": r.ha_user, "name": r.name, "display": r.display,
                    "first_seen": iso(r.first_seen), "last_seen": iso(r.last_seen), "status": r.status,
                })).collect::<Vec<_>>()
            } else {
                vec![]
            },
            "public_url": app.public_url,
            "node": { "name": app.node_name, "version": env!("CARGO_PKG_VERSION") },
            "catalogue": app.brain.catalogue()?.iter().map(|k| json!({
                "key": k.key, "label": k.label, "description": k.description,
                "feature": k.feature.map(|f| f.word()),
            })).collect::<Vec<_>>(),
            // An administrator sees the people; anyone else, only themselves.
            "users": all_users.iter().filter(|u| me.admin || u.id == me.id).map(|u| {
                let mut j = user_json(u);
                j["spaces"] = json!(users::access_of(ctl, &u.id)
                    .map(|m| m.keys().map(ToString::to_string).collect::<Vec<_>>())
                    .unwrap_or_default());
                j
            }).collect::<Vec<_>>(),
            "spaces": spaces,
            // What waits in « À trier », for the person to decide when the janitor did not.
            "triage": match &triage {
                Some(t) => memcp_core::triage::waiting(&app.brain, t, 300)?,
                None => Value::Null,
            },
            "connections": connections,
        }))
    };
    match build() {
        Ok(v) => Response::json(200, &v),
        Err(e) => fail(e),
    }
}
