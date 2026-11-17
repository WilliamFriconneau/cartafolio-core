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

/// This node's name in the directory: the one the person gave it, else one made once and kept.
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
        params![
            NAME_KEY,
            format!("node-{}", util::random_base32(8).to_lowercase())
        ],
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
    ctl.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![NAME_KEY, name],
    )?;
    Ok(())
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
    })
}

const COLUMNS: &str = "name, last_seen, version, first_seen, learned_from, stamp, link, revoked, revoked_at, exposures";

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
    match entry_of(&ctl, &name)? {
        Some(e)
            if e.version == version
                && serde_json::to_string(&e.exposures).is_ok_and(|t| t == exposures)
                && (0..TOUCH_EVERY).contains(&(now - e.last_seen)) => {}
        Some(_) => {
            ctl.execute(
                "UPDATE peer_directory SET last_seen = ?2, version = ?3, stamp = ?4, learned_from = NULL,
                   exposures = ?5
                 WHERE name = ?1",
                params![name, now, version, stamp, exposures],
            )?;
        }
        None => {
            ctl.execute(
                "INSERT INTO peer_directory (name, last_seen, version, first_seen, learned_from, stamp, exposures)
                 VALUES (?1, ?2, ?3, ?2, NULL, ?4, ?5)",
                params![name, now, version, stamp, exposures],
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
        if name == own {
            continue;
        }
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
                       (name, last_seen, version, first_seen, learned_from, stamp, revoked, revoked_at, exposures)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
    let all = entries(brain)?;
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
           AND coalesce(user_id, 'owner') = ?1 AND agent NOT LIKE 'peer-%'
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
