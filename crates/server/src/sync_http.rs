//! The synchronisation between peers over HTTP, at the address of the peer's own connection:
//!
//! - `GET  /mcp/<connection>/sync/status`: every peer of this memory, its direction, progress per space and last activity
//!   (what the apps show live).
//! - `GET  /mcp/<connection>/sync/peers`: this node's peer directory (`{node, peers: [{name, last_seen, version, ...}]}`).
//! - `POST /mcp/<connection>/sync/peers` `{node: {name}, peers: [...]}`: the peer's directory, merged here.
//! - `GET  /mcp/<connection>/sync/spaces`: the spaces this key reaches, with their kind and the head of their log.
//! - `GET  /mcp/<connection>/sync/events?space=S&after=N[&limit=L]`: the events of a space after the asker's cursor, never
//!   the ones it sent itself.
//! - `POST /mcp/<connection>/sync/events?space=S` `{events: [...]}`: events from the peer, applied here idempotently.
//!
//! Only the connection's own key is let in, and only on the spaces it reaches (read for pulling, write for pushing). The
//! engine is `memcp_core::sync`; nothing here decides what a merge does.

use memcp_core::{Space, access, directory, sync};
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
    let _ = access::touch(&ctl, &caller, req.ip.as_deref(), now);
    // Serving a request is activity: this node's own entry in the peer directory says so.
    let _ = directory::touch_self(brain, now);
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
            for s in brain.spaces().unwrap_or_default() {
                if !caller.can_read(&s) {
                    continue;
                }
                let kind = ctl
                    .query_row(
                        "SELECT kind FROM spaces WHERE name = ?1",
                        [s.as_str()],
                        |r| r.get::<_, String>(0),
                    )
                    .unwrap_or_else(|_| "custom".into());
                let head = sync::head(brain, &s).unwrap_or(0);
                out.push(json!({ "space": s.as_str(), "kind": kind, "head": head, "writable": caller.can_write(&s) }));
            }
            // The exchange's protocol and this node's version: machines of different versions keep in step as long as
            // they speak the same protocol; a newer protocol is refused by an older node, which says so and waits.
            Response::json(
                200,
                &json!({ "protocol": sync::PROTOCOL, "version": env!("CARGO_PKG_VERSION"), "spaces": out }),
            )
        }
        ("GET", "events") => {
            let space = match space() {
                Ok(s) => s,
                Err(r) => return r,
            };
            if !caller.can_read(&space) {
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
            if !caller.can_write(&space) {
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
