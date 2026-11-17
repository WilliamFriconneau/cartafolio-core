//! Joining by request: a device that found this node (see discovery.rs) asks to join. The person sees the request on
//! their page, and in the notifications of their apps, with a **check phrase** of three words that the device shows
//! too; they compare the two, and allow or deny. Nothing is typed, and a device cannot join without a person saying yes.
//!
//! A request lives in memory only, for two minutes. The key of an allowed request is handed once to the device that holds
//! the secret of the request, then forgotten.

use std::collections::HashMap;

use memcp_core::users::User;
use memcp_core::{pairing, util};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Request, Response};

/// How long a request waits for an answer.
pub const TTL: i64 = 120;
/// Requests waiting at once, from anyone.
const MAX_PENDING: usize = 8;
/// Requests a minute from one address.
const PER_MIN: i64 = 5;

const WORDS: [&str; 64] = [
    "amber", "anchor", "apple", "arrow", "badge", "basil", "beach", "birch", "bronze", "candle",
    "cedar", "cliff", "cloud", "comet", "coral", "daisy", "delta", "ember", "falcon", "fern",
    "flint", "forest", "garnet", "glade", "harbor", "hazel", "indigo", "island", "jasper",
    "juniper", "lantern", "lemon", "lotus", "maple", "meadow", "mint", "moss", "nectar", "oasis",
    "olive", "orchid", "pearl", "pepper", "pine", "planet", "quartz", "raven", "river", "saffron",
    "sage", "sierra", "silver", "spruce", "summit", "tiger", "tulip", "velvet", "violet", "willow",
    "winter", "yarrow", "zephyr", "zinc", "marble",
];

pub enum State {
    Waiting,
    Denied,
    Approved(Value),
}

pub struct Join {
    device: String,
    check: String,
    secret: String,
    created: i64,
    state: State,
}

fn refused(status: u16, message: &str) -> Response {
    Response::json(status, &json!({ "error": message }))
}

fn clean_device(raw: &str) -> String {
    let d: String = raw.chars().filter(|c| !c.is_control()).take(60).collect();
    let d = d.trim().to_string();
    if d.is_empty() { "a device".into() } else { d }
}

/// Three words, each one of 64 chosen evenly (a random byte modulo 64 has no bias: 256 is a multiple of it).
pub fn phrase() -> String {
    util::random_bytes::<3>()
        .iter()
        .map(|b| WORDS[usize::from(*b) % WORDS.len()])
        .collect::<Vec<_>>()
        .join("-")
}

fn sweep(joins: &mut HashMap<String, Join>, now: i64) {
    joins.retain(|_, j| now - j.created <= TTL * 3);
}

/// `POST /join` {device}: a device asks to join. Answers its id, the secret it polls with and the check phrase.
pub fn request(app: &App, req: &Request) -> Response {
    let now = util::now();
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) || !app.rate_ok(now, &format!("join:{who}"), PER_MIN) {
        return refused(429, "too many tries, wait a minute").with("Retry-After", "60");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return refused(400, "JSON expected: {\"device\": \"name\"}");
    };
    let device = clean_device(body.get("device").and_then(Value::as_str).unwrap_or(""));
    // A ticket made by a peer of the same person: the device joins at once, once, with a key of its own.
    let ticket_owner = match body.get("ticket").and_then(Value::as_str) {
        Some(t) => {
            let key = ticket_key(t);
            let mut tickets = app.tickets.lock().unwrap_or_else(|e| e.into_inner());
            tickets.retain(|_, (_, _, until)| *until > now);
            match tickets.remove(&key) {
                Some((user_id, user_name, _)) => Some((user_id, user_name)),
                None => {
                    drop(tickets);
                    app.fail(now, &who);
                    return refused(403, "this ticket is unknown, used or expired");
                }
            }
        }
        None => None,
    };
    let mut joins = app.joins.lock().unwrap_or_else(|e| e.into_inner());
    sweep(&mut joins, now);
    let waiting = joins
        .values()
        .filter(|j| matches!(j.state, State::Waiting) && now - j.created <= TTL)
        .count();
    if waiting >= MAX_PENDING {
        return refused(
            429,
            "too many requests are waiting, try again in a few minutes",
        );
    }
    let id = util::random_base32(12);
    let secret = util::random_base32(24);
    let check = phrase();
    // A device the person allowed in advance joins at once, once: no words to compare.
    let allowed = match ticket_owner {
        Some((id, name)) => Some((id, name, now)),
        None => {
            let mut a = app
                .allowed_devices
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            a.retain(|_, (_, _, until)| *until > now);
            a.remove(&device.to_lowercase())
        }
    };
    let mut state = State::Waiting;
    if let Some((user_id, user_name, _)) = allowed
        && let Ok(ctl) = app.brain.open_control()
        && let Ok((code, _)) = pairing::create_for_peer(&app.brain, &user_id, &user_name, now)
        && let Ok(paired) = pairing::redeem(&ctl, &code, &device, now)
    {
        memcp_core::access::auth_event(&ctl, Some(&paired.agent), None, "joined", &device);
        state = State::Approved(json!({
            "agent": paired.agent,
            "key": paired.token,
            "mcp_url": format!("{}/mcp/{}", app.public_url, paired.agent),
            "spaces": paired.spaces,
        }));
    }
    joins.insert(
        id.clone(),
        Join {
            device,
            check: check.clone(),
            secret: secret.clone(),
            created: now,
            state,
        },
    );
    Response::json(
        200,
        &json!({ "id": id, "secret": secret, "check": check, "expires_in": TTL }),
    )
}

/// `GET /join/<id>` with `X-Join-Secret`: where the request stands. An allowed one hands its key, once.
pub fn poll(app: &App, req: &Request, id: &str) -> Response {
    let now = util::now();
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if !app.rate_ok(now, &format!("joinpoll:{who}"), 120) {
        return refused(429, "too many tries").with("Retry-After", "10");
    }
    let secret = req
        .headers
        .get("x-join-secret")
        .map(String::as_str)
        .unwrap_or("");
    let mut joins = app.joins.lock().unwrap_or_else(|e| e.into_inner());
    let Some(j) = joins.get(id) else {
        return refused(404, "unknown request");
    };
    if !util::ct_eq(j.secret.as_bytes(), secret.as_bytes()) {
        app.fail(now, &who);
        return refused(404, "unknown request");
    }
    if now - j.created > TTL && matches!(j.state, State::Waiting) {
        joins.remove(id);
        return Response::json(200, &json!({ "state": "expired" }));
    }
    match &j.state {
        State::Waiting => Response::json(200, &json!({ "state": "waiting" })),
        State::Denied => {
            joins.remove(id);
            Response::json(200, &json!({ "state": "denied" }))
        }
        State::Approved(key) => {
            let out = json!({ "state": "approved", "join": key });
            joins.remove(id);
            Response::json(200, &out)
        }
    }
}

/// What a signed-in person sees: the requests waiting, with their check phrases.
pub fn pending(app: &App, _me: &User) -> Response {
    let now = util::now();
    let mut joins = app.joins.lock().unwrap_or_else(|e| e.into_inner());
    sweep(&mut joins, now);
    let list: Vec<Value> = joins
        .iter()
        .filter(|(_, j)| matches!(j.state, State::Waiting) && now - j.created <= TTL)
        .map(|(id, j)| {
            json!({ "id": id, "device": j.device, "check": j.check, "waited": now - j.created, "expires_in": TTL - (now - j.created) })
        })
        .collect();
    Response::json(200, &json!({ "requests": list }))
}

/// `POST /join/approve` and `/join/deny` {id}: the person's answer. Allowing makes a connection of its own for the
/// device, reading and writing every space of the person, as a peer.
pub fn decide(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    body: &Value,
) -> Result<Value, Response> {
    let now = util::now();
    if path == "/join/allow-device" {
        // The person lets a device come back (a reinstalled phone) without the three words, for a while, once.
        let device = clean_device(body.get("device").and_then(Value::as_str).unwrap_or(""));
        let minutes = body
            .get("minutes")
            .and_then(Value::as_i64)
            .unwrap_or(30)
            .clamp(1, 1440);
        let mut a = app
            .allowed_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        a.retain(|_, (_, _, until)| *until > now);
        if a.len() >= 16 {
            return Err(refused(429, "too many devices are allowed at once"));
        }
        a.insert(
            device.to_lowercase(),
            (me.id.clone(), me.name.clone(), now + minutes * 60),
        );
        return Ok(json!({ "result": "allowed", "device": device, "minutes": minutes }));
    }
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| refused(400, "id is required"))?;
    let mut joins = app.joins.lock().unwrap_or_else(|e| e.into_inner());
    let Some(j) = joins.get_mut(id) else {
        return Err(refused(404, "this request is gone"));
    };
    if !matches!(j.state, State::Waiting) || now - j.created > TTL {
        return Err(refused(404, "this request is gone"));
    }
    if path == "/join/deny" {
        j.state = State::Denied;
        return Ok(json!({ "result": "denied" }));
    }
    let (code, _) = pairing::create_for_peer(&app.brain, &me.id, &me.name, now)
        .map_err(|e| refused(400, &e.to_string()))?;
    let paired =
        pairing::redeem(ctl, &code, &j.device, now).map_err(|e| refused(400, &e.to_string()))?;
    memcp_core::access::auth_event(ctl, Some(&paired.agent), None, "joined", &j.device);
    j.state = State::Approved(json!({
        "agent": paired.agent,
        "key": paired.token,
        "mcp_url": format!("{}/mcp/{}", app.public_url, paired.agent),
        "spaces": paired.spaces,
    }));
    Ok(json!({ "result": "approved", "connection": paired.agent }))
}

/// Whether the ticket of a `POST /join` body is held, valid, by this installation (the manager routes the request to it).
pub(crate) fn holds_ticket(app: &App, body: &[u8]) -> bool {
    let Some(t) = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|b| b.get("ticket").and_then(Value::as_str).map(str::to_string))
    else {
        return false;
    };
    let now = util::now();
    app.tickets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&ticket_key(&t))
        .is_some_and(|(_, _, until)| *until > now)
}

/// The hash a ticket is kept under (the ticket itself is never stored).
fn ticket_key(ticket: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("join-ticket:{}", ticket.trim()).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// How long an invitation ticket lives, in seconds.
pub const TICKET_TTL: i64 = 600;
/// Tickets waiting at once, from all peers.
const MAX_TICKETS: usize = 16;

/// `POST /mcp/<connection>/join/invite` {device?}, with the key of a PEER of the person: a one-time ticket that lets
/// another device of the same person join at once (`POST /join` {device, ticket}), without the page or the three words,
/// with a key of its own that can be revoked alone. The ticket is shown once, lives ten minutes, is kept only as a hash,
/// and is never the peer's own key. Only a peer (a connection that reads and writes the person's spaces) may invite.
pub fn invite(app: &App, req: &Request, now: i64, agent: &str) -> Response {
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) {
        return refused(429, "too many attempts").with("Retry-After", "900");
    }
    let Ok(ctl) = app.brain.open_control() else {
        return refused(500, "storage failed");
    };
    let Some(caller) = crate::http::bearer_of(req.header("authorization")).and_then(|t| {
        memcp_core::access::authenticate(&ctl, &t, now)
            .ok()
            .flatten()
    }) else {
        app.fail(now, &who);
        return refused(401, "refused");
    };
    if caller.agent != agent {
        return refused(403, "this key does not reach this address");
    }
    if !caller.agent.starts_with("peer-") || caller.is_scoped() || caller.read_only_forced {
        return refused(403, "only a peer of the person may invite a device");
    }
    if !app.rate_ok(now, &format!("invite:{}", caller.id), 6) {
        return refused(429, "too many invitations, wait a minute").with("Retry-After", "60");
    }
    let name = memcp_core::users::user(&ctl, &caller.user_id)
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_default();
    let ticket = util::random_base32(24);
    {
        let mut t = app.tickets.lock().unwrap_or_else(|e| e.into_inner());
        t.retain(|_, (_, _, until)| *until > now);
        if t.len() >= MAX_TICKETS {
            return refused(429, "too many invitations are waiting");
        }
        t.insert(
            ticket_key(&ticket),
            (caller.user_id.clone(), name, now + TICKET_TTL),
        );
    }
    memcp_core::access::auth_event(&ctl, Some(&caller.agent), None, "join_invite", "");
    Response::json(
        200,
        &json!({ "ticket": ticket, "expires_in": TICKET_TTL, "single_use": true }),
    )
}

/// `GET /mcp/<connection>/join/pending` and `POST /mcp/<connection>/join/approve|deny` {id}, with the key of a PEER of
/// the person: every node of the person sees the devices asking to join and can answer, not only the one they asked
/// (all nodes are equal). The same answer as the page's: allowing makes the device a peer of its own.
pub fn by_peer(app: &App, req: &Request, now: i64, agent: &str, op: &str) -> Response {
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) {
        return refused(429, "too many attempts").with("Retry-After", "900");
    }
    let Ok(ctl) = app.brain.open_control() else {
        return refused(500, "storage failed");
    };
    let Some(caller) = crate::http::bearer_of(req.header("authorization")).and_then(|t| {
        memcp_core::access::authenticate(&ctl, &t, now)
            .ok()
            .flatten()
    }) else {
        app.fail(now, &who);
        return refused(401, "refused");
    };
    if caller.agent != agent {
        return refused(403, "this key does not reach this address");
    }
    if !caller.agent.starts_with("peer-") || caller.is_scoped() || caller.read_only_forced {
        return refused(403, "only a peer of the person may answer a device");
    }
    let Ok(Some(me)) = memcp_core::users::user(&ctl, &caller.user_id) else {
        return refused(403, "no person behind this key");
    };
    match (req.method.as_str(), op) {
        ("GET", "pending") => pending(app, &me),
        ("POST", "approve" | "deny") => {
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
                return refused(400, "JSON expected: {\"id\": \"…\"}");
            };
            match decide(app, &ctl, &me, &format!("/join/{op}"), &body) {
                Ok(v) => Response::json(200, &v),
                Err(r) => r,
            }
        }
        _ => refused(404, "not found"),
    }
}
