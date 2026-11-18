//! The nodes of one person form a swarm: every node is equal, and a space that exists on any of them exists on all of
//! them. This module holds what the exchange of events (`sync`) does not: which spaces exist, and which were archived.
//!
//! - **A space made on one node is offered to the others** ([`offers`], [`accept`]): its name, kind, label, parent and
//!   position. The node that receives it makes it for the same person and from then on its events travel like those
//!   of any space, layout included (`layout_changed` events, the newest stamp wins per field).
//! - **Inbox is one box across the swarm.** Each node has one for the person, perhaps under another name: the
//!   exchange pairs the person's box here with the peer's ([`accept`] answers [`Answer::Triage`]), so a note captured
//!   on any node lands in the box of every node.
//! - **Archiving travels as an archive, never as a deletion.** A node that archives a space keeps a mark (the space,
//!   its owner, when). A peer that still holds the space archives its own copy ([`archive_here`]) when its copy began
//!   before that time ([`since`]); a copy made or brought back later wins, and the space comes back everywhere.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::brain::Brain;
use crate::catalogue::{CUSTOM, TRIAGE};
use crate::error::{Error, Result};
use crate::lifecycle::{Children, Disposition};
use crate::space::Space;
use crate::users::is_space_owner;

const ARCHIVED: &str = "swarm_archived:";
const RESTORED: &str = "swarm_restored:";
/// How far ahead of this node's clock the time of an archive may be, in seconds: a peer's clock drifts, and a time
/// further ahead would win over every copy made or brought back until then.
const SKEW: i64 = 300;

/// The time of an archive as this node keeps it: never further ahead than its clock allows.
fn bounded(at: i64) -> i64 {
    at.min(crate::util::now().saturating_add(SKEW))
}

fn meta(ctl: &Connection, k: &str) -> Result<Option<String>> {
    Ok(ctl
        .query_row("SELECT v FROM meta WHERE k = ?1", [k], |r| r.get(0))
        .optional()?)
}

fn meta_set(ctl: &Connection, k: &str, v: &str) -> Result<()> {
    ctl.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![k, v],
    )?;
    Ok(())
}

fn meta_del(ctl: &Connection, k: &str) -> Result<()> {
    ctl.execute("DELETE FROM meta WHERE k = ?1", [k])?;
    Ok(())
}

/// A mark kept before archive times were bounded may lie further ahead: it is brought back within bounds, once, so it
/// stops winning over every copy brought back.
fn settle_marks(ctl: &Connection) -> Result<()> {
    ctl.execute(
        "UPDATE meta SET v = json_set(v, '$.at', ?1)
         WHERE k LIKE 'swarm_archived:%' AND json_valid(v) AND json_extract(v, '$.at') > ?1",
        [crate::util::now().saturating_add(SKEW)],
    )?;
    Ok(())
}

/// When `space` was archived on this node or on a node it heard from, and by whose decision: `(owner, at)`.
pub fn archived_at(brain: &Brain, space: &Space) -> Result<Option<(String, i64)>> {
    let ctl = brain.open_control()?;
    settle_marks(&ctl)?;
    Ok(meta(&ctl, &format!("{ARCHIVED}{space}"))?.and_then(|v| {
        let v: Value = serde_json::from_str(&v).ok()?;
        Some((v["owner"].as_str()?.to_string(), v["at"].as_i64()?))
    }))
}

/// Keeps the mark of an archive (the newest one wins), so it travels to the other nodes.
pub(crate) fn note_archived(brain: &Brain, space: &Space, owner: &str, at: i64) -> Result<()> {
    let at = bounded(at);
    if archived_at(brain, space)?.is_some_and(|(_, was)| was >= at) {
        return Ok(());
    }
    let ctl = brain.open_control()?;
    meta_set(
        &ctl,
        &format!("{ARCHIVED}{space}"),
        &json!({ "owner": owner, "at": at }).to_string(),
    )?;
    meta_del(&ctl, &format!("{RESTORED}{space}"))
}

/// A space brought back from its archive: the mark goes, and the copy counts as begun now, or just after the archive
/// it comes back from if that one is stamped later (a clock ahead): a space brought back always wins over the archives
/// this node knew of.
pub(crate) fn note_restored(brain: &Brain, space: &Space, at: i64) -> Result<()> {
    let at = match archived_at(brain, space)? {
        Some((_, archived)) => at.max(archived.saturating_add(1)),
        None => at,
    };
    let ctl = brain.open_control()?;
    meta_del(&ctl, &format!("{ARCHIVED}{space}"))?;
    meta_set(&ctl, &format!("{RESTORED}{space}"), &at.to_string())
}

/// When this node's copy of `space` began, in seconds: the time the space was made at its origin (a copy made from a
/// peer takes the peer's), or the time it was brought back from an archive, whichever is later.
pub fn since(brain: &Brain, space: &Space) -> Result<i64> {
    let ctl = brain.open_control()?;
    let made: i64 = ctl
        .query_row(
            "SELECT created_at FROM spaces WHERE name = ?1",
            [space.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::UnknownSpace(space.to_string()))?;
    let back = meta(&ctl, &format!("{RESTORED}{space}"))?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    Ok(made.max(back))
}

/// The archives of `owner`'s spaces this node knows of: `(space, at)`, by name.
pub fn tombstones(brain: &Brain, owner: &str) -> Result<Vec<(Space, i64)>> {
    let ctl = brain.open_control()?;
    settle_marks(&ctl)?;
    let mut st = ctl.prepare("SELECT k, v FROM meta WHERE k LIKE 'swarm_archived:%' ORDER BY k")?;
    let rows: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (k, v) in rows {
        let Ok(space) = Space::new(k.trim_start_matches(ARCHIVED)) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&v) else {
            continue;
        };
        if v["owner"].as_str() == Some(owner)
            && let Some(at) = v["at"].as_i64()
        {
            out.push((space, at));
        }
    }
    Ok(out)
}

/// A space as one node offers it to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Its name, the same on every node (Inbox excepted, see [`Answer::Triage`]).
    pub space: Space,
    /// Its kind (`custom` when it was made without one; `a-trier` for the box).
    pub kind: String,
    /// The label the person gave it, if any.
    pub label: Option<String>,
    /// The space it sits under, if any.
    pub parent: Option<Space>,
    /// Its order among its brothers.
    pub position: i64,
    /// When the offering node's copy began (see [`since`]).
    pub since: i64,
}

impl Offer {
    /// The wire form: `{space, kind, label, parent, position, since}`.
    pub fn to_json(&self) -> Value {
        json!({
            "space": self.space.as_str(), "kind": self.kind, "label": self.label,
            "parent": self.parent.as_ref().map(Space::as_str), "position": self.position, "since": self.since,
        })
    }

    /// Reads the wire form.
    ///
    /// # Errors
    /// [`Error::Invalid`] when `space` (or `parent`) is not a space name, or `since` is missing.
    pub fn from_json(v: &Value) -> Result<Offer> {
        let space = Space::new(v["space"].as_str().unwrap_or_default())?;
        let parent = match v["parent"].as_str() {
            Some(p) => Some(Space::new(p)?),
            None => None,
        };
        let label = match v["label"].as_str() {
            Some(l) => Some(crate::layout::clean_label(l)?),
            None => None,
        };
        Ok(Offer {
            space,
            kind: v["kind"].as_str().unwrap_or(CUSTOM).to_string(),
            label,
            parent,
            position: v["position"].as_i64().unwrap_or(0),
            since: v["since"]
                .as_i64()
                .ok_or_else(|| Error::Invalid("since: when the copy began, in seconds".into()))?,
        })
    }
}

/// What a node answers to an offered space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Made here now; its events are welcome.
    Created(Space),
    /// Already here, the same person's.
    Held(Space),
    /// The offer was the person's Inbox: this node's box is that one, under this name. The two are paired.
    Triage(Space),
    /// This node archived the space after the offering node's copy began: the offering node archives its copy too.
    Archived(i64),
    /// This node takes nothing of that space from the offering machine (its policy there is `send` or `off`): nothing
    /// was made, and nothing of it is exchanged.
    Declined(Space),
}

impl Answer {
    /// `{result: created|held|triage|declined, space}` or `{result: archived, archived_at}`.
    pub fn to_json(&self) -> Value {
        match self {
            Answer::Created(s) => json!({ "result": "created", "space": s.as_str() }),
            Answer::Held(s) => json!({ "result": "held", "space": s.as_str() }),
            Answer::Triage(s) => json!({ "result": "triage", "space": s.as_str() }),
            Answer::Archived(at) => json!({ "result": "archived", "archived_at": at }),
            Answer::Declined(s) => json!({
                "result": "declined", "space": s.as_str(),
                "why": "the person set this space apart from this machine on that node",
            }),
        }
    }

    /// Reads the wire form.
    ///
    /// # Errors
    /// [`Error::Invalid`] for an answer of another shape.
    pub fn from_json(v: &Value) -> Result<Answer> {
        let space = || Space::new(v["space"].as_str().unwrap_or_default());
        match v["result"].as_str() {
            Some("created") => Ok(Answer::Created(space()?)),
            Some("held") => Ok(Answer::Held(space()?)),
            Some("triage") => Ok(Answer::Triage(space()?)),
            Some("declined") => Ok(Answer::Declined(space()?)),
            Some("archived") => v["archived_at"]
                .as_i64()
                .map(Answer::Archived)
                .ok_or_else(|| Error::Invalid("archived_at is missing".into())),
            _ => Err(Error::Invalid(format!("an answer to an offer: {v}"))),
        }
    }
}

/// The spaces of `user` this node offers to a peer, parents before their children: every space the person owns here,
/// Inbox excepted (it is paired, not offered, unless the peer has none: see the caller).
pub fn offers(brain: &Brain, user: &str) -> Result<Vec<Offer>> {
    let ctl = brain.open_control()?;
    let mut out = vec![];
    for p in brain.layout(user)? {
        let label: Option<String> = ctl
            .query_row(
                "SELECT label FROM spaces WHERE name = ?1",
                [p.space.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        out.push(Offer {
            since: since(brain, &p.space)?,
            space: p.space,
            kind: p.kind,
            label,
            parent: p.parent,
            position: p.position,
        });
    }
    Ok(out)
}

/// The spaces of `user` this node offers to the machine it knows as `peer`: [`offers`], less those it sends nothing of
/// to that machine (`policy::Policy::gives`: a space set `receive` or `off` for it, or made later under such a choice).
pub fn offers_to(brain: &Brain, user: &str, peer: &str) -> Result<Vec<Offer>> {
    let ctl = brain.open_control()?;
    let mut out = vec![];
    for o in offers(brain, user)? {
        if crate::policy::of(&ctl, peer, o.space.as_str())?.gives() {
            out.push(o);
        }
    }
    Ok(out)
}

/// Makes `space` here for `user` when a peer has it and this node does not, as the peer's copy: its kind, and the time
/// its copy began (so an archive decided between the two is judged the same way everywhere). Its layout comes with its
/// events. `true` when it was made now.
///
/// # Errors
/// [`Error::Invalid`] when a space of that name here belongs to another person: the two are never paired.
pub fn adopt(brain: &Brain, user: &str, space: &Space, kind: &str, since: i64) -> Result<bool> {
    let set_kind = |brain: &Brain| -> Result<()> {
        if kind == TRIAGE {
            // Inbox is not in the catalogue (`set_space_kind` refuses it), yet it must stay one here.
            brain.open_control()?.execute(
                "UPDATE spaces SET kind = ?2 WHERE name = ?1",
                [space.as_str(), TRIAGE],
            )?;
            Ok(())
        } else {
            brain.set_space_kind(space, kind)
        }
    };
    let other_person = || {
        Error::Invalid(format!(
            "{space} is the name of a space of another person here: it is not exchanged"
        ))
    };
    if brain.has_space(space)? {
        // A space of that name that is not this person's is never paired with theirs: its notes would cross from one
        // person to another.
        if !is_space_owner(&brain.open_control()?, space, user)? {
            return Err(other_person());
        }
        // A box that an earlier version made `custom` here is put right.
        if kind == TRIAGE && brain.space_kind(space)? == CUSTOM {
            set_kind(brain)?;
        }
        return Ok(false);
    }
    if let Err(e) = brain.add_space_for(space, user) {
        // Another pass of this node (it runs one per peer, each its own process) may be making the same space at this
        // moment: once it is made, it is held here, as above.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !brain.has_space(space)? && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if brain.has_space(space)? && is_space_owner(&brain.open_control()?, space, user)? {
            return Ok(false);
        }
        return Err(e);
    }
    // A kind this node's catalogue does not know stays `custom`.
    let _ = set_kind(brain);
    let ctl = brain.open_control()?;
    ctl.execute(
        "UPDATE spaces SET created_at = ?2 WHERE name = ?1",
        params![space.as_str(), since],
    )?;
    // The space is back: an older archive mark of it here no longer holds.
    if archived_at(brain, space)?.is_some_and(|(_, at)| at < since) {
        meta_del(&ctl, &format!("{ARCHIVED}{space}"))?;
    }
    crate::sync::mark_layout_logged(&brain.open_space(space)?)?;
    Ok(true)
}

/// A peer of `user` offers a space. See [`Answer`].
///
/// # Errors
/// [`Error::Invalid`] when the name is held here by a space of another person.
pub fn accept(brain: &Brain, user: &str, offer: &Offer) -> Result<Answer> {
    // A name held here by a space of another person is never taken, whatever the offer says it is (a box offered
    // under that name would otherwise turn that person's space into a box).
    let other_person = || {
        Error::Invalid(format!(
            "{} is the name of a space of another person here",
            offer.space
        ))
    };
    let held_by_other = brain.has_space(&offer.space)?
        && !is_space_owner(&brain.open_control()?, &offer.space, user)?;
    if offer.kind == TRIAGE {
        if let Some(mine) = crate::triage::of(brain, user)? {
            return Ok(Answer::Triage(mine));
        }
        if held_by_other {
            return Err(other_person());
        }
        adopt(brain, user, &offer.space, TRIAGE, offer.since)?;
        return Ok(Answer::Created(offer.space.clone()));
    }
    if brain.has_space(&offer.space)? {
        if !held_by_other {
            return Ok(Answer::Held(offer.space.clone()));
        }
        return Err(other_person());
    }
    if let Some((_, at)) = archived_at(brain, &offer.space)?
        && at >= offer.since
    {
        return Ok(Answer::Archived(at));
    }
    adopt(brain, user, &offer.space, &offer.kind, offer.since)?;
    // The layout the offer carries, so the space shows in its place at once; the events that follow say the same
    // under their own stamps. Not recorded as events of this node (see `adopt`).
    let ctl = brain.open_control()?;
    let parent = offer
        .parent
        .as_ref()
        .filter(|p| is_space_owner(&ctl, p, user).unwrap_or(false));
    ctl.execute(
        "UPDATE spaces SET label = ?2, parent = ?3, position = ?4 WHERE name = ?1",
        params![
            offer.space.as_str(),
            offer.label,
            parent.map(Space::as_str),
            offer.position
        ],
    )?;
    Ok(Answer::Created(offer.space.clone()))
}

/// The machine this node knows as `peer` offers a space: [`accept`], unless this node takes nothing of that space from
/// it ([`Answer::Declined`]; the person's box is judged by its name here).
///
/// # Errors
/// As [`accept`].
pub fn accept_from(brain: &Brain, user: &str, peer: &str, offer: &Offer) -> Result<Answer> {
    let here = if offer.kind == TRIAGE {
        crate::triage::of(brain, user)?.unwrap_or_else(|| offer.space.clone())
    } else {
        offer.space.clone()
    };
    if !crate::policy::of(&brain.open_control()?, peer, here.as_str())?.takes() {
        return Ok(Answer::Declined(offer.space.clone()));
    }
    accept(brain, user, offer)
}

/// The machine this node knows as `peer` says `space` was archived at `at`: [`archive_here`], unless this node takes
/// nothing of that space from it. Then nothing is archived and no mark is kept.
///
/// # Errors
/// As [`archive_here`].
pub fn archive_from(brain: &Brain, user: &str, peer: &str, space: &Space, at: i64) -> Result<bool> {
    if !crate::policy::of(&brain.open_control()?, peer, space.as_str())?.takes() {
        return Ok(false);
    }
    archive_here(brain, user, space, at)
}

/// A peer says `space` was archived at `at`: this node archives its copy of it, as an archive (exported, set aside,
/// never erased), when the copy is `user`'s and began before then. Inbox is never archived. When the space is not
/// here the mark is kept, so it travels on to the next node. `true` when this node archived it now.
pub fn archive_here(brain: &Brain, user: &str, space: &Space, at: i64) -> Result<bool> {
    // The time comes from a peer: bounded before it is compared or kept.
    let at = bounded(at);
    if !brain.has_space(space)? {
        note_archived(brain, space, user, at)?;
        return Ok(false);
    }
    if !is_space_owner(&brain.open_control()?, space, user)?
        || crate::triage::is_triage(brain, space)
        || since(brain, space)? >= at
    {
        return Ok(false);
    }
    brain.delete_space(
        user,
        space,
        &Disposition::Archive,
        &Children::Grandparent,
        false,
        at,
    )?;
    Ok(true)
}
