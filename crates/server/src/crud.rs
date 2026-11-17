//! Create, read, update and delete of what a person owns, over HTTP (`/admin/api/...`): spaces, nodes, connections,
//! people, vocabularies and library folders. See docs/CRUD.md for the whole matrix.
//!
//! Every destructive route takes `"dry_run": true` and then answers what it would do, in `would` (one sentence the page
//! shows in its confirmation), without doing it. Every real change is audited by the core function that does it.
//! A person touches their own objects only; an answer for someone else's object is 403 or 404, never a result.

use memcp_core::lifecycle::{Children, Disposition, SpaceNew, SpaceUpdate};
use memcp_core::users::{self, User};
use memcp_core::{Space, access, layout::Change, peers, people, rules, util};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Request, Response};

fn err(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

fn fail(e: impl std::fmt::Display) -> Response {
    err(400, &e.to_string())
}

fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, Response> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err(400, &format!("{key} is required")))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn dry(body: &Value) -> bool {
    body.get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn space_of(v: &str) -> Result<Space, Response> {
    Space::new(v).map_err(|_| err(400, "invalid space"))
}

/// One of the person's own spaces: 403 for any other.
fn owned(ctl: &Connection, me: &User, name: &str) -> Result<Space, Response> {
    let space = space_of(name)?;
    if users::is_space_owner(ctl, &space, &me.id).map_err(fail)? {
        Ok(space)
    } else {
        Err(err(403, "not your space"))
    }
}

fn parent_of(v: &Value) -> Result<Option<Space>, Response> {
    match v.as_str().filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(p) => space_of(p).map(Some),
    }
}

fn user_row(ctl: &Connection, u: &User) -> Value {
    json!({
        "id": u.id, "name": u.name, "display": u.display, "admin": u.admin, "role": if u.admin { "admin" } else { "member" },
        "disabled": u.disabled, "home_assistant": u.ha_user.is_some(), "created_at": util::iso(u.created_at),
        "heir": users::heir_of(ctl, &u.id).ok().flatten(),
        "recovery_key": users::has_recovery_key(ctl, &u.id).unwrap_or(false),
        "last_seen": users::last_seen(ctl, &u.id).ok().map(util::iso),
    })
}

/// Reads. `None` when the path is not one of ours.
pub fn get(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    req: &Request,
    now: i64,
) -> Option<Response> {
    if !matches!(
        path,
        "/spaces/one"
            | "/spaces/archived"
            | "/nodes"
            | "/connections/one"
            | "/users"
            | "/user"
            | "/space/vocabularies"
            | "/space/libraries"
            | "/space/folder"
    ) {
        return None;
    }
    let q = |k: &str| req.query.get(k).map(String::as_str).unwrap_or("");
    let done = (|| -> Result<Value, Response> {
        match path {
            "/spaces/one" => {
                let s = owned(ctl, me, q("space"))?;
                Ok(app.brain.space_info(&me.id, &s).map_err(fail)?.to_json())
            }
            "/spaces/archived" => Ok(
                json!({ "archived": app.brain.archived_spaces(&me.id).map_err(fail)?
                .iter().map(memcp_core::lifecycle::Archived::to_json).collect::<Vec<_>>() }),
            ),
            "/nodes" => {
                let list = peers::list(&app.brain, me, now, q("all") == "1").map_err(fail)?;
                Ok(json!({ "nodes": list.iter().map(|n| {
                    let mut j = n.to_json();
                    j["may_forget"] = json!(peers::may_forget(n, now));
                    j
                }).collect::<Vec<_>>() }))
            }
            "/connections/one" => {
                let c = access::get(ctl, q("agent"))
                    .map_err(fail)?
                    .filter(|c| c.user_id == me.id)
                    .ok_or_else(|| err(404, "no such connection"))?;
                Ok(
                    json!({ "agent": c.agent, "rights": c.rights, "read_only": c.read_only_forced,
                    "revoked": c.revoked_at.is_some(), "internal": c.internal,
                    "created_at": util::iso(c.created_at), "last_used_at": c.last_used_at.map(util::iso),
                    "expires_at": c.expires_at.map(util::iso) }),
                )
            }
            "/users" => {
                let all = users::list_users(ctl).map_err(fail)?;
                Ok(
                    json!({ "users": all.iter().filter(|u| me.admin || u.id == me.id).map(|u| user_row(ctl, u)).collect::<Vec<_>>() }),
                )
            }
            "/user" => {
                let u = users::user(ctl, q("id"))
                    .map_err(fail)?
                    .or(users::user_by_name(ctl, q("id")).map_err(fail)?)
                    .ok_or_else(|| err(404, "no such person"))?;
                if !me.admin && u.id != me.id {
                    return Err(err(403, "only an administrator reads someone else"));
                }
                Ok(user_row(ctl, &u))
            }
            "/space/vocabularies" => {
                let s = owned(ctl, me, q("space"))?;
                let conn = app.brain.open_space(&s).map_err(fail)?;
                Ok(
                    json!({ "vocabularies": memcp_core::vocabulary::list(&conn).map_err(fail)?.iter().map(|v| json!({
                    "name": v.name, "source": v.source, "terms": v.terms, "imported_at": util::iso(v.imported_at),
                })).collect::<Vec<_>>() }),
                )
            }
            "/space/libraries" => {
                let s = owned(ctl, me, q("space"))?;
                let conn = app.brain.open_space(&s).map_err(fail)?;
                Ok(
                    json!({ "libraries": memcp_core::library::roots(&conn).map_err(fail)?.iter().map(|r| json!({
                    "id": r.id, "root": r.root, "files": r.entries, "scanned_at": r.scanned_at.map(util::iso),
                })).collect::<Vec<_>>() }),
                )
            }
            "/space/folder" => {
                let s = owned(ctl, me, q("space"))?;
                memcp_core::folders::info(&app.brain, &s).map_err(fail)
            }
            _ => Err(err(404, "not found")),
        }
    })();
    Some(match done {
        Ok(v) => Response::json(200, &v),
        Err(r) => r,
    })
}

/// Writes. `None` when the path is not one of ours.
pub fn post(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    body: &Value,
) -> Option<Result<Value, Response>> {
    Some(match path {
        "/spaces/create" => spaces_create(app, me, body),
        "/spaces/update" => spaces_update(app, ctl, me, body),
        "/spaces/delete" => spaces_delete(app, ctl, me, body),
        "/spaces/restore" => spaces_restore(app, me, body),
        "/nodes/update" | "/nodes/pause" | "/nodes/resume" | "/nodes/revoke" | "/nodes/forget" => {
            nodes(app, me, path, body)
        }
        "/connections/rotate" | "/connections/delete" => connections(app, ctl, me, path, body),
        "/users/delete" => users_delete(app, me, body),
        "/recovery/clear" => {
            if dry(body) {
                Ok(
                    json!({ "dry_run": true, "would": "your recovery key is removed: no new account can take your memory over with it any more" }),
                )
            } else {
                users::clear_recovery_key(ctl, &me.id)
                    .map(|()| json!({ "ok": true }))
                    .map_err(fail)
            }
        }
        "/space/vocabulary" => vocabulary(app, ctl, me, body),
        "/space/library" => library(app, ctl, me, body),
        "/space/folder" => folder(app, ctl, me, body),
        _ => return None,
    })
}

fn spaces_create(app: &App, me: &User, body: &Value) -> Result<Value, Response> {
    let new = SpaceNew {
        kind: body
            .get("kind")
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())
            .map(String::from),
        label: text(body, "label")?.to_string(),
        parent: match body.get("parent") {
            None => None,
            Some(v) => parent_of(v)?,
        },
        keywords: strings(body.get("keywords")),
        tags: strings(body.get("tags")),
    };
    let s = app
        .brain
        .create_space_with(&me.id, &me.name, &new)
        .map_err(fail)?;
    Ok(
        json!({ "result": "created", "space": app.brain.space_info(&me.id, &s).map_err(fail)?.to_json() }),
    )
}

fn spaces_update(app: &App, ctl: &Connection, me: &User, body: &Value) -> Result<Value, Response> {
    let s = owned(ctl, me, text(body, "space")?)?;
    let u = SpaceUpdate {
        layout: Change {
            label: body.get("label").and_then(Value::as_str).map(String::from),
            parent: match body.get("parent") {
                None => None,
                Some(v) => Some(parent_of(v)?),
            },
            position: body.get("position").and_then(Value::as_i64),
        },
        kind: body.get("kind").and_then(Value::as_str).map(String::from),
        rules: rules::Edit {
            add_tags: strings(body.get("add_tags")),
            remove_tags: strings(body.get("remove_tags")),
            add_keywords: strings(body.get("add_keywords")),
            remove_keywords: strings(body.get("remove_keywords")),
        },
    };
    let info = app
        .brain
        .update_space(&me.id, &me.name, &s, &u)
        .map_err(fail)?;
    Ok(json!({ "result": "updated", "space": info.to_json() }))
}

fn spaces_delete(app: &App, ctl: &Connection, me: &User, body: &Value) -> Result<Value, Response> {
    let s = owned(ctl, me, text(body, "space")?)?;
    let disposition = match (
        body.get("into")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
        body.get("archive").and_then(Value::as_bool),
    ) {
        (Some(t), None | Some(false)) => Disposition::Into(owned(ctl, me, t)?),
        (None, Some(true)) => Disposition::Archive,
        _ => {
            return Err(err(
                400,
                "a space is never deleted silently: choose `into` another space, or `archive: true`",
            ));
        }
    };
    let children = match body.get("children").and_then(Value::as_str) {
        None => Children::default(),
        Some(c) => Children::parse(c).map_err(fail)?,
    };
    let r = app
        .brain
        .delete_space(&me.id, &s, &disposition, &children, dry(body), util::now())
        .map_err(fail)?;
    let mut j = r.to_json();
    j["would"] = json!(r.sentence());
    Ok(j)
}

fn spaces_restore(app: &App, me: &User, body: &Value) -> Result<Value, Response> {
    let d = dry(body);
    let s = app
        .brain
        .restore_space(&me.id, text(body, "archive")?, d)
        .map_err(fail)?;
    Ok(
        json!({ "dry_run": d, "space": s.as_str(), "would": format!("{s} comes back with its notes, files, label and place in the tree") }),
    )
}

fn nodes(app: &App, me: &User, path: &str, body: &Value) -> Result<Value, Response> {
    let peer = text(body, "peer")?;
    let now = util::now();
    let node = peers::get(&app.brain, me, peer, now).map_err(|_| err(404, "no such machine"))?;
    let d = dry(body);
    let would = match path {
        "/nodes/pause" => format!(
            "{} stops exchanging until you resume it; nothing is lost",
            node.name
        ),
        "/nodes/resume" => format!("{} resumes where it stopped", node.name),
        "/nodes/revoke" => format!(
            "{}'s key stops at once; the entry stays, marked revoked, with its history",
            node.name
        ),
        "/nodes/forget" => format!(
            "{} leaves the lists (revoked or silent machines only); it shows again if it speaks again",
            node.name
        ),
        _ => format!("{} is renamed or annotated", node.name),
    };
    if d {
        if path == "/nodes/forget" && !peers::may_forget(&node, now) {
            return Err(err(
                400,
                "this machine is still in use: revoke it first, or wait until it has been silent for 30 days",
            ));
        }
        return Ok(json!({ "dry_run": true, "peer": peer, "would": would }));
    }
    match path {
        "/nodes/update" => {
            let n = peers::update(
                &app.brain,
                me,
                peer,
                body.get("label")
                    .or_else(|| body.get("name"))
                    .and_then(Value::as_str),
                body.get("note").and_then(Value::as_str),
            )
            .map_err(fail)?;
            Ok(json!({ "node": n.to_json() }))
        }
        "/nodes/pause" => Ok(
            json!({ "node": peers::set_paused(&app.brain, me, peer, true).map_err(fail)?.to_json() }),
        ),
        "/nodes/resume" => Ok(
            json!({ "node": peers::set_paused(&app.brain, me, peer, false).map_err(fail)?.to_json() }),
        ),
        "/nodes/revoke" => {
            Ok(json!({ "node": peers::revoke(&app.brain, me, peer).map_err(fail)?.to_json() }))
        }
        _ => {
            peers::forget(&app.brain, me, peer, now).map_err(fail)?;
            Ok(json!({ "ok": true }))
        }
    }
}

fn connections(
    app: &App,
    ctl: &Connection,
    me: &User,
    path: &str,
    body: &Value,
) -> Result<Value, Response> {
    let _ = app;
    let agent = text(body, "agent")?;
    let c = access::get(ctl, agent)
        .map_err(fail)?
        .filter(|c| c.user_id == me.id)
        .ok_or_else(|| err(403, "not your connection"))?;
    let rotate = path == "/connections/rotate";
    let would = if rotate {
        format!(
            "{agent} gets a new key at the same address, with the same rights; the old key stops at once"
        )
    } else {
        format!("{agent} is removed from the list; what the audit says about it stays")
    };
    if dry(body) {
        if !rotate && c.revoked_at.is_none() {
            return Err(err(400, "this connection is still active: revoke it first"));
        }
        if rotate && c.revoked_at.is_some() {
            return Err(err(400, "this connection is revoked"));
        }
        return Ok(json!({ "dry_run": true, "agent": agent, "would": would }));
    }
    if rotate {
        let token = access::rotate(ctl, agent, &me.id)
            .map_err(fail)?
            .ok_or_else(|| err(400, "this connection is revoked"))?;
        Ok(json!({
            "token": token,
            "mcp_url": format!("{}/mcp/{agent}", app.public_url),
            "mcp_url_with_key": format!("{}/mcp/{agent}?key={token}", app.public_url),
        }))
    } else {
        access::delete(ctl, agent, &me.id).map_err(fail)?;
        Ok(json!({ "ok": true }))
    }
}

fn users_delete(app: &App, me: &User, body: &Value) -> Result<Value, Response> {
    if !me.admin {
        return Err(err(403, "administrators only"));
    }
    let r = people::retire_person(&app.brain, me, text(body, "id")?, dry(body), util::now())
        .map_err(fail)?;
    let mut j = r.to_json();
    j["would"] = json!(r.sentence());
    Ok(j)
}

fn vocabulary(app: &App, ctl: &Connection, me: &User, body: &Value) -> Result<Value, Response> {
    let s = owned(ctl, me, text(body, "space")?)?;
    let name = text(body, "name")?;
    let terms = strings(body.get("terms"));
    let op = text(body, "op")?;
    let mut conn = app.brain.open_space(&s).map_err(fail)?;
    let would = match op {
        "delete" => {
            format!("the vocabulary {name} and its terms leave {s}; the original file is untouched")
        }
        "add" => format!("{} term(s) join {name}", terms.len()),
        "remove" => format!("{} term(s) leave {name}", terms.len()),
        _ => return Err(err(400, "op: add, remove or delete")),
    };
    if dry(body) {
        return Ok(json!({ "dry_run": true, "would": would }));
    }
    let n = match op {
        "delete" => usize::from(memcp_core::vocabulary::remove(&mut conn, name).map_err(fail)?),
        "add" => memcp_core::vocabulary::add_terms(&mut conn, name, &terms).map_err(fail)?,
        _ => memcp_core::vocabulary::remove_terms(&mut conn, name, &terms).map_err(fail)?,
    };
    access::auth_event(
        ctl,
        Some(&me.id),
        None,
        "vocabulary_edit",
        &format!("{s} {name} {op}"),
    );
    Ok(json!({ "ok": true, "changed": n }))
}

fn library(app: &App, ctl: &Connection, me: &User, body: &Value) -> Result<Value, Response> {
    let s = owned(ctl, me, text(body, "space")?)?;
    let op = text(body, "op")?;
    let conn = app.brain.open_space(&s).map_err(fail)?;
    let now = util::now();
    let result = match op {
        // A path on the machine: only an administrator names one.
        "add" => {
            if !me.admin {
                return Err(err(
                    403,
                    "administrators only: mounting a folder names a path on this machine",
                ));
            }
            let path = text(body, "path")?;
            if dry(body) {
                return Ok(
                    json!({ "dry_run": true, "would": format!("{path} is mounted read-only on {s} and indexed; it is never copied or changed") }),
                );
            }
            // One setting: `library add` is `folder set` in read-only mode (see docs/FOLDERS.md).
            memcp_core::folders::set(
                &app.brain,
                &s,
                Some(std::path::Path::new(path)),
                memcp_core::folders::Mode::Read,
                now,
            )
            .map_err(fail)?;
            let canon = std::fs::canonicalize(path).map_err(fail)?;
            let id = memcp_core::library::roots(&conn)
                .map_err(fail)?
                .into_iter()
                .find(|r| std::path::Path::new(&r.root) == canon)
                .map(|r| r.id)
                .ok_or_else(|| err(500, "the folder was not mounted"))?;
            let r = memcp_core::library::rescan(&conn, id, now).map_err(fail)?;
            json!({ "ok": true, "id": id, "scan": r.to_json() })
        }
        "remove" => {
            let id = body
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| err(400, "id is required"))?;
            let root = memcp_core::library::roots(&conn)
                .map_err(fail)?
                .into_iter()
                .find(|r| r.id == id)
                .ok_or_else(|| err(404, "no such library"))?;
            if dry(body) {
                return Ok(
                    json!({ "dry_run": true, "would": format!("{} is unmounted from {s}: its index is forgotten, the folder itself is never touched", root.root) }),
                );
            }
            memcp_core::library::remove_root(&conn, id).map_err(fail)?;
            // The folder of the space on this node was this one: the setting goes with it.
            if memcp_core::folders::get(&app.brain, &s)
                .map_err(fail)?
                .is_some_and(|f| f.path == std::path::Path::new(&root.root))
            {
                memcp_core::folders::set(
                    &app.brain,
                    &s,
                    None,
                    memcp_core::folders::Mode::Read,
                    now,
                )
                .map_err(fail)?;
            }
            json!({ "ok": true })
        }
        "scan" => {
            json!({ "ok": true, "scan": memcp_core::library::rescan_all(&conn, now).map_err(fail)?.to_json() })
        }
        _ => return Err(err(400, "op: add, remove or scan")),
    };
    access::auth_event(
        ctl,
        Some(&me.id),
        None,
        "library_edit",
        &format!("{s} {op}"),
    );
    Ok(result)
}

/// `POST /space/folder`: the folder of a space on THIS node. `{space, path, mode?}` sets it (`path` null or empty: back
/// to the default storage folder), `{space, op: "sync"}` makes a pass now. Naming a path of the machine is for
/// administrators; going back to the default and syncing are for the owner of the space.
fn folder(app: &App, ctl: &Connection, me: &User, body: &Value) -> Result<Value, Response> {
    let s = owned(ctl, me, text(body, "space")?)?;
    let op = body.get("op").and_then(Value::as_str).unwrap_or("set");
    let now = util::now();
    let result = match op {
        "sync" => {
            if memcp_core::folders::get(&app.brain, &s)
                .map_err(fail)?
                .is_none()
            {
                return Err(err(400, "this space has no folder on this node"));
            }
            if dry(body) {
                return Ok(
                    json!({ "dry_run": true, "would": format!("the folder of {s} is read and written once now") }),
                );
            }
            let r = memcp_core::folders::sync_space(
                &app.brain,
                &s,
                now,
                memcp_core::folders::Options::default(),
            )
            .map_err(fail)?;
            json!({ "ok": true, "scan": r.to_json(), "folder": memcp_core::folders::info(&app.brain, &s).map_err(fail)? })
        }
        "set" => {
            let path = body
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|p| !p.is_empty());
            if path.is_some() && !me.admin {
                return Err(err(
                    403,
                    "administrators only: choosing a folder names a path on this machine",
                ));
            }
            let mode = memcp_core::folders::Mode::parse(
                body.get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("readwrite"),
            )
            .map_err(fail)?;
            if dry(body) {
                let would = match path {
                    Some(p) if mode == memcp_core::folders::Mode::Read => format!(
                        "{p} becomes the folder of {s} on this node, only read and indexed for search; nothing is written in it"
                    ),
                    Some(p) => format!(
                        "{p} becomes the folder of {s} on this node: its notes are kept there as Markdown and its files as themselves, and what you change there comes back into the memory. This is not shared with the other nodes"
                    ),
                    None => format!(
                        "{s} goes back to the node's own storage folder; the folder it had is left as it is"
                    ),
                };
                return Ok(json!({ "dry_run": true, "would": would }));
            }
            let info =
                memcp_core::folders::set(&app.brain, &s, path.map(std::path::Path::new), mode, now)
                    .map_err(fail)?;
            json!({ "ok": true, "folder": info })
        }
        _ => return Err(err(400, "op: set or sync")),
    };
    access::auth_event(ctl, Some(&me.id), None, "folder_edit", &format!("{s} {op}"));
    Ok(result)
}
