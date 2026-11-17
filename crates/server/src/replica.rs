//! Starting and joining a memory from the node page.
//!
//! - **Start a memory**: the six default spaces of `memcp init --defaults` for the person, and their name.
//! - **Join a memory**: this node asks another one to let it in (the person there compares three words and says yes),
//!   then keeps in step with it. The asking and the syncing are the work of the `memcp-sync` program, started from here:
//!   the node itself has a TLS stack for one thing only, the speech service the person may name for their voice notes (see
//!   deny.toml): the sync with other nodes stays a program of its own. The program tells the page where the request
//!   stands in a status file (the three words, never a key); the peer it keeps (address and key) lives in `peers/`,
//!   private to the owner, and is started again with the node.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use memcp_core::users::{self, User};
use memcp_core::{Error, Space, util};
use serde_json::{Value, json};

use crate::app::App;

/// The spaces `memcp init --defaults` makes.
pub const DEFAULT_SPACES: [&str; 6] = ["personal", "health", "home", "finances", "hobbies", "work"];

/// Programs started by this node that may run at once.
const MAX_CHILDREN: usize = 16;

/// The seconds between two passes with a peer.
const EVERY: &str = "10";

/// `POST /start` {name?}: the default spaces, those missing only, and the person's display name.
pub fn start(app: &App, ctl: &rusqlite::Connection, me: &User, name: &str) -> Result<Value, Error> {
    let name = name.trim();
    if !name.is_empty() {
        users::set_display(ctl, &me.id, name)?;
    }
    let mut made = vec![];
    for s in DEFAULT_SPACES {
        let space = Space::new(s)?;
        if !app.brain.has_space(&space)? {
            app.brain.add_space_for(&space, &me.id)?;
            made.push(s);
        }
    }
    Ok(json!({ "made": made, "spaces": DEFAULT_SPACES }))
}

/// A program shipped next to this one (or the one named by the variable).
fn program(var: &str, base: &str) -> PathBuf {
    if let Some(p) = std::env::var_os(var) {
        return PathBuf::from(p);
    }
    let name = if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    };
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn sync_program() -> PathBuf {
    program("MEMCP_SYNC_BIN", "memcp-sync")
}

/// `GET /discover`: the nodes found on this network and on the tailnet (`intake::discover` owns the mDNS browsing and
/// the probes), this node left out. Nothing found is an empty list, never an error.
pub fn discover(app: &App) -> Value {
    let nodes: Vec<Value> = crate::intake::discover::discover(std::time::Duration::from_secs(3))
        .into_iter()
        .filter(|f| f.name != app.node_name && f.address != app.public_url)
        .map(
            |f| json!({ "name": f.name, "address": f.address, "version": f.version, "via": f.via }),
        )
        .collect();
    json!({ "nodes": nodes })
}

fn keep(app: &App, child: Child) {
    let mut kids = app.children.lock().unwrap_or_else(|e| e.into_inner());
    kids.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    kids.push(child);
}

fn clean_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn status_file(app: &App, id: &str) -> PathBuf {
    app.brain.dir().join(format!("join-{id}.json"))
}

/// `POST /join-memory` {address}: asks that node to let this one in. Answers an id to follow it by.
pub fn join_start(app: &App, address: &str, ticket: Option<&str>) -> Result<Value, String> {
    let address = address.trim().trim_end_matches('/');
    if !(address.starts_with("https://") || address.starts_with("http://"))
        || address.chars().any(|c| c.is_whitespace() || c.is_control())
        || address.len() > 300
    {
        return Err("an address such as https://memory.example.net".into());
    }
    {
        // Programs that ask to join are few and short: a flood of requests must not fill the process table.
        let mut kids = app.children.lock().unwrap_or_else(|e| e.into_inner());
        kids.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
        if kids.len() >= MAX_CHILDREN {
            return Err("too many programs are running already, try again in a minute".into());
        }
    }
    let id = util::random_base32(12).to_ascii_lowercase();
    let file = status_file(app, &id);
    let mut command = Command::new(sync_program());
    // A one-time ticket from a node of the same person (no words to compare): in the environment, never in the arguments
    // another user of the machine could read. The program reads `MEMCP_JOIN_TICKET`.
    if let Some(t) = ticket.map(str::trim).filter(|t| !t.is_empty()) {
        command.env("MEMCP_JOIN_TICKET", t);
    }
    let child = command
        .arg("--data")
        .arg(app.brain.dir())
        .args(["--join", address, "--status-file"])
        .arg(&file)
        .args(["--name", &app.node_name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            "the program that joins (memcp-sync) is not installed on this node".to_string()
        })?;
    keep(app, child);
    Ok(json!({ "id": id, "state": "asking" }))
}

/// `GET /join-memory?id=`: where the request stands. When it is approved, the sync with that peer starts.
pub fn join_state(app: &App, id: &str) -> Result<Value, String> {
    if !clean_id(id) {
        return Err("unknown request".into());
    }
    let file = status_file(app, id);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(json!({ "state": "asking" }));
    };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if v["state"] == "approved" {
        let _ = std::fs::remove_file(&file);
        resume(app);
    } else if matches!(v["state"].as_str(), Some("denied" | "expired" | "failed")) {
        let _ = std::fs::remove_file(&file);
    }
    Ok(v)
}

/// Starts the sync with every peer this node keeps and has not started yet (at start, and after a join).
pub fn resume(app: &App) {
    let Ok(entries) = std::fs::read_dir(app.brain.dir().join("peers")) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let (Some(name), Some(address), Some(key)) =
            (v["name"].as_str(), v["address"].as_str(), v["key"].as_str())
        else {
            continue;
        };
        {
            let mut started = app.synced.lock().unwrap_or_else(|e| e.into_inner());
            if !started.insert(name.to_string()) {
                continue;
            }
        }
        if let Ok(child) = Command::new(sync_program())
            .arg("--data")
            .arg(app.brain.dir())
            .args(["--peer", address, "--name", name, "--every", EVERY])
            .env("MEMCP_PEER_TOKEN", key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            keep(app, child);
        } else {
            app.synced
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(name);
        }
    }
}
