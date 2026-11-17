//! The machines (nodes, peers) that hold this memory, as their owner manages them: read, rename, note, pause and resume
//! their sync, revoke, forget.
//!
//! A peer is a connection named `peer-<machine>-xxxx` (its key stops when revoked) plus the progress rows of
//! `sync_state`. What the person says about it (a name, a note, paused, forgotten) is kept in `peer_meta`. Nothing here
//! deletes history: revoking stops the key and the entry stays, marked revoked; forgetting only hides a revoked or
//! silent machine from the lists, and it shows again if it speaks again.
//!
//! What each node exposes comes with the peer-directory work and is not part of this module.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::access::{self, auth_event};
use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::sync::{self, PEER_PREFIX};
use crate::users::User;
use crate::util;

/// A machine silent for this long may be forgotten without being revoked first.
pub const SILENT_SECS: i64 = 30 * 86_400;
const MAX_NAME: usize = 64;
const MAX_NOTE: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
/// The node.
pub struct Node {
    /// The connection's name, `peer-<machine>-xxxx`.
    pub peer: String,
    /// What is shown: the name the person gave, else the machine's own.
    pub name: String,
    /// The label.
    pub label: Option<String>,
    /// The note.
    pub note: String,
    /// `active`, `paused` or `revoked`.
    pub state: String,
    /// The created at.
    pub created_at: Option<i64>,
    /// The last used at.
    pub last_used_at: Option<i64>,
    /// `sending`, `receiving` or `idle`.
    pub direction: String,
    /// The last activity.
    pub last_activity: Option<i64>,
    /// Hidden from the lists since then (it shows again if it speaks after).
    pub forgotten_at: Option<i64>,
}

impl Node {
    /// To json.
    pub fn to_json(&self) -> Value {
        json!({
            "peer": self.peer, "name": self.name, "label": self.label, "note": self.note,
            "state": self.state, "paused": self.state == "paused", "revoked": self.state == "revoked",
            "created_at": self.created_at.map(util::iso), "last_used_at": self.last_used_at.map(util::iso),
            "direction": self.direction, "last_activity": self.last_activity,
            "forgotten": self.forgotten_at.is_some(),
            // Forgotten and silent since: left out of the lists.
            "hidden": self.forgotten_at.is_some_and(|f| self.last_seen().is_none_or(|s| s <= f)),
        })
    }

    /// The latest sign of life of the machine.
    pub fn last_seen(&self) -> Option<i64> {
        self.last_used_at.max(self.last_activity)
    }
}

type Meta = (Option<String>, String, Option<i64>, Option<i64>);

fn meta_of(ctl: &Connection) -> Result<BTreeMap<String, Meta>> {
    let mut st = ctl.prepare("SELECT peer, label, note, paused_at, forgotten_at FROM peer_meta")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The machines of a person: their peer connections (revoked ones too) and, for an administrator, the machines this
/// node syncs with that hold no key here. Forgotten ones are left out unless `all`.
pub fn list(brain: &Brain, who: &User, now: i64, all: bool) -> Result<Vec<Node>> {
    let ctl = brain.open_control()?;
    let meta = meta_of(&ctl)?;
    let status: BTreeMap<String, sync::PeerStatus> = sync::status(brain, now)?
        .into_iter()
        .map(|p| (p.peer.clone(), p))
        .collect();
    let mut out: BTreeMap<String, Node> = BTreeMap::new();
    for c in access::list(&ctl)? {
        if c.kind != access::Kind::Peer || c.user_id != who.id {
            continue;
        }
        out.insert(
            c.agent.clone(),
            Node {
                peer: c.agent.clone(),
                name: String::new(),
                label: None,
                note: String::new(),
                state: if c.revoked_at.is_some() {
                    "revoked"
                } else {
                    "active"
                }
                .into(),
                created_at: Some(c.created_at),
                last_used_at: c.last_used_at,
                direction: "idle".into(),
                last_activity: None,
                forgotten_at: None,
            },
        );
    }
    if who.admin {
        for (peer, p) in &status {
            out.entry(peer.clone()).or_insert_with(|| Node {
                peer: peer.clone(),
                name: String::new(),
                label: None,
                note: String::new(),
                state: "active".into(),
                created_at: None,
                last_used_at: None,
                direction: p.direction.clone(),
                last_activity: p.last_activity,
                forgotten_at: None,
            });
        }
    }
    for n in out.values_mut() {
        if let Some(p) = status.get(&n.peer) {
            n.direction = p.direction.clone();
            n.last_activity = p.last_activity;
        }
        n.name = sync::machine_name(&n.peer);
        if let Some((label, note, paused, forgotten)) = meta.get(&n.peer) {
            n.label = label.clone();
            n.note = note.clone();
            n.forgotten_at = *forgotten;
            if let Some(l) = label {
                n.name = l.clone();
            }
            if n.state == "active" && paused.is_some() {
                n.state = "paused".into();
            }
        }
    }
    Ok(out
        .into_values()
        .filter(|n| {
            all || n
                .forgotten_at
                .is_none_or(|f| n.last_seen().is_some_and(|s| s > f))
        })
        .collect())
}

/// One machine of the person.
pub fn get(brain: &Brain, who: &User, peer: &str, now: i64) -> Result<Node> {
    list(brain, who, now, true)?
        .into_iter()
        .find(|n| n.peer == peer)
        .ok_or_else(|| Error::Invalid(format!("no machine {peer} for this person")))
}

fn check(brain: &Brain, who: &User, peer: &str) -> Result<()> {
    if !peer.starts_with(PEER_PREFIX) {
        return Err(Error::Invalid(format!(
            "{peer} is not a machine (its name starts with {PEER_PREFIX})"
        )));
    }
    get(brain, who, peer, util::now()).map(|_| ())
}

/// Gives a machine a name and a note (`None` leaves one as it is; an empty name goes back to the machine's own).
pub fn update(
    brain: &Brain,
    who: &User,
    peer: &str,
    label: Option<&str>,
    note: Option<&str>,
) -> Result<Node> {
    check(brain, who, peer)?;
    let ctl = brain.open_control()?;
    if let Some(l) = label {
        let l = l.trim();
        if l.chars().count() > MAX_NAME || l.chars().any(char::is_control) {
            return Err(Error::Invalid(format!(
                "a name is up to {MAX_NAME} characters, without control characters"
            )));
        }
        ctl.execute(
            "INSERT INTO peer_meta (peer, label) VALUES (?1, ?2)
             ON CONFLICT(peer) DO UPDATE SET label = excluded.label",
            params![peer, (!l.is_empty()).then_some(l)],
        )?;
    }
    if let Some(n) = note {
        let n = n.trim();
        if n.chars().count() > MAX_NOTE {
            return Err(Error::Invalid(format!(
                "a note is up to {MAX_NOTE} characters"
            )));
        }
        ctl.execute(
            "INSERT INTO peer_meta (peer, note) VALUES (?1, ?2)
             ON CONFLICT(peer) DO UPDATE SET note = excluded.note",
            params![peer, n],
        )?;
    }
    auth_event(&ctl, Some(&who.id), None, "node_update", peer);
    get(brain, who, peer, util::now())
}

/// Pauses or resumes the sync with a machine: this node refuses its sync requests and does not contact it, until
/// resumed. Nothing is lost: it resumes where it stopped.
pub fn set_paused(brain: &Brain, who: &User, peer: &str, paused: bool) -> Result<Node> {
    check(brain, who, peer)?;
    let ctl = brain.open_control()?;
    ctl.execute(
        "INSERT INTO peer_meta (peer, paused_at) VALUES (?1, ?2)
         ON CONFLICT(peer) DO UPDATE SET paused_at = excluded.paused_at",
        params![peer, paused.then(util::now)],
    )?;
    auth_event(
        &ctl,
        Some(&who.id),
        None,
        if paused { "node_pause" } else { "node_resume" },
        peer,
    );
    get(brain, who, peer, util::now())
}

/// Whether the sync with this machine is paused (by the name this node knows it under).
pub fn is_paused(ctl: &Connection, peer: &str) -> bool {
    ctl.query_row(
        "SELECT paused_at IS NOT NULL FROM peer_meta WHERE peer = ?1",
        [peer],
        |r| r.get::<_, bool>(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// Revokes the machine's key: it stops at once. The entry stays, marked revoked, with its history.
pub fn revoke(brain: &Brain, who: &User, peer: &str) -> Result<Node> {
    check(brain, who, peer)?;
    let ctl = brain.open_control()?;
    if !access::revoke(&ctl, peer)? {
        return Err(Error::Invalid(format!(
            "{peer} holds no active key here: forget it instead"
        )));
    }
    auth_event(&ctl, Some(&who.id), None, "node_revoke", peer);
    get(brain, who, peer, util::now())
}

/// Whether a machine can be forgotten now: revoked, or silent for [`SILENT_SECS`].
pub fn may_forget(n: &Node, now: i64) -> bool {
    n.state == "revoked"
        || n.last_seen()
            .or(n.created_at)
            .is_none_or(|t| now - t >= SILENT_SECS)
}

/// Removes a revoked or silent machine from the lists. Its progress rows go; the connection row and the audit stay.
pub fn forget(brain: &Brain, who: &User, peer: &str, now: i64) -> Result<()> {
    check(brain, who, peer)?;
    let n = get(brain, who, peer, now)?;
    if !may_forget(&n, now) {
        return Err(Error::Invalid(format!(
            "{peer} is still in use: revoke it first, or wait until it has been silent for 30 days"
        )));
    }
    let ctl = brain.open_control()?;
    ctl.execute("DELETE FROM sync_state WHERE peer = ?1", [peer])?;
    ctl.execute(
        "INSERT INTO peer_meta (peer, forgotten_at) VALUES (?1, ?2)
         ON CONFLICT(peer) DO UPDATE SET forgotten_at = excluded.forgotten_at",
        params![peer, now],
    )?;
    auth_event(&ctl, Some(&who.id), None, "node_forget", peer);
    Ok(())
}
