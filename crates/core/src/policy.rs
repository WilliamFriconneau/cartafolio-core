//! The sync policy per space and per machine: what THIS node does with one machine of the person, space by space.
//!
//! - **`both`** (the default): this node sends its events of the space to that machine and takes the machine's.
//! - **`send`**: it sends, and takes none.
//! - **`receive`**: it takes, and sends none.
//! - **`off`**: neither. The space is not listed to that machine, not offered to it, not made here from its word, and
//!   its events neither go nor come. What the machine already holds stays on it: `off` erases nothing.
//!
//! A choice is kept per machine under the name this node knows it by (the connection of a machine that reaches this
//! node, `peer-<machine>-xxxx`; the name a pass keeps the machine it reaches under, `peer-<node>`), the same name as a
//! pause. A machine also has a choice for **the spaces made later** (`both` by default): a space with no choice of its
//! own follows it. Setting it fixes the spaces already here at what they were, so it only ever changes what comes next.
//!
//! **Each node enforces its own side**, on every path, whichever node runs the pass: the passes it runs (`pull`,
//! `push`, offers, archives) and the requests it serves (`/sync/spaces`, `/sync/events`). The policy does not travel:
//! a node never applies another node's choice. Each node tells its side to the other for the page (`policy` in the
//! list of spaces it serves, `policy` in the directory it gives), kept here as `theirs`, which decides nothing.
//!
//! With nothing set, everything goes both ways as before: the person's machines keep every space, present and future.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::access::auth_event;
use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::space::Space;
use crate::users::{self, User};

/// The row that holds a machine's choice for the spaces made later.
const LATER: &str = "*";

/// What this node does with one space and one machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Sends and takes (the default).
    Both,
    /// Sends, takes none.
    Send,
    /// Takes, sends none.
    Receive,
    /// Neither: the space is kept apart from that machine.
    Off,
}

impl Policy {
    /// Reads `both`, `send`, `receive` or `off`.
    ///
    /// # Errors
    /// [`Error::Invalid`] for any other word.
    pub fn parse(word: &str) -> Result<Policy> {
        match word.trim() {
            "both" => Ok(Policy::Both),
            "send" => Ok(Policy::Send),
            "receive" => Ok(Policy::Receive),
            "off" => Ok(Policy::Off),
            w => Err(Error::Invalid(format!(
                "a policy is both, send, receive or off, not {w:?}"
            ))),
        }
    }

    /// The word: `both`, `send`, `receive` or `off`.
    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Both => "both",
            Policy::Send => "send",
            Policy::Receive => "receive",
            Policy::Off => "off",
        }
    }

    /// Whether this node sends its events (and offers, lists, archives) to the machine.
    pub fn gives(self) -> bool {
        matches!(self, Policy::Both | Policy::Send)
    }

    /// Whether this node takes the machine's events (and its offers and archives).
    pub fn takes(self) -> bool {
        matches!(self, Policy::Both | Policy::Receive)
    }
}

fn read(ctl: &Connection, peer: &str, space: &str) -> Result<Option<Policy>> {
    let word: Option<String> = ctl
        .query_row(
            "SELECT policy FROM sync_policy WHERE peer = ?1 AND space = ?2 AND policy IS NOT NULL",
            params![peer, space],
            |r| r.get(0),
        )
        .optional()?;
    word.map(|w| Policy::parse(&w)).transpose()
}

/// The choice for the spaces made later with `peer`: `both` when none was made.
pub fn later(ctl: &Connection, peer: &str) -> Result<Policy> {
    Ok(read(ctl, peer, LATER)?.unwrap_or(Policy::Both))
}

/// What this node does with `space` (its name here) and `peer` (the name it knows the machine by): the choice for that
/// space, else the choice for the spaces made later, else `both`.
pub fn of(ctl: &Connection, peer: &str, space: &str) -> Result<Policy> {
    match read(ctl, peer, space)? {
        Some(p) => Ok(p),
        None => later(ctl, peer),
    }
}

/// The machine must be one of the person's (as the list of machines shows it), else nothing is set or shown.
fn check_peer(brain: &Brain, who: &User, peer: &str) -> Result<()> {
    crate::peers::get(brain, who, peer, crate::util::now()).map(|_| ())
}

fn upsert(ctl: &Connection, peer: &str, space: &str, p: Policy) -> Result<()> {
    ctl.execute(
        "INSERT INTO sync_policy (peer, space, policy, set_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(peer, space) DO UPDATE SET policy = excluded.policy, set_at = excluded.set_at",
        params![peer, space, p.as_str(), crate::util::now()],
    )?;
    Ok(())
}

/// The person sets what this node does with one of their spaces and one of their machines. Audited.
///
/// # Errors
/// [`Error::Invalid`] when the machine is not one of the person's, or the space not theirs.
pub fn set(brain: &Brain, who: &User, peer: &str, space: &Space, p: Policy) -> Result<()> {
    check_peer(brain, who, peer)?;
    let ctl = brain.open_control()?;
    if !users::is_space_owner(&ctl, space, &who.id)? {
        return Err(Error::Invalid(format!("{space} is not a space of yours")));
    }
    upsert(&ctl, peer, space.as_str(), p)?;
    auth_event(
        &ctl,
        Some(&who.id),
        None,
        "sync_policy",
        &format!("{peer} {space} {}", p.as_str()),
    );
    Ok(())
}

/// The person sets what this node does with the spaces made later and one of their machines. The spaces of the person
/// here that had no choice of their own keep what they had (the former choice for spaces made later): only what comes
/// next follows the new one. Audited.
///
/// # Errors
/// [`Error::Invalid`] when the machine is not one of the person's.
pub fn set_later(brain: &Brain, who: &User, peer: &str, p: Policy) -> Result<()> {
    check_peer(brain, who, peer)?;
    let ctl = brain.open_control()?;
    let was = later(&ctl, peer)?;
    for place in brain.layout(&who.id)? {
        if read(&ctl, peer, place.space.as_str())?.is_none() {
            upsert(&ctl, peer, place.space.as_str(), was)?;
        }
    }
    upsert(&ctl, peer, LATER, p)?;
    auth_event(
        &ctl,
        Some(&who.id),
        None,
        "sync_policy",
        &format!("{peer} later {}", p.as_str()),
    );
    Ok(())
}

/// What `peer` told of its own side for `space` (its name here; `None` for the spaces made later is not used: see
/// [`note_theirs_later`]). For the page only.
pub fn note_theirs(ctl: &Connection, peer: &str, space: &str, p: Option<Policy>) -> Result<()> {
    ctl.execute(
        "INSERT INTO sync_policy (peer, space, theirs, theirs_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(peer, space) DO UPDATE SET theirs = excluded.theirs, theirs_at = excluded.theirs_at",
        params![peer, space, p.map(Policy::as_str), crate::util::now()],
    )?;
    Ok(())
}

/// What `peer` told of its own choice for the spaces made later. For the page only.
pub fn note_theirs_later(ctl: &Connection, peer: &str, p: Option<Policy>) -> Result<()> {
    note_theirs(ctl, peer, LATER, p)
}

/// Forgets what `peer` told of its side, before it tells it again (a space it no longer lists is unknown).
pub fn clear_theirs(ctl: &Connection, peer: &str) -> Result<()> {
    ctl.execute(
        "UPDATE sync_policy SET theirs = NULL, theirs_at = NULL WHERE peer = ?1",
        [peer],
    )?;
    Ok(())
}

/// The choice of this node for each space of `user` with `peer`, as the peer is told it: `{space: policy}`, the spaces
/// made later under `*`.
pub fn sides(brain: &Brain, user: &str, peer: &str) -> Result<Value> {
    let ctl = brain.open_control()?;
    let mut out = serde_json::Map::new();
    for place in brain.layout(user)? {
        out.insert(
            place.space.as_str().to_string(),
            json!(of(&ctl, peer, place.space.as_str())?.as_str()),
        );
    }
    out.insert(LATER.into(), json!(later(&ctl, peer)?.as_str()));
    Ok(Value::Object(out))
}

/// A machine's name is moved with its connection's (`access::rename`): its policy follows.
pub(crate) fn rename(ctl: &Connection, from: &str, to: &str) -> Result<()> {
    ctl.execute(
        "UPDATE OR REPLACE sync_policy SET peer = ?2 WHERE peer = ?1",
        params![from, to],
    )?;
    Ok(())
}

/// Whether a space holds health or money, by its kind, its name or its label: the page marks it next to its policy.
pub fn sensitive(kind: &str, name: &str, label: &str) -> bool {
    const WORDS: [&str; 7] = [
        "health", "sante", "money", "finances", "finance", "argent", "budget",
    ];
    let hit = |s: &str| {
        let s = crate::text::fold(s);
        WORDS
            .iter()
            .any(|w| s == *w || s.ends_with(&format!("-{w}")) || s.ends_with(&format!(" {w}")))
    };
    kind == "health" || kind == "money" || hit(name) || hit(label)
}

/// What the page shows for one machine: the choice for each space of the person and for the spaces made later, what
/// the machine told of its own side, a mark on health and money, and whether the two already exchanged the space (an
/// `off` erases nothing: what the machine holds stays on it).
///
/// # Errors
/// [`Error::Invalid`] when the machine is not one of the person's.
pub fn view(brain: &Brain, who: &User, peer: &str) -> Result<Value> {
    check_peer(brain, who, peer)?;
    let ctl = brain.open_control()?;
    let theirs = |space: &str| -> Result<Option<String>> {
        Ok(ctl
            .query_row(
                "SELECT theirs FROM sync_policy WHERE peer = ?1 AND space = ?2",
                params![peer, space],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    };
    let mut spaces = vec![];
    for place in brain.layout(&who.id)? {
        let name = place.space.as_str();
        let exchanged: bool = ctl.query_row(
            "SELECT EXISTS (SELECT 1 FROM sync_state WHERE peer = ?1 AND space = ?2)",
            params![peer, name],
            |r| r.get(0),
        )?;
        spaces.push(json!({
            "space": name,
            "label": place.label,
            "kind": place.kind,
            "parent": place.parent.as_ref().map(Space::as_str),
            "policy": of(&ctl, peer, name)?.as_str(),
            "set": read(&ctl, peer, name)?.is_some(),
            "theirs": theirs(name)?,
            "sensitive": sensitive(&place.kind, name, &place.label),
            "exchanged": exchanged,
        }));
    }
    Ok(json!({
        "peer": peer,
        "later": later(&ctl, peer)?.as_str(),
        "later_theirs": theirs(LATER)?,
        "spaces": spaces,
    }))
}
