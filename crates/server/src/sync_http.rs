//! The synchronisation between peers over HTTP, at the address of the peer's own connection:
//!
//! - `GET  /mcp/<connection>/sync/status`: every peer of this memory, its direction, progress per space and last activity
//!   (what the apps show live).
//! - `GET  /mcp/<connection>/sync/peers`: this node's peer directory (`{node, peers: [{name, last_seen, version, ...}]}`).
//! - `POST /mcp/<connection>/sync/peers` `{node: {name}, peers: [...]}`: the peer's directory, merged here.
//! - `GET  /mcp/<connection>/sync/spaces`: the spaces this key reaches, with their kind, layout, the time the copy began
//!   and the head of their log; which one is the person's Inbox; the archives this node knows of; whether it takes
//!   offers.
//! - `POST /mcp/<connection>/sync/spaces` `{space, kind, label, parent, position, since}`: a space of the person that the
//!   peer has and this node lacks (see `memcp_core::swarm`); or `{space, archived_at}`: the peer archived it.
//! - `GET  /mcp/<connection>/sync/events?space=S&after=N[&limit=L][&chunks=1]`: the events of a space after the asker's
//!   cursor, never the ones it sent itself. With `chunks=1` (an asker that takes chunks) a file bigger than travels whole
//!   comes as the list of its chunks; without it, such a file is left out.
//! - `POST /mcp/<connection>/sync/events?space=S` `{events: [...]}`: events from the peer, applied here idempotently; the
//!   answer says which were dealt with (`consumed`), which of those this node did not keep (`not_kept`, with why), and
//!   the chunks of a big file it waits for (`wants: {file, missing: [{n, hash}]}`).
//! - `GET  /mcp/<connection>/sync/chunk?space=S&file=SHA&n=N[&hash=H]`: chunk `n` of a file of the space, raw bytes,
//!   from the whole file or from the chunks this node keeps aside (404 when it has neither).
//! - `POST /mcp/<connection>/sync/chunk?space=S&file=SHA&hash=H` (raw bytes): a chunk of a file this node waits for,
//!   kept only when the bytes match `hash`.
//!
//! Only the connection's own key is let in, and only the key of a machine of the person (a connection made by a pairing
//! code or a join request, `access::Kind::Peer`): an assistant's key, whatever its name, is refused everywhere here.
//! The status, the directory and the offers of spaces need a full peer (`Caller::is_peer`: every space of the person,
//! whole, may write); the log of a space (`spaces` read, `events`) is open to any machine of the person on the spaces
//! it reaches (read for pulling, write for pushing), so a machine the person limited to some spaces keeps in step with
//! those. The engine is `memcp_core::sync`; nothing here decides what a merge does.
//!
//! This node's sync policy for the machine of the key (`memcp_core::policy`, under the connection's name) holds on every
//! route: a space set `off` is not listed, a space it does not give is not served (`events` read: 403), one it does not
//! take is not written (`events` write: 403; an offer is `declined`, an archive `kept`), whatever the machine sends.

use memcp_core::policy::{self, Policy};
use memcp_core::{Space, access, directory, swarm, sync};
use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Request, Response};

/// Events per page, and bytes, so a first copy of a large space goes in steps a progress bar can follow.
const PAGE_EVENTS: usize = 500;
const PAGE_BYTES: usize = 4 << 20;

fn refused(status: u16, message: &str) -> Response {
    Response::json(status, &json!({ "error": message }))
}

pub fn handle(app: &App, req: &Request, now: i64, agent: &str, op: &str) -> Response {
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) {
        return refused(429, "too many attempts").with("Retry-After", "900");
    }
    let brain = &app.brain;
    let Ok(ctl) = brain.open_control() else {
        return refused(500, "storage failed");
    };
    let token = crate::http::bearer_of(req.header("authorization"));
    let Some(caller) = token.and_then(|t| access::authenticate(&ctl, &t, now).ok().flatten())
    else {
        app.fail(now, &who);
        return refused(401, "refused");
    };
    // The address names a connection: only that connection's key.
    if caller.agent != agent {
        return refused(403, "this key does not reach this address");
    }
    // Only a machine of the person syncs; the rest of what is served here needs a full peer.
    let full_peer = caller.is_peer();
    let log_only = matches!(
        (req.method.as_str(), op),
        ("GET", "spaces" | "events" | "chunk") | ("POST", "events" | "chunk")
    );
    if caller.kind != access::Kind::Peer || !(full_peer || log_only) {
        return refused(403, "only a machine of the person syncs");
    }
    let _ = access::touch(&ctl, &caller, crate::app::client_seen(req).as_deref(), now);
    // Serving a request is activity: this node's own entry in the peer directory says so.
    let _ = directory::touch_self(brain, now);
    if !app.rate_ok(now, &format!("sync:{}", caller.id), caller.rate_per_min) {
        return refused(429, "rate limit exceeded").with("Retry-After", "60");
    }
    // The log is the whole vault: a vault reached only through tag paths is not the peer's to copy or to write.
    let whole = |s: &Space| caller.read_paths(s).is_none();
    // A machine the person paused is not served until they resume it (its status stays readable).
    if op != "status" && memcp_core::peers::is_paused(&ctl, &caller.agent) {
        return refused(423, "the sync with this machine is paused");
    }
    let space = || -> Result<Space, Response> {
        let name = req.query.get("space").map(String::as_str).unwrap_or("");
        Space::new(name).map_err(|_| refused(400, "space: a space name"))
    };
    match (req.method.as_str(), op) {
        ("GET", "status") => match status_body(brain, now, &caller.user_id) {
            Ok(body) => Response::json(200, &body),
            Err(e) => refused(500, &e),
        },
        // The peer directory, as this node tells it: its own entry first-hand, the others as it learned them.
        ("GET", "peers") => match (directory::node_json(brain, now), directory::to_wire(brain)) {
            (Ok(mut node), Ok(peers)) => {
                // The devices asking to join this node: told to a peer of the person, who may answer them (join.rs).
                node["pending"] = json!(crate::join::pending_here(app));
                Response::json(200, &json!({ "node": node, "peers": peers }))
            }
            (Err(e), _) | (_, Err(e)) => refused(500, &e.to_string()),
        },
        // A peer tells its directory: merged here, and the peer is known as the node behind this connection.
        ("POST", "peers") => {
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
                return refused(400, "JSON expected: {\"node\": {...}, \"peers\": [...]}");
            };
            let from = body["node"]["name"].as_str().unwrap_or(&caller.agent);
            let list = body["peers"].as_array().cloned().unwrap_or_default();
            // What the person answered, on the peer, to the requests this node told it about.
            if let Some(answers) = body["answers"].as_array()
                && let Ok(Some(me)) = memcp_core::users::user(&ctl, &caller.user_id)
            {
                crate::join::apply_answers(app, &ctl, &me, answers);
            }
            // The machine's own side of the sync policy, for the page of this node (it decides nothing here).
            if let Some(sides) = body["policy"].as_object() {
                note_sides(&ctl, &caller, sides);
            }
            match directory::merge(brain, from, &list, now) {
                Ok(changed) => {
                    if let Some(name) = body["node"]["name"].as_str() {
                        let _ = directory::set_link(brain, name, &caller.agent);
                    }
                    Response::json(200, &json!({ "merged": changed }))
                }
                Err(e) => refused(400, &e.to_string()),
            }
        }
        ("GET", "spaces") => {
            let mut out = Vec::new();
            // The person's own box: the one a peer pairs with its own (see `memcp_core::swarm`).
            let triage = memcp_core::triage::of(brain, &caller.user_id)
                .ok()
                .flatten();
            for s in brain.spaces().unwrap_or_default() {
                if !caller.can_read(&s) || !whole(&s) {
                    continue;
                }
                // A space the person set apart from this machine here is not listed to it at all.
                let side = side(&ctl, &caller, &s);
                if side == Policy::Off {
                    continue;
                }
                type Layout = (String, Option<String>, Option<String>, i64);
                let (kind, label, parent, position): Layout = ctl
                    .query_row(
                        "SELECT kind, label, parent, position FROM spaces WHERE name = ?1",
                        [s.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .unwrap_or_else(|_| ("custom".into(), None, None, 0));
                let head = sync::head(brain, &s).unwrap_or(0);
                out.push(json!({
                    "space": s.as_str(), "kind": kind, "head": head,
                    // Writable: the key may write it, and this node takes it from this machine.
                    "writable": caller.can_write(&s) && side.takes(),
                    // This node's side for this machine: an older peer ignores it, a newer one pulls only when it
                    // gives, and shows it on its page.
                    "policy": side.as_str(),
                    "label": label, "parent": parent, "position": position,
                    "since": swarm::since(brain, &s).unwrap_or(0),
                    "triage": triage.as_ref() == Some(&s),
                }));
            }
            let archived: Vec<Value> = swarm::tombstones(brain, &caller.user_id)
                .unwrap_or_default()
                .into_iter()
                .filter(|(s, _)| side(&ctl, &caller, s).gives())
                .map(|(s, at)| json!({ "space": s.as_str(), "archived_at": at }))
                .collect();
            // The exchange's protocol and this node's version: machines of different versions keep in step as long as
            // they speak the same protocol; a newer protocol is refused by an older node, which says so and waits.
            Response::json(
                200,
                &json!({
                    "protocol": sync::PROTOCOL, "version": env!("CARGO_PKG_VERSION"), "spaces": out,
                    "offers": full_peer, "archived": archived,
                    // Big files go in chunks of this size, both ways; a node of an earlier version says nothing here.
                    "chunks": app.file_chunks.then_some(memcp_core::chunks::CHUNK_BYTES),
                    "later": policy::later(&ctl, &caller.agent).unwrap_or(Policy::Both).as_str(),
                }),
            )
        }
        // A space of the person that the peer has and this node lacks, or one the peer archived.
        ("POST", "spaces") => {
            if !full_peer {
                return refused(
                    403,
                    "only a peer of the person that reaches all their spaces offers a space",
                );
            }
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
                return refused(400, "JSON expected: {\"space\": ..., \"since\": ...}");
            };
            if let Some(at) = body.get("archived_at").and_then(Value::as_i64) {
                let Ok(space) = Space::new(body["space"].as_str().unwrap_or_default()) else {
                    return refused(400, "space: a space name");
                };
                return match swarm::archive_from(brain, &caller.user_id, &caller.agent, &space, at)
                {
                    Ok(done) => Response::json(
                        200,
                        &json!({ "result": if done { "archived" } else { "kept" }, "space": space.as_str() }),
                    ),
                    Err(e) => refused(400, &e.to_string()),
                };
            }
            let offer = match swarm::Offer::from_json(&body) {
                Ok(o) => o,
                Err(e) => return refused(400, &e.to_string()),
            };
            match swarm::accept_from(brain, &caller.user_id, &caller.agent, &offer) {
                Ok(answer) => Response::json(200, &answer.to_json()),
                Err(e) => refused(409, &e.to_string()),
            }
        }
        ("GET", "events") => {
            let space = match space() {
                Ok(s) => s,
                Err(r) => return r,
            };
            if !caller.can_read(&space) || !whole(&space) {
                return refused(403, "this key does not read this space");
            }
            if !side(&ctl, &caller, &space).gives() {
                return refused(403, APART);
            }
            let mut after = req
                .query
                .get("after")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
                .max(0);
            let chunks =
                app.file_chunks && req.query.get("chunks").map(String::as_str) == Some("1");
            // A machine that took no chunks before and takes them now gets back what was left out for it.
            let served = format!("served:{}", caller.agent);
            if chunks && let Ok(Some(seq)) = sync::take_withheld(brain, &space, &served) {
                after = after.min(seq - 1).max(0);
            }
            let limit = req
                .query
                .get("limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(PAGE_EVENTS)
                .clamp(1, PAGE_EVENTS);
            match sync::events_after_with(
                brain,
                &space,
                after,
                limit,
                PAGE_BYTES,
                Some(&caller.agent),
                chunks,
            ) {
                Ok(page) => {
                    if let Some((seq, _)) = page.withheld.first() {
                        let _ = sync::note_withheld(brain, &space, &served, *seq);
                    }
                    let _ = sync::note_transfer(
                        brain,
                        &caller.agent,
                        &space,
                        "sending",
                        page.next,
                        page.head,
                        now,
                    );
                    Response::json(
                        200,
                        &json!({
                            "space": space.as_str(),
                            "events": page.events.iter().map(sync::Event::to_json).collect::<Vec<_>>(),
                            "next": page.next,
                            "head": page.head,
                            "more": page.more,
                        }),
                    )
                }
                Err(e) => refused(500, &e.to_string()),
            }
        }
        ("POST", "events") => {
            let space = match space() {
                Ok(s) => s,
                Err(r) => return r,
            };
            if !caller.can_write(&space) || !whole(&space) {
                return refused(403, "this key does not write this space");
            }
            if !side(&ctl, &caller, &space).takes() {
                return refused(403, APART);
            }
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
                return refused(400, "JSON expected: {\"events\": [...]}");
            };
            let mut events = Vec::new();
            let mut malformed = 0usize;
            for v in body
                .get("events")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                match sync::Event::from_json(&v) {
                    Ok(e) => events.push(e),
                    Err(_) => malformed += 1,
                }
            }
            match sync::apply(brain, &space, &events, &caller.agent) {
                Ok(done) => {
                    let total = body.get("head").and_then(Value::as_i64).unwrap_or(0);
                    let at = body.get("next").and_then(Value::as_i64).unwrap_or(0);
                    if total > 0 {
                        let _ = sync::note_transfer(
                            brain,
                            &caller.agent,
                            &space,
                            "receiving",
                            at,
                            total,
                            now,
                        );
                    }
                    Response::json(
                        200,
                        &json!({
                            "applied": done.applied,
                            "skipped": done.skipped,
                            "disputed": done.disputed,
                            "malformed": malformed + done.rejected.len(),
                            // Dealt with, from the first; the rest waits for an event that has not arrived.
                            "consumed": done.consumed,
                            "stopped": done.stopped,
                            // Dealt with but not kept here, each with why when it was refused: the sender does not
                            // count them as held by this node (its window keeps what they are about).
                            "not_kept": not_kept(&done),
                            "gave_up": done.gave_up,
                            // The chunks of a big file this node waits for: the sender gives them, then the rest.
                            "wants": done.wants.as_ref().map(sync::Wanted::to_json),
                        }),
                    )
                }
                Err(e) => refused(400, &e.to_string()),
            }
        }
        ("GET", "chunk") if app.file_chunks => {
            let space = match space() {
                Ok(s) => s,
                Err(r) => return r,
            };
            if !caller.can_read(&space) || !whole(&space) {
                return refused(403, "this key does not read this space");
            }
            if !side(&ctl, &caller, &space).gives() {
                return refused(403, APART);
            }
            let file = req.query.get("file").map_or("", String::as_str);
            let Some(n) = req.query.get("n").and_then(|v| v.parse::<usize>().ok()) else {
                return refused(400, "n: the position of the chunk");
            };
            let hash = req.query.get("hash").map(String::as_str);
            // Only a file the space holds or waits for: a digest alone reaches no other space's bytes.
            if !held_here(brain, &space, file) {
                return refused(404, "no such chunk here");
            }
            match memcp_core::chunks::read(brain, &space, file, n, hash) {
                Ok(Some(bytes)) => Response {
                    status: 200,
                    headers: vec![("Content-Type".into(), "application/octet-stream".into())],
                    body: bytes,
                    file: None,
                },
                Ok(None) => refused(404, "no such chunk here"),
                Err(e) => refused(500, &e.to_string()),
            }
        }
        ("POST", "chunk") if app.file_chunks => {
            let space = match space() {
                Ok(s) => s,
                Err(r) => return r,
            };
            if !caller.can_write(&space) || !whole(&space) {
                return refused(403, "this key does not write this space");
            }
            if !side(&ctl, &caller, &space).takes() {
                return refused(403, APART);
            }
            let file = req.query.get("file").map_or("", String::as_str);
            let hash = req.query.get("hash").map_or("", String::as_str);
            match memcp_core::chunks::store(brain, &space, file, hash, &req.body) {
                Ok(()) => Response::json(200, &json!({ "stored": true })),
                Err(e) => refused(400, &e.to_string()),
            }
        }
        _ => refused(404, "not found"),
    }
}

/// Whether a file of this digest is the space's: a record of it, or chunks of it waited for here.
fn held_here(brain: &memcp_core::Brain, space: &Space, file: &str) -> bool {
    if !memcp_core::chunks::is_digest(file) {
        return false;
    }
    let known = brain.open_space(space).is_ok_and(|c| {
        c.query_row(
            "SELECT count(*) > 0 FROM files WHERE sha256 = ?1",
            [file],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false)
    });
    known || memcp_core::chunks::staged(brain, space, file) > 0
}

/// Why a request about a space is refused by the policy of this node.
const APART: &str = "the person set this space apart from this machine on this node (sync policy)";

/// This node's sync policy for `space` with the machine of this key. A policy that cannot be read is `off`: nothing
/// goes or comes on a doubt.
fn side(ctl: &rusqlite::Connection, caller: &access::Caller, space: &Space) -> Policy {
    policy::of(ctl, &caller.agent, space.as_str()).unwrap_or(Policy::Off)
}

/// The most spaces a machine may tell its side of in one request.
const MAX_SIDES: usize = 2_000;

/// What the machine told of its own side, space by space (`*`: the spaces made later), kept for the page. Only the
/// person's spaces, and only the four words.
fn note_sides(
    ctl: &rusqlite::Connection,
    caller: &access::Caller,
    sides: &serde_json::Map<String, Value>,
) {
    if sides.len() > MAX_SIDES || policy::clear_theirs(ctl, &caller.agent).is_err() {
        return;
    }
    for (name, word) in sides {
        let Some(p) = word.as_str().and_then(|w| Policy::parse(w).ok()) else {
            continue;
        };
        if name == "*" {
            let _ = policy::note_theirs_later(ctl, &caller.agent, Some(p));
            continue;
        }
        let Ok(space) = Space::new(name) else {
            continue;
        };
        if memcp_core::users::is_space_owner(ctl, &space, &caller.user_id).unwrap_or(false) {
            let _ = policy::note_theirs(ctl, &caller.agent, space.as_str(), Some(p));
        }
    }
}

/// `[{eid, why}]`: the events dealt with but not kept, with the reason of a refusal (none for one left out of the
/// window).
fn not_kept(done: &sync::Applied) -> Vec<Value> {
    done.not_kept
        .iter()
        .map(|eid| {
            let why = done
                .rejected
                .iter()
                .find(|(e, _)| e == eid)
                .map(|(_, w)| w.as_str());
            json!({ "eid": eid, "why": why })
        })
        .collect()
}

/// What `GET /sync/status` answers: every peer of the memory (direct ones with their progress, and the nodes of the
/// peer directory), this node, and this replica's window with what it pruned last.
pub(crate) fn status_body(
    brain: &memcp_core::Brain,
    now: i64,
    user: &str,
) -> Result<Value, String> {
    let _ = directory::touch_self(brain, now);
    let fail = |e: memcp_core::Error| e.to_string();
    Ok(json!({
        "node": directory::node_json(brain, now).map_err(fail)?,
        "peers": sync::peers_view(brain, now).map_err(fail)?,
        // The assistants of this person on this node, and how each reaches it.
        "connections": directory::connections(brain, user).map_err(fail)?,
        "window": sync::window_status(brain).map_or(Value::Null, |w| w.to_json()),
        // The facts in dispute in this person's spaces, waiting for the person to pick a version.
        "disputes": dispute_count(brain, user),
    }))
}

/// How many disputes hold in the spaces a person owns. A space that cannot be opened counts none.
fn dispute_count(brain: &memcp_core::Brain, user: &str) -> usize {
    let Ok(ctl) = brain.open_control() else {
        return 0;
    };
    let spaces = memcp_core::users::access_of(&ctl, user).unwrap_or_default();
    spaces
        .keys()
        .filter_map(|s| brain.open_space(s).ok())
        .filter_map(|c| sync::dispute_count(&c).ok())
        .sum()
}
