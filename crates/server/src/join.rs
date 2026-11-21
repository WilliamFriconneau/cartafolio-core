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

/// How long a request waits for an answer by default.
pub const TTL: i64 = 120;
/// What the tests set (`MEMCP_JOIN_TTL`, seconds, 2 to 3600) to see a request expire without waiting two minutes.
const TTL_VAR: &str = "MEMCP_JOIN_TTL";

/// How long a request waits for an answer on this node.
pub fn ttl() -> i64 {
    static T: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        memcp_core::env::var(TTL_VAR)
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .map_or(TTL, |v| v.clamp(2, 3600))
    })
}
/// Requests waiting at once, from anyone.
const MAX_PENDING: usize = 64;
/// Requests waiting at once from one address (an IPv6 client counts as its /64): one device on the network cannot fill
/// the list and hide the request of the person's own device.
const MAX_PENDING_PER_IP: usize = 3;
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
    /// Who asked (the address, cut to its /64 for IPv6), for the cap on requests waiting from one address.
    from: String,
    /// The id of the node that asks, when it says it.
    node_id: Option<String>,
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
    joins.retain(|_, j| now - j.created <= ttl() * 3);
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
    let node_id = body
        .get("node_id")
        .and_then(Value::as_str)
        .and_then(memcp_core::directory::valid_id);
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
    let from = App::guard_key(&who);
    let waiting: Vec<&Join> = joins
        .values()
        .filter(|j| matches!(j.state, State::Waiting) && now - j.created <= ttl())
        .collect();
    if ticket_owner.is_none()
        && (waiting.len() >= MAX_PENDING
            || waiting.iter().filter(|j| j.from == from).count() >= MAX_PENDING_PER_IP)
    {
        return refused(
            429,
            "too many requests are waiting, try again in a few minutes",
        );
    }
    let id = util::random_base32(12);
    let secret = util::random_base32(24);
    let check = phrase();
    let mut state = State::Waiting;
    if let Some((user_id, user_name)) = ticket_owner
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
            from,
            node_id,
            check: check.clone(),
            secret: secret.clone(),
            created: now,
            state,
        },
    );
    Response::json(
        200,
        &json!({ "id": id, "secret": secret, "check": check, "expires_in": ttl() }),
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
    if now - j.created > ttl() && matches!(j.state, State::Waiting) {
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

/// The requests waiting on THIS node, with their check phrases (what a peer is told, and what the page shows first).
pub fn pending_here(app: &App) -> Vec<Value> {
    let now = util::now();
    let mut joins = app.joins.lock().unwrap_or_else(|e| e.into_inner());
    sweep(&mut joins, now);
    let mut list: Vec<(i64, Value)> = joins
        .iter()
        .filter(|(_, j)| matches!(j.state, State::Waiting) && now - j.created <= ttl())
        .map(|(id, j)| {
            (j.created, json!({ "id": id, "device": j.device, "check": j.check, "node_id": j.node_id, "waited": now - j.created, "expires_in": ttl() - (now - j.created) }))
        })
        .collect();
    list.sort_by_key(|(t, _)| *t);
    list.into_iter().map(|(_, v)| v).collect()
}

/// What a signed-in person sees: the requests waiting here, and the ones the peers of this node told, each with its
/// check phrase (`node` names the node that was asked, for the second kind).
pub fn pending(app: &App, _me: &User) -> Response {
    let mut list = pending_here(app);
    list.extend(memcp_core::directory::remote_joins(&app.brain, util::now()).unwrap_or_default());
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
        // The person lets a device come back (a reinstalled phone) without the three words: a one-time ticket, shown to
        // the person only, that the device gives with its request. A device name is no secret (any device on the
        // network may say it), so it lets nothing in by itself.
        let device = clean_device(body.get("device").and_then(Value::as_str).unwrap_or(""));
        let minutes = body
            .get("minutes")
            .and_then(Value::as_i64)
            .unwrap_or(TICKET_TTL / 60)
            .clamp(1, TICKET_TTL / 60);
        let ticket = make_ticket_for(app, &me.id, &me.name, now, minutes * 60)?;
        let link = memcp_core::directory::own_link(&app.brain, &app.public_url, Some(&ticket))
            .map(|l| l.to_link())
            .ok();
        return Ok(json!({
            "result": "allowed", "device": device, "minutes": minutes,
            "ticket": ticket, "link": link, "expires_in": minutes * 60, "single_use": true,
        }));
    }
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| refused(400, "id is required"))?;
    let mut joins = app.joins.lock().unwrap_or_else(|e| e.into_inner());
    let local = joins
        .get(id)
        .is_some_and(|j| matches!(j.state, State::Waiting) && now - j.created <= ttl());
    if !local {
        drop(joins);
        // A request that another node of the person holds: the answer is kept and goes to that node with the next
        // exchange; the request leaves this list at once.
        let result = if path == "/join/deny" {
            "denied"
        } else {
            "approved"
        };
        return match memcp_core::directory::answer_remote_join(&app.brain, id, result, now) {
            Ok(Some(node)) => Ok(json!({ "result": result, "via": node, "queued": true })),
            _ => Err(refused(404, "this request is gone")),
        };
    }
    let Some(j) = joins.get_mut(id) else {
        return Err(refused(404, "this request is gone"));
    };
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
    if !caller.is_peer() {
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
    let ticket = match make_ticket(app, &caller.user_id, &name, now) {
        Ok(t) => t,
        Err(r) => return r,
    };
    memcp_core::access::auth_event(&ctl, Some(&caller.agent), None, "join_invite", "");
    let link = memcp_core::directory::own_link(&app.brain, &app.public_url, Some(&ticket))
        .map(|l| l.to_link())
        .ok();
    Response::json(
        200,
        &json!({ "ticket": ticket, "link": link, "expires_in": TICKET_TTL, "single_use": true }),
    )
}

/// A one-time ticket for `person`: kept as a hash, valid [`TICKET_TTL`] seconds.
pub fn make_ticket(app: &App, user_id: &str, person: &str, now: i64) -> Result<String, Response> {
    make_ticket_for(app, user_id, person, now, TICKET_TTL)
}

/// A one-time ticket for `person`, valid `secs` seconds.
fn make_ticket_for(
    app: &App,
    user_id: &str,
    person: &str,
    now: i64,
    secs: i64,
) -> Result<String, Response> {
    let ticket = util::random_base32(24);
    let mut t = app.tickets.lock().unwrap_or_else(|e| e.into_inner());
    t.retain(|_, (_, _, until)| *until > now);
    if t.len() >= MAX_TICKETS {
        return Err(refused(429, "too many invitations are waiting"));
    }
    t.insert(
        ticket_key(&ticket),
        (user_id.to_string(), person.to_string(), now + secs),
    );
    Ok(ticket)
}

/// The answers a peer of the person gives (in `POST /sync/peers`, `answers: [{id, result}]`) to the requests this node
/// told it about: the same as the page's click. An answer to a request that is gone (answered elsewhere, expired) is
/// ignored.
pub fn apply_answers(app: &App, ctl: &Connection, me: &User, answers: &[Value]) {
    for a in answers.iter().take(16) {
        let (Some(id), Some(result)) = (a["id"].as_str(), a["result"].as_str()) else {
            continue;
        };
        let path = match result {
            "approved" => "/join/approve",
            "denied" => "/join/deny",
            _ => continue,
        };
        let _ = decide(app, ctl, me, path, &json!({ "id": id }));
    }
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
    if !caller.is_peer() {
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
