//! The synchronisation between peers over HTTP, at the address of the peer's own connection:
//!
//! - `GET  /mcp/<connection>/sync/status`: every peer of this memory, its direction, progress per space and last activity
//!   (what the apps show live).
//! - `GET  /mcp/<connection>/sync/spaces`: the spaces this key reaches, with their kind and the head of their log.
//! - `GET  /mcp/<connection>/sync/events?space=S&after=N[&limit=L]`: the events of a space after the asker's cursor, never
//!   the ones it sent itself.
//! - `POST /mcp/<connection>/sync/events?space=S` `{events: [...]}`: events from the peer, applied here idempotently.
//!
//! Only the connection's own key is let in, and only on the spaces it reaches (read for pulling, write for pushing). The
//! engine is `memcp_core::sync`; nothing here decides what a merge does.

use memcp_core::{Space, access, sync};
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
    let space = || -> Result<Space, Response> {
        let name = req.query.get("space").map(String::as_str).unwrap_or("");
        Space::new(name).map_err(|_| refused(400, "space: a space name"))
    };
    match (req.method.as_str(), op) {
        ("GET", "status") => match sync::status(brain, now) {
            Ok(list) => Response::json(
                200,
                &json!({ "peers": list.iter().map(status_json).collect::<Vec<_>>() }),
            ),
            Err(e) => refused(500, &e.to_string()),
        },
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
            Response::json(200, &json!({ "spaces": out }))
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

fn status_json(p: &sync::PeerStatus) -> Value {
    let done: i64 = p.spaces.iter().map(|s| s.done).sum();
    let total: i64 = p.spaces.iter().map(|s| s.total).sum();
    json!({
        "peer": p.peer,
        "name": p.name,
        "direction": p.direction,
        "done": done,
        "total": total,
        "percent": if total > 0 { done * 100 / total } else { 100 },
        "spaces": p.spaces.iter().map(|s| json!({ "space": s.space, "done": s.done, "total": s.total, "percent": s.percent })).collect::<Vec<_>>(),
        "last_activity": p.last_activity,
    })
}
