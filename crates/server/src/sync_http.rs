//! The synchronisation between peers over HTTP, at the address of the peer's own connection:
//!
//! - `GET  /mcp/<connection>/sync/status`: every peer of this memory, its direction, progress per space and last activity
//!   (what the apps show live).
//! - `GET  /mcp/<connection>/sync/peers`: this node's peer directory (`{node, peers: [{name, last_seen, version, ...}]}`).
//! - `POST /mcp/<connection>/sync/peers` `{node: {name}, peers: [...]}`: the peer's directory, merged here.
//! - `GET  /mcp/<connection>/sync/spaces`: the spaces this key reaches, with their kind, layout, the time the copy began
//!   and the head of their log; which one is the person's « À trier »; the archives this node knows of; whether it takes
//!   offers.
//! - `POST /mcp/<connection>/sync/spaces` `{space, kind, label, parent, position, since}`: a space of the person that the
//!   peer has and this node lacks (see `memcp_core::swarm`); or `{space, archived_at}`: the peer archived it.
//! - `GET  /mcp/<connection>/sync/events?space=S&after=N[&limit=L]`: the events of a space after the asker's cursor, never
//!   the ones it sent itself.
//! - `POST /mcp/<connection>/sync/events?space=S` `{events: [...]}`: events from the peer, applied here idempotently.
//!
//! Only the connection's own key is let in, and only on the spaces it reaches (read for pulling, write for pushing). The
//! engine is `memcp_core::sync`; nothing here decides what a merge does.

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
            (Ok(node), Ok(peers)) => Response::json(200, &json!({ "node": node, "peers": peers })),
            (Err(e), _) | (_, Err(e)) => refused(500, &e.to_string()),
        },
        // A peer tells its directory: merged here, and the peer is known as the node behind this connection.
        ("POST", "peers") => {
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
                return refused(400, "JSON expected: {\"node\": {...}, \"peers\": [...]}");
            };
            let from = body["node"]["name"].as_str().unwrap_or(&caller.agent);
            let list = body["peers"].as_array().cloned().unwrap_or_default();
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
                    "space": s.as_str(), "kind": kind, "head": head, "writable": caller.can_write(&s),
                    "label": label, "parent": parent, "position": position,
                    "since": swarm::since(brain, &s).unwrap_or(0),
                    "triage": triage.as_ref() == Some(&s),
                }));
            }
            let archived: Vec<Value> = swarm::tombstones(brain, &caller.user_id)
                .unwrap_or_default()
                .into_iter()
                .map(|(s, at)| json!({ "space": s.as_str(), "archived_at": at }))
                .collect();
            // The exchange's protocol and this node's version: machines of different versions keep in step as long as
            // they speak the same protocol; a newer protocol is refused by an older node, which says so and waits.
            Response::json(
                200,
                &json!({
                    "protocol": sync::PROTOCOL, "version": env!("CARGO_PKG_VERSION"), "spaces": out,
                    "offers": takes_offers(&caller), "archived": archived,
                }),
            )
        }
        // A space of the person that the peer has and this node lacks, or one the peer archived.
        ("POST", "spaces") => {
            if !takes_offers(&caller) {
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
                return match swarm::archive_here(brain, &caller.user_id, &space, at) {
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
            match swarm::accept(brain, &caller.user_id, &offer) {
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
            let after = req
                .query
                .get("after")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
                .max(0);
            let limit = req
                .query
                .get("limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(PAGE_EVENTS)
                .clamp(1, PAGE_EVENTS);
            match sync::events_after(brain, &space, after, limit, PAGE_BYTES, Some(&caller.agent)) {
                Ok(page) => {
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
                        }),
                    )
                }
                Err(e) => refused(400, &e.to_string()),
            }
        }
        _ => refused(404, "not found"),
    }
}

/// Whether the caller may offer spaces and archives: a machine of the person (`peer-…`) that reaches every space of
/// theirs, whole, and may write.
fn takes_offers(caller: &access::Caller) -> bool {
    caller.agent.starts_with(sync::PEER_PREFIX)
        && caller.all_spaces
        && !caller.read_only_forced
        && !caller.is_scoped()
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
    }))
}
