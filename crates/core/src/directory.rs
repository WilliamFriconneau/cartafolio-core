//! The peer directory: every node knows every peer of the memory, like the peers of a torrent swarm, and not only the
//! ones that joined it.
//!
//! Each node writes its OWN entry (its name, the program version, when it was last active) at every synchronisation
//! pass and when it serves a synchronisation request. Nodes pass their directories to each other at each pass
//! (`GET` and `POST /sync/peers`): per name the entry with the newest `last_seen` wins, an entry is never made up
//! (a time in the future is held back to the receiver's clock) and never removed. The only way out is the person
//! revoking the peer: the entry then carries a `revoked` flag, which sticks and spreads like the rest, and the peer
//! is no longer listed. An entry not seen for [`GONE_AFTER`] seconds is listed as `gone`, and kept.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::{Brain, util};

/// A peer not seen for this long (90 days) is shown as gone.
pub const GONE_AFTER: i64 = 90 * 86_400;
/// An entry told to be later than this much after the receiver's clock is held back to the receiver's clock.
const SKEW: i64 = 300;
/// The own entry is rewritten at most this often (batteries and SSDs matter).
const TOUCH_EVERY: i64 = 30;
/// Entries taken from one exchange: a directory is small, anything bigger is not one.
const MAX_ENTRIES: usize = 10_000;

/// The name stored on first use if no name was given.
pub const DEFAULT_NODE_NAME: &str = "Silice";

const NAME_KEY: &str = "node_name";

/// The version of this program, as peers tell each other.
pub fn own_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// One row of the directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The node's name.
    pub name: String,
    /// When this node was last seen alive (unix seconds, never in the future).
    pub last_seen: i64,
    /// The program version the node reported.
    pub version: String,
    /// When this node first learned of the entry.
    pub first_seen: i64,
    /// The node that told this one about the entry (`None`: it is this node's own).
    pub learned_from: Option<String>,
    /// `last_seen` and the node the observation comes from: which write the row reflects.
    pub stamp: String,
    /// The name of the direct connection (here or on the peer) this node is, when it is one.
    pub link: Option<String>,
    /// Whether the person revoked this node (the flag spreads and sticks).
    pub revoked: bool,
    /// When it was revoked.
    pub revoked_at: Option<i64>,
    /// How the node can be reached: `[{kind, address?}]`, `kind` one of [`KINDS`]. Empty when not known.
    pub exposures: Vec<Value>,
    /// The node's identity (`ABCD-EFGH-IJKL`, see [`node_id`]); empty for a node that does not tell it yet.
    pub node_id: String,
}

/// The ways a node can be reached: this machine only, the local network, the tailnet, the internet.
pub const KINDS: [&str; 4] = ["loopback", "lan", "tailnet", "public"];

/// A list of exposures as a peer told it: known kinds only, an address at most 200 characters, each kind once.
fn clean_exposures(v: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for e in v.as_array().into_iter().flatten().take(16) {
        let Some(kind) = e["kind"].as_str().filter(|k| KINDS.contains(k)) else {
            continue;
        };
        if out.iter().any(|o| o["kind"] == kind) {
            continue;
        }
        let address: Option<String> = e["address"]
            .as_str()
            .map(|a| a.chars().filter(|c| !c.is_control()).take(200).collect())
            .filter(|a: &String| !a.is_empty());
        out.push(match address {
            Some(a) => json!({ "kind": kind, "address": a }),
            None => json!({ "kind": kind }),
        });
    }
    out
}

/// Whether a node can be reached by nothing but this machine or its local network: it syncs when it is opened,
/// with the peers it can reach. A node whose exposures are unknown is not said to be.
fn only_local(exposures: &[Value]) -> bool {
    !exposures.is_empty()
        && exposures
            .iter()
            .all(|e| matches!(e["kind"].as_str(), Some("loopback" | "lan")))
}

/// What a node can be reached by, from its settings: the address it listens on, the public address it is told
/// to have, and the tailnet address when it serves one. A loopback listener is `loopback`; any other listener is
/// the local network (`lan`); a public address on the tailnet's domain (`.ts.net`), or `tailnet` given, is
/// `tailnet`; any other public address that is not private is `public`.
pub fn exposures_from(listen: &str, public_url: &str, tailnet: Option<&str>) -> Vec<Value> {
    let host = |u: &str| {
        let rest = u.split("://").nth(1).unwrap_or(u);
        let h = rest.split('/').next().unwrap_or("");
        let h = h.rsplit('@').next().unwrap_or("");
        if let Some(v6) = h.strip_prefix('[') {
            v6.split(']').next().unwrap_or("").to_string()
        } else {
            h.split(':').next().unwrap_or("").to_lowercase()
        }
    };
    let loopback = |h: &str| h == "localhost" || h.starts_with("127.") || h == "::1";
    let private = |h: &str| {
        loopback(h)
            || h.ends_with(".local")
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| is_private(&ip))
    };
    let mut out: Vec<Value> = Vec::new();
    let lh = host(listen);
    if loopback(&lh) {
        out.push(json!({ "kind": "loopback", "address": listen }));
    } else if !lh.is_empty() {
        out.push(json!({ "kind": "lan", "address": listen }));
    }
    let ph = host(public_url);
    if !ph.is_empty() && !loopback(&ph) {
        if ph.ends_with(".ts.net") {
            out.push(json!({ "kind": "tailnet", "address": public_url }));
        } else if private(&ph) {
            if !out.iter().any(|e| e["kind"] == "lan") {
                out.push(json!({ "kind": "lan", "address": public_url }));
            }
        } else {
            out.push(json!({ "kind": "public", "address": public_url }));
        }
    }
    if let Some(t) = tailnet.filter(|t| !t.is_empty())
        && !out.iter().any(|e| e["kind"] == "tailnet")
    {
        out.push(json!({ "kind": "tailnet", "address": t }));
    }
    clean_exposures(&Value::Array(out))
}

/// Loopback, private, link-local, shared (tailnet) or unique-local addresses: a program of the person's own
/// machines, not a service on the internet.
fn is_private(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        std::net::IpAddr::V6(v) => {
            v.is_loopback()
                || (v.segments()[0] & 0xfe00) == 0xfc00
                || (v.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

const EXPOSURES_KEY: &str = "node_exposures";

/// Keeps how this node can be reached (see [`exposures_from`]) and puts it in its own entry.
pub fn set_exposures(brain: &Brain, exposures: &[Value]) -> Result<()> {
    let list = clean_exposures(&Value::Array(exposures.to_vec()));
    let ctl = brain.open_control()?;
    let text = Value::Array(list).to_string();
    ctl.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![EXPOSURES_KEY, text],
    )?;
    ctl.execute(
        "UPDATE peer_directory SET exposures = ?2 WHERE name = ?1",
        params![own_name_in(&ctl)?, text],
    )?;
    Ok(())
}

fn own_exposures(ctl: &Connection) -> Result<String> {
    Ok(meta_get(ctl, EXPOSURES_KEY)?.unwrap_or_else(|| "[]".into()))
}

impl Entry {
    /// What goes over the wire: no link (it is local) and no `learned_from` (the receiver says who told it).
    pub fn to_wire(&self) -> Value {
        json!({
            "name": self.name,
            "last_seen": self.last_seen,
            "version": self.version,
            "first_seen": self.first_seen,
            "revoked": self.revoked,
            "revoked_at": self.revoked_at,
            "exposures": self.exposures,
            "id": (!self.node_id.is_empty()).then_some(&self.node_id),
        })
    }
}

fn valid_name(name: &str) -> bool {
    let n = name.trim();
    n == name && !n.is_empty() && n.chars().count() <= 64 && !n.chars().any(char::is_control)
}

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT v FROM meta WHERE k = ?1", [key], |r| r.get(0))
        .optional()?)
}

/// The name already recorded for this node, without generating one.
pub fn stored_own_name(brain: &Brain) -> Result<Option<String>> {
    meta_get(&brain.open_control()?, NAME_KEY)
}

/// This node's stored name, defaulting to Silice on first use.
pub fn own_name(brain: &Brain) -> Result<String> {
    let ctl = brain.open_control()?;
    own_name_in(&ctl)
}

fn own_name_in(ctl: &Connection) -> Result<String> {
    if let Some(n) = meta_get(ctl, NAME_KEY)? {
        return Ok(n);
    }
    ctl.execute(
        "INSERT OR IGNORE INTO meta (k, v) VALUES (?1, ?2)",
        params![NAME_KEY, DEFAULT_NODE_NAME],
    )?;
    meta_get(ctl, NAME_KEY)?.ok_or_else(|| Error::Schema("node name not written".into()))
}

/// Gives this node the name it goes by in the directory.
pub fn set_own_name(brain: &Brain, name: &str) -> Result<()> {
    if !valid_name(name) {
        return Err(Error::Invalid(
            "a node name is 1 to 64 characters, without control characters".into(),
        ));
    }
    let ctl = brain.open_control()?;
    // The name it went by before stays this node's: a peer that still knows it under that name never brings it back
    // as another machine.
    let before = meta_get(&ctl, NAME_KEY)?;
    if let Some(old) = before.filter(|o| o != name) {
        let mut former = former_names(&ctl)?;
        if !former.contains(&old) {
            former.push(old.clone());
            ctl.execute(
                "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                params![FORMER_KEY, serde_json::to_string(&former).unwrap_or_default()],
            )?;
        }
        ctl.execute("DELETE FROM peer_directory WHERE name = ?1", [&old])?;
    }
    ctl.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![NAME_KEY, name],
    )?;
    Ok(())
}

const FORMER_KEY: &str = "node_former_names";

/// The names this node went by before (see [`set_own_name`]).
fn former_names(ctl: &Connection) -> Result<Vec<String>> {
    Ok(meta_get(ctl, FORMER_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

/// This node, under its name or one it had.
fn is_self(ctl: &Connection, name: &str) -> Result<bool> {
    Ok(own_name_in(ctl)? == name || former_names(ctl)?.iter().any(|n| n == name))
}

fn read_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        name: r.get(0)?,
        last_seen: r.get(1)?,
        version: r.get(2)?,
        first_seen: r.get(3)?,
        learned_from: r.get(4)?,
        stamp: r.get(5)?,
        link: r.get(6)?,
        revoked: r.get::<_, i64>(7)? != 0,
        revoked_at: r.get(8)?,
        exposures: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
        node_id: r.get(10)?,
    })
}

const COLUMNS: &str = "name, last_seen, version, first_seen, learned_from, stamp, link, revoked, revoked_at, exposures, node_id";

fn entry_of(ctl: &Connection, name: &str) -> Result<Option<Entry>> {
    Ok(ctl
        .query_row(
            &format!("SELECT {COLUMNS} FROM peer_directory WHERE name = ?1"),
            [name],
            read_entry,
        )
        .optional()?)
}

/// Every entry, revoked ones too (they must keep spreading).
pub fn entries(brain: &Brain) -> Result<Vec<Entry>> {
    let ctl = brain.open_control()?;
    let mut st = ctl.prepare(&format!(
        "SELECT {COLUMNS} FROM peer_directory ORDER BY name"
    ))?;
    Ok(st
        .query_map([], read_entry)?
        .collect::<rusqlite::Result<_>>()?)
}

/// Writes this node's own entry: it is active now, at this version. A burst of requests writes once.
pub fn touch_self(brain: &Brain, now: i64) -> Result<()> {
    let ctl = brain.open_control()?;
    let name = own_name_in(&ctl)?;
    let version = own_version();
    let stamp = format!("{now:015}.{name}");
    let exposures = own_exposures(&ctl)?;
    let id = node_id_in(&ctl)?;
    match entry_of(&ctl, &name)? {
        Some(e)
            if e.version == version
                && e.node_id == id
                && serde_json::to_string(&e.exposures).is_ok_and(|t| t == exposures)
                && (0..TOUCH_EVERY).contains(&(now - e.last_seen)) => {}
        Some(_) => {
            ctl.execute(
                "UPDATE peer_directory SET last_seen = ?2, version = ?3, stamp = ?4, learned_from = NULL,
                   exposures = ?5, node_id = ?6
                 WHERE name = ?1",
                params![name, now, version, stamp, exposures, id],
            )?;
        }
        None => {
            ctl.execute(
                "INSERT INTO peer_directory (name, last_seen, version, first_seen, learned_from, stamp, exposures, node_id)
                 VALUES (?1, ?2, ?3, ?2, NULL, ?4, ?5, ?6)",
                params![name, now, version, stamp, exposures, id],
            )?;
        }
    }
    Ok(())
}

/// The directory as this node tells it to a peer.
pub fn to_wire(brain: &Brain) -> Result<Vec<Value>> {
    Ok(entries(brain)?.iter().map(Entry::to_wire).collect())
}

/// Takes the entries `from` told into the directory. Per name the newest `last_seen` wins; the first sight is the
/// earliest known; a revoked flag, once set, stays. This node's own entry is never taken from another. Returns how
/// many rows changed.
pub fn merge(brain: &Brain, from: &str, entries: &[Value], now: i64) -> Result<usize> {
    let ctl = brain.open_control()?;
    let own = own_name_in(&ctl)?;
    let mut changed = 0;
    for v in entries.iter().take(MAX_ENTRIES) {
        let Some(name) = v["name"].as_str().filter(|n| valid_name(n)) else {
            continue;
        };
        let told_id = valid_id(v["id"].as_str().unwrap_or(""));
        if name == own
            || is_self(&ctl, name)?
            || told_id.as_deref() == Some(node_id_in(&ctl)?.as_str())
        {
            continue;
        }
        let told_id = told_id.unwrap_or_default();
        let version: String = v["version"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        let told_seen = v["last_seen"].as_i64().unwrap_or(0).clamp(0, now + SKEW);
        let told_first = v["first_seen"]
            .as_i64()
            .filter(|f| *f > 0)
            .unwrap_or(told_seen)
            .min(told_seen.max(1));
        let told_exposures = Value::Array(clean_exposures(&v["exposures"])).to_string();
        let told_revoked = v["revoked"].as_bool() == Some(true);
        let told_revoked_at = v["revoked_at"].as_i64().unwrap_or(now);
        match entry_of(&ctl, name)? {
            None => {
                ctl.execute(
                    "INSERT INTO peer_directory
                       (name, last_seen, version, first_seen, learned_from, stamp, revoked, revoked_at, exposures, node_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        name,
                        told_seen,
                        version,
                        told_first,
                        from,
                        format!("{told_seen:015}.{from}"),
                        i64::from(told_revoked),
                        told_revoked.then_some(told_revoked_at),
                        told_exposures,
                        told_id,
                    ],
                )?;
                changed += 1;
            }
            Some(e) => {
                let mut did = false;
                if told_seen > e.last_seen {
                    ctl.execute(
                        "UPDATE peer_directory SET last_seen = ?2, version = ?3, learned_from = ?4, stamp = ?5,
                           exposures = ?6
                         WHERE name = ?1",
                        params![
                            name,
                            told_seen,
                            version,
                            from,
                            format!("{told_seen:015}.{from}"),
                            told_exposures
                        ],
                    )?;
                    did = true;
                }
                // The identity of a node never changes: it is taken once, from whoever tells it first.
                if e.node_id.is_empty() && !told_id.is_empty() {
                    ctl.execute(
                        "UPDATE peer_directory SET node_id = ?2 WHERE name = ?1",
                        params![name, told_id],
                    )?;
                    did = true;
                }
                if told_first < e.first_seen {
                    ctl.execute(
                        "UPDATE peer_directory SET first_seen = ?2 WHERE name = ?1",
                        params![name, told_first],
                    )?;
                    did = true;
                }
                if told_revoked && !e.revoked {
                    ctl.execute(
                        "UPDATE peer_directory SET revoked = 1, revoked_at = ?2 WHERE name = ?1",
                        params![name, told_revoked_at],
                    )?;
                    did = true;
                }
                changed += usize::from(did);
            }
        }
    }
    Ok(changed)
}

/// Notes that the node `name` is the direct peer reached by `connection` (a connection on this node, or the name
/// this node keeps the peer under). Does nothing for an unknown name or this node itself.
pub fn set_link(brain: &Brain, name: &str, connection: &str) -> Result<()> {
    let ctl = brain.open_control()?;
    ctl.execute(
        "UPDATE peer_directory SET link = ?2 WHERE name = ?1 AND name <> ?3",
        params![name, connection, own_name_in(&ctl)?],
    )?;
    Ok(())
}

/// The person revokes the node `name`: its entry carries the flag from now on (made if there was none, so that
/// the revocation reaches nodes that never saw the peer), and it spreads. Never applies to this node itself.
pub fn revoke_node(brain: &Brain, name: &str, now: i64) -> Result<bool> {
    let ctl = brain.open_control()?;
    if !valid_name(name) || own_name_in(&ctl)? == name {
        return Ok(false);
    }
    ctl.execute(
        "INSERT INTO peer_directory (name, last_seen, version, first_seen, stamp, revoked, revoked_at)
         VALUES (?1, 0, '', ?2, ?3, 1, ?2)
         ON CONFLICT(name) DO UPDATE SET revoked = 1, revoked_at = coalesce(revoked_at, ?2)",
        params![name, now, format!("{:015}.revoked", 0)],
    )?;
    Ok(true)
}

/// A connection made for a peer is revoked: the directory entry reached through it (by link, or named as the
/// machine of that connection) is revoked with it.
pub(crate) fn revoked_connection(ctl: &Connection, agent: &str, now: i64) -> Result<()> {
    let own = own_name_in(ctl)?;
    ctl.execute(
        "UPDATE peer_directory SET revoked = 1, revoked_at = coalesce(revoked_at, ?3)
         WHERE (link = ?1 OR name = ?2 OR name = ?1) AND revoked = 0 AND name <> ?4",
        params![agent, crate::sync::machine_name(agent), now, own],
    )?;
    Ok(())
}

/// The numbers of a version `AAAAMMJJ.0.N`.
fn numbers(version: &str) -> Option<Vec<u64>> {
    let v: Option<Vec<u64>> = version.split('.').map(|p| p.parse().ok()).collect();
    v.filter(|v| !v.is_empty())
}

/// Whether `version` is older than this program's (a version that cannot be read is not).
pub fn behind(version: &str) -> bool {
    behind_of(version, own_version())
}

/// Whether `version` is older than `mine`.
pub fn behind_of(version: &str, mine: &str) -> bool {
    match (numbers(version), numbers(mine)) {
        (Some(theirs), Some(mine)) => theirs < mine,
        _ => false,
    }
}

/// A direct peer as this node sees it: a connection it holds or a peer it syncs with.
#[derive(Clone, Debug, Default)]
pub struct Local {
    /// The connection or sync name, `peer-…`.
    pub connection: String,
    /// The machine's name as the person gave it.
    pub name: String,
    /// `sending`, `receiving` or `idle`.
    pub direction: String,
    /// Progress of the latest transfer, per space.
    pub spaces: Vec<crate::sync::SpaceProgress>,
    /// When the latest transfer happened (unix seconds).
    pub last_activity: Option<i64>,
    /// Whether the connection is still allowed (not revoked or expired).
    pub allowed: Option<bool>,
    /// When the connection was made.
    pub created_at: Option<String>,
    /// When the connection was last used.
    pub last_used_at: Option<String>,
}

/// The union: the direct peers (with direction and progress) and every node of the directory, each row once.
/// Revoked nodes are left out. This node is not listed: it is the one asking.
pub fn union(brain: &Brain, now: i64, locals: Vec<Local>) -> Result<Vec<Value>> {
    let own = own_name(brain)?;
    let ctl = brain.open_control()?;
    let all: Vec<Entry> = entries(brain)?
        .into_iter()
        .filter(|e| !is_self(&ctl, &e.name).unwrap_or(false))
        .collect();
    let mut used = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for l in locals {
        let entry = all
            .iter()
            .find(|e| e.link.as_deref() == Some(l.connection.as_str()) && e.name != own);
        if let Some(e) = entry {
            used.insert(e.name.clone());
        }
        // A peer revoked in the directory is not listed as a machine any more.
        if entry.is_some_and(|e| e.revoked) {
            continue;
        }
        let done: i64 = l.spaces.iter().map(|s| s.done).sum();
        let total: i64 = l.spaces.iter().map(|s| s.total).sum();
        let last_seen = entry
            .map(|e| e.last_seen)
            .filter(|s| *s > 0)
            .or(l.last_activity);
        rows.push(json!({
            "name": entry.map_or(l.name.as_str(), |e| e.name.as_str()),
            "peer": l.connection,
            "connection": l.connection,
            "direct": true,
            "version": entry.map(|e| e.version.clone()).filter(|v| !v.is_empty()),
            "last_seen": last_seen,
            "first_seen": entry.map(|e| e.first_seen),
            "learned_from": entry.and_then(|e| e.learned_from.clone()),
            "gone": last_seen.is_some_and(|s| now - s > GONE_AFTER),
            "behind": entry.is_some_and(|e| behind(&e.version)),
            "exposures": entry.map_or_else(Vec::new, |e| e.exposures.clone()),
            "id": entry.map(|e| e.node_id.clone()).filter(|i| !i.is_empty()),
            "addresses": entry.map_or_else(Vec::new, |e| addresses_of(&e.exposures)),
            "via": entry.and_then(|e| via(&ctl, &e.name)),
            "only_local": entry.is_some_and(|e| only_local(&e.exposures)),
            "note": entry.filter(|e| only_local(&e.exposures)).map(|_| SYNCS_ON_OPEN),
            "direction": l.direction,
            "done": done,
            "total": total,
            "percent": if total > 0 { done * 100 / total } else { 100 },
            "spaces": l.spaces.iter().map(|s| json!({
                "space": s.space, "done": s.done, "total": s.total, "percent": s.percent,
            })).collect::<Vec<_>>(),
            "last_activity": l.last_activity,
            // What the latest pass with this peer could not do, when this node runs the sync with it.
            "last_pass": crate::sync::last_pass(&ctl, &l.connection),
            "allowed": l.allowed,
            "created_at": l.created_at,
            "last_used_at": l.last_used_at,
        }));
    }
    let mut rest: Vec<&Entry> = all
        .iter()
        .filter(|e| e.name != own && !e.revoked && !used.contains(&e.name) && e.last_seen > 0)
        .collect();
    rest.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.name.cmp(&b.name)));
    rows.extend(rest.into_iter().map(|e| {
        json!({
            "name": e.name,
            "peer": e.name,
            "connection": e.link,
            "direct": false,
            "version": (!e.version.is_empty()).then(|| e.version.clone()),
            "last_seen": e.last_seen,
            "first_seen": e.first_seen,
            "learned_from": e.learned_from,
            "gone": now - e.last_seen > GONE_AFTER,
            "behind": behind(&e.version),
            "exposures": e.exposures,
            "id": (!e.node_id.is_empty()).then(|| e.node_id.clone()),
            "addresses": addresses_of(&e.exposures),
            "via": via(&ctl, &e.name),
            "only_local": only_local(&e.exposures),
            "note": only_local(&e.exposures).then_some(SYNCS_ON_OPEN),
            "direction": Value::Null,
            "last_activity": Value::Null,
        })
    }));
    Ok(rows)
}

/// The note on a peer that can only be reached from its own machine or network: it syncs when it is opened,
/// with the peers it can reach. A key for the apps to say it in the person's words.
pub const SYNCS_ON_OPEN: &str = "syncs_on_open";

/// This node, as the answers about peers carry it.
pub fn node_json(brain: &Brain, now: i64) -> Result<Value> {
    let ctl = brain.open_control()?;
    let name = own_name_in(&ctl)?;
    let exposures: Vec<Value> = serde_json::from_str(&own_exposures(&ctl)?).unwrap_or_default();
    Ok(json!({
        "name": name,
        "id": node_id_in(&ctl)?,
        "version": own_version(),
        "last_seen": now,
        "exposures": exposures,
        "only_local": only_local(&exposures),
        "note": only_local(&exposures).then_some(SYNCS_ON_OPEN),
    }))
}

/// The assistant connections of `user` (the ones that are not peers), and how each reaches this node: `cloud` when
/// its requests last came from an address on the internet (a service such as claude.ai), `local` when they came
/// from this machine, the local network or the tailnet (a program of the person's), or never. `via` is the node
/// that served those requests: this one.
pub fn connections(brain: &Brain, user: &str) -> Result<Vec<Value>> {
    let ctl = brain.open_control()?;
    let via = own_name_in(&ctl)?;
    let mut st = ctl.prepare(
        "SELECT agent, last_used_at, last_ip FROM connections
         WHERE console = 0 AND revoked_at IS NULL AND internal = 0
           AND coalesce(user_id, 'owner') = ?1 AND kind != 'peer'
         ORDER BY agent",
    )?;
    let rows = st.query_map([user], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (name, used, ip) = row?;
        // Through Tailscale Funnel (`funnel <proxy address>`, app::client_seen): the internet, whatever the address.
        let funnel = ip.as_deref().is_some_and(|i| i.starts_with("funnel"));
        let cloud = funnel
            || ip
                .as_deref()
                .and_then(|i| {
                    i.trim_start_matches('[')
                        .split(']')
                        .next()
                        .map(str::to_string)
                })
                .and_then(|i| {
                    i.parse::<std::net::IpAddr>()
                        .ok()
                        .or_else(|| i.rsplit_once(':').and_then(|(h, _)| h.parse().ok()))
                })
                .is_some_and(|ip| !is_private(&ip));
        out.push(json!({
            "name": name,
            "kind": if cloud { "cloud" } else { "local" },
            "via": via,
            "last_used_at": used.map(util::iso),
        }));
    }
    Ok(out)
}

// --- the identity of a node ---------------------------------------------------------------------------------------

const ID_KEY: &str = "node_id";
const ID_LEN: usize = 12;

/// An id as it may be typed or scanned: dashes and spaces dropped, capitals. `None` when it is not one (12 characters
/// of the base32 alphabet, `A-Z` and `2-7`), else the way it is shown, `ABCD-EFGH-IJKL`.
pub fn valid_id(text: &str) -> Option<String> {
    let c: String = text
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (c.len() == ID_LEN && c.bytes().all(|b| matches!(b, b'A'..=b'Z' | b'2'..=b'7')))
        .then(|| format!("{}-{}-{}", &c[..4], &c[4..8], &c[8..]))
}

/// This node's identity: made once, random (60 bits), kept in the control database and told to every peer. It names
/// the node, it proves nothing: whoever knows it can say it. What lets a node in is always a person's answer or a
/// ticket, never the id.
pub fn node_id(brain: &Brain) -> Result<String> {
    node_id_in(&brain.open_control()?)
}

fn node_id_in(ctl: &Connection) -> Result<String> {
    if let Some(i) = meta_get(ctl, ID_KEY)? {
        return Ok(i);
    }
    let id =
        valid_id(&util::random_base32(ID_LEN)).ok_or_else(|| Error::Schema("node id".into()))?;
    ctl.execute(
        "INSERT OR IGNORE INTO meta (k, v) VALUES (?1, ?2)",
        params![ID_KEY, id],
    )?;
    meta_get(ctl, ID_KEY)?.ok_or_else(|| Error::Schema("node id not written".into()))
}

/// The entry of the node with this id, when this node knows it (its own entry too).
pub fn find_by_id(brain: &Brain, id: &str) -> Result<Option<Entry>> {
    let Some(id) = valid_id(id) else {
        return Ok(None);
    };
    let ctl = brain.open_control()?;
    Ok(ctl
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM peer_directory WHERE node_id = ?1 ORDER BY last_seen DESC"
            ),
            [id],
            read_entry,
        )
        .optional()?)
}

// --- addresses, links, the choice of a way in ---------------------------------------------------------------------

/// An address as a base address: a bare `host:port` (a listener) is read as plain HTTP, the way a listener serves.
fn base_url(address: &str) -> String {
    let a = address.trim().trim_end_matches('/');
    if a.contains("://") {
        a.to_string()
    } else {
        format!("http://{a}")
    }
}

/// The addresses a node can be asked at, best first: the internet, the tailnet, the local network, this machine.
pub fn addresses_of(exposures: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for kind in ["public", "tailnet", "lan", "loopback"] {
        for e in exposures.iter().filter(|e| e["kind"] == kind) {
            if let Some(a) = e["address"].as_str() {
                let u = base_url(a);
                if !out.contains(&u) {
                    out.push(u);
                }
            }
        }
    }
    out
}

const VIA_PREFIX: &str = "node_via:";

/// The address the person chose for asking the node `name`, if they chose one.
pub fn via(ctl: &Connection, name: &str) -> Option<String> {
    meta_get(ctl, &format!("{VIA_PREFIX}{name}")).ok().flatten()
}

/// The person chooses by which address this node asks the node `name` (one of the addresses it told), or clears the
/// choice (`None`).
pub fn set_via(brain: &Brain, name: &str, address: Option<&str>) -> Result<()> {
    let ctl = brain.open_control()?;
    let key = format!("{VIA_PREFIX}{name}");
    let Some(address) = address else {
        ctl.execute("DELETE FROM meta WHERE k = ?1", [&key])?;
        return Ok(());
    };
    let entry = entry_of(&ctl, name)?
        .ok_or_else(|| Error::Invalid(format!("no node called {name} is known")))?;
    let wanted = base_url(address);
    if !addresses_of(&entry.exposures).contains(&wanted) {
        return Err(Error::Invalid(
            "that address is not one this node told: choose one of its addresses".into(),
        ));
    }
    ctl.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![key, wanted],
    )?;
    Ok(())
}

/// Where a node can be asked, in the order to try: the chosen address first.
pub fn ways_in(brain: &Brain, entry: &Entry) -> Result<Vec<String>> {
    let ctl = brain.open_control()?;
    let mut out: Vec<String> = via(&ctl, &entry.name).into_iter().collect();
    for a in addresses_of(&entry.exposures) {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    Ok(out)
}

/// What a person shows or scans to add a node: `memcp://node?id=…&name=…&addr=…&addr=…[&t=ticket]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeLink {
    /// The node's id (`ABCD-EFGH-IJKL`), when the link says it.
    pub id: Option<String>,
    /// The node's name.
    pub name: Option<String>,
    /// The addresses to try, in order.
    pub addresses: Vec<String>,
    /// A one-time ticket from a node the person already holds: with it, nobody has to compare three words.
    pub ticket: Option<String>,
}

fn pct_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn pct_decode(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |x: u8| (x as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl NodeLink {
    /// The link, as it is shown and put in a QR code.
    pub fn to_link(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(i) = &self.id {
            parts.push(format!("id={}", pct_encode(i)));
        }
        if let Some(n) = &self.name {
            parts.push(format!("name={}", pct_encode(n)));
        }
        for a in &self.addresses {
            parts.push(format!("addr={}", pct_encode(a)));
        }
        if let Some(t) = &self.ticket {
            parts.push(format!("t={}", pct_encode(t)));
        }
        format!("memcp://node?{}", parts.join("&"))
    }

    /// Reads what a person pasted or scanned: a node link, a bare id, an address, or the older join link
    /// (`memcp://join?server=…`, whose server is the address).
    pub fn parse(text: &str) -> Result<NodeLink> {
        let t = text.trim();
        let bad = || Error::Invalid("not a node link, an id (ABCD-EFGH-IJKL) or an address".into());
        if t.is_empty() || t.len() > 2000 || t.chars().any(char::is_control) {
            return Err(bad());
        }
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("memcp://node?") || lower.starts_with("memcp://join?") {
            let q = &t[t.find('?').map_or(0, |i| i + 1)..];
            let mut link = NodeLink::default();
            for pair in q.split('&') {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                let v = pct_decode(v);
                match k {
                    "id" => link.id = Some(valid_id(&v).ok_or_else(bad)?),
                    "name" if !v.is_empty() => link.name = Some(v.chars().take(64).collect()),
                    "addr" | "server" if link.addresses.len() < 8 => {
                        if !(v.starts_with("https://") || v.starts_with("http://"))
                            || v.contains(char::is_whitespace)
                        {
                            return Err(bad());
                        }
                        link.addresses.push(v.trim_end_matches('/').to_string());
                    }
                    "t" if !v.is_empty() => link.ticket = Some(v),
                    _ => {}
                }
            }
            if link.id.is_none() && link.addresses.is_empty() {
                return Err(bad());
            }
            return Ok(link);
        }
        if let Some(id) = valid_id(t) {
            return Ok(NodeLink {
                id: Some(id),
                ..NodeLink::default()
            });
        }
        if (lower.starts_with("https://") || lower.starts_with("http://"))
            && !t.contains(char::is_whitespace)
        {
            return Ok(NodeLink {
                addresses: vec![t.trim_end_matches('/').to_string()],
                ..NodeLink::default()
            });
        }
        Err(bad())
    }
}

/// This node's link: its id, its name and its addresses (`public_url` first, then what it exposes), with a ticket
/// when the person made one.
pub fn own_link(brain: &Brain, public_url: &str, ticket: Option<&str>) -> Result<NodeLink> {
    let ctl = brain.open_control()?;
    let exposures: Vec<Value> = serde_json::from_str(&own_exposures(&ctl)?).unwrap_or_default();
    let mut addresses: Vec<String> = Vec::new();
    for a in std::iter::once(base_url(public_url)).chain(addresses_of(&exposures)) {
        if !addresses.contains(&a) {
            addresses.push(a);
        }
    }
    Ok(NodeLink {
        id: Some(node_id_in(&ctl)?),
        name: Some(own_name_in(&ctl)?),
        addresses,
        ticket: ticket.map(str::to_string),
    })
}

// --- requests to join, told by peers -------------------------------------------------------------------------------
//
// A request to join waits on the node that was asked. That node tells its peers (in the exchange `GET /sync/peers`), so
// that the person can answer from any of their nodes; an answer given elsewhere goes back with the next exchange
// (`POST /sync/peers`, `answers`). A request that is gone from the asked node's list is gone here at the next exchange.

/// Keeps the requests `node` told (`[{id, device, check, expires_in}]`): the ones it no longer lists are dropped, an
/// answer given here and not yet delivered stays.
pub fn store_remote_joins(brain: &Brain, node: &str, list: &[Value], now: i64) -> Result<()> {
    if !valid_name(node) {
        return Ok(());
    }
    let ctl = brain.open_control()?;
    let clip =
        |t: &str, n: usize| -> String { t.chars().filter(|c| !c.is_control()).take(n).collect() };
    let mut kept: Vec<String> = Vec::new();
    for v in list.iter().take(16) {
        let (Some(id), Some(device), Some(words)) =
            (v["id"].as_str(), v["device"].as_str(), v["check"].as_str())
        else {
            continue;
        };
        let expires = now + v["expires_in"].as_i64().unwrap_or(0).clamp(0, 3600);
        ctl.execute(
            "INSERT INTO remote_joins (node, id, device, words, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(node, id) DO UPDATE SET expires_at = excluded.expires_at",
            params![node, clip(id, 40), clip(device, 60), clip(words, 60), expires],
        )?;
        kept.push(clip(id, 40));
    }
    let mut st = ctl.prepare("SELECT id FROM remote_joins WHERE node = ?1 AND answer IS NULL")?;
    let held: Vec<String> = st
        .query_map([node], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in held.into_iter().filter(|i| !kept.contains(i)) {
        ctl.execute(
            "DELETE FROM remote_joins WHERE node = ?1 AND id = ?2",
            params![node, id],
        )?;
    }
    ctl.execute(
        "DELETE FROM remote_joins WHERE expires_at < ?1 - 3600",
        [now],
    )?;
    Ok(())
}

/// The requests of other nodes waiting for this person's answer: `{id, device, check, node, expires_in, remote}`.
pub fn remote_joins(brain: &Brain, now: i64) -> Result<Vec<Value>> {
    let ctl = brain.open_control()?;
    let mut st = ctl.prepare(
        "SELECT node, id, device, words, expires_at FROM remote_joins
         WHERE answer IS NULL AND expires_at > ?1 ORDER BY expires_at, id",
    )?;
    let rows = st.query_map([now], |r| {
        Ok(json!({
            "node": r.get::<_, String>(0)?, "id": r.get::<_, String>(1)?,
            "device": r.get::<_, String>(2)?, "check": r.get::<_, String>(3)?,
            "expires_in": r.get::<_, i64>(4)? - now, "remote": true,
        }))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The person answers a request told by another node (`result`: `approved` or `denied`): it leaves the list here at
/// once and the answer is kept for that node. Returns the node's name, or `None` when no such request is held.
pub fn answer_remote_join(
    brain: &Brain,
    id: &str,
    result: &str,
    now: i64,
) -> Result<Option<String>> {
    let ctl = brain.open_control()?;
    let node: Option<String> = ctl
        .query_row(
            "SELECT node FROM remote_joins WHERE id = ?1 AND answer IS NULL AND expires_at > ?2",
            params![id, now],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(n) = &node {
        ctl.execute(
            "UPDATE remote_joins SET answer = ?3 WHERE node = ?1 AND id = ?2",
            params![n, id, result],
        )?;
    }
    Ok(node)
}

/// The answers waiting to be told to `node`: `[{id, result}]`.
pub fn answers_for(brain: &Brain, node: &str) -> Result<Vec<Value>> {
    let ctl = brain.open_control()?;
    let mut st =
        ctl.prepare("SELECT id, answer FROM remote_joins WHERE node = ?1 AND answer IS NOT NULL")?;
    let rows = st.query_map([node], |r| {
        Ok(json!({ "id": r.get::<_, String>(0)?, "result": r.get::<_, String>(1)? }))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The answers to `node` were told: they are forgotten.
pub fn answers_delivered(brain: &Brain, node: &str) -> Result<()> {
    brain.open_control()?.execute(
        "DELETE FROM remote_joins WHERE node = ?1 AND answer IS NOT NULL",
        [node],
    )?;
    Ok(())
}
