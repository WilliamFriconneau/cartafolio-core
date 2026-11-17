//! Pairing a bridge: the person asks their page for a code, types it into the app on a device, and the
//! app exchanges it once for a connection of its own, limited to the person's « À trier » (see [`crate::triage`]).
//!
//! The code is short so that it can be typed (eight characters from an alphabet without look-alikes,
//! about 40 bits). That is enough because it lives ten minutes, works once, and the route that takes it
//! is rate limited by address. Only its hash is kept.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::access::{self, Rights};
use crate::{Error, Result};

/// How long a code can be used.
pub const CODE_TTL: i64 = 10 * 60;
/// No 0/O, 1/I/L: a code is read on one screen and typed on another.
const ALPHABET: &[u8; 31] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;

/// What a device gets for a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub agent: String,
    pub token: String,
    /// The spaces whose inbox the connection writes to.
    pub spaces: Vec<String>,
}

/// A code as the person may type it: lower case, with a dash or spaces.
pub fn normalize(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn hash(code: &str) -> Vec<u8> {
    Sha256::digest(format!("pairing:{}", normalize(code)).as_bytes()).to_vec()
}

/// `ABCD-EFGH`, the way it is shown.
pub fn display(code: &str) -> String {
    let c = normalize(code);
    if c.len() == CODE_LEN {
        format!("{}-{}", &c[..4], &c[4..])
    } else {
        c
    }
}

/// The rights a bridge gets for the spaces ticked: the inbox of each, nothing else.
pub fn rights_for(spaces: &[String]) -> Result<Rights> {
    if spaces.is_empty() {
        return Err(Error::Invalid("tick at least one space".into()));
    }
    let spec: Vec<String> = spaces.iter().map(|v| format!("{v}/inbox:rw")).collect();
    Rights::parse(&spec.join(","))
}

/// A new code for `user_id`, valid [`CODE_TTL`] seconds from `now`, for the inboxes of `spaces`.
pub fn create(ctl: &Connection, user_id: &str, spaces: &[String], now: i64) -> Result<String> {
    create_with(ctl, user_id, rights_for(spaces)?, now)
}

/// A new code for a node of the person's own (a peer): it reads and writes every space the person owns, as the
/// server does. Nothing is shut in a box: a peer holds the same memory.
pub fn create_for_triage(
    brain: &crate::Brain,
    user_id: &str,
    person: &str,
    now: i64,
) -> Result<(String, String)> {
    let triage = crate::triage::ensure(brain, user_id, person)?;
    let ctl = brain.open_control()?;
    let spaces: Vec<String> = brain
        .layout(user_id)?
        .iter()
        .map(|p| p.space.as_str().to_string())
        .collect();
    let spec: Vec<String> = spaces.iter().map(|v| format!("{v}:rw")).collect();
    let code = create_with(&ctl, user_id, Rights::parse(&spec.join(","))?, now)?;
    Ok((code, triage.as_str().to_string()))
}

fn create_with(ctl: &Connection, user_id: &str, rights: Rights, now: i64) -> Result<String> {
    access::check_within(ctl, user_id, &rights)?;
    // Old codes of anyone, used or expired, go: they are worth nothing.
    ctl.execute(
        "DELETE FROM bridge_pairings WHERE expires_at < ?1 - 86400",
        [now],
    )?;
    // Each character equally likely: a byte of 248 or more (not a multiple of 31) is drawn again.
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        for b in crate::util::random_bytes::<16>() {
            if b < 248 && code.len() < CODE_LEN {
                code.push(ALPHABET[usize::from(b) % ALPHABET.len()] as char);
            }
        }
    }
    ctl.execute(
        "INSERT INTO bridge_pairings (hash, user_id, rights, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![hash(&code), user_id, rights.label(), now, now + CODE_TTL],
    )?;
    access::auth_event(ctl, None, None, "pairing_code", "");
    Ok(display(&code))
}

/// Whether this control database holds `code`, unused and not expired (the manager finds the person this way).
/// Where a join code stands for the person who made it: waiting, joined (by which connection) or expired.
#[derive(Debug, PartialEq, Eq)]
pub enum Standing {
    Waiting,
    Joined(String),
    Expired,
    Unknown,
}

pub fn standing(ctl: &Connection, user_id: &str, code: &str, now: i64) -> Standing {
    let row: Option<(Option<i64>, i64, Option<String>)> = ctl
        .query_row(
            "SELECT used_at, expires_at, agent FROM bridge_pairings WHERE hash = ?1 AND user_id = ?2",
            params![hash(code), user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    match row {
        None => Standing::Unknown,
        Some((Some(_), _, agent)) => Standing::Joined(agent.unwrap_or_default()),
        Some((None, expires, _)) if expires <= now => Standing::Expired,
        Some(_) => Standing::Waiting,
    }
}

pub fn holds(ctl: &Connection, code: &str, now: i64) -> bool {
    ctl.query_row(
        "SELECT 1 FROM bridge_pairings WHERE hash = ?1 AND used_at IS NULL AND expires_at > ?2",
        params![hash(code), now],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

/// A connection's name from what the device calls itself: `peer-<name>-<4 random>`.
fn agent_name(device: &str) -> String {
    let mut slug = String::new();
    for c in device.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 20 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    let tail = crate::util::random_base32(4);
    if slug.is_empty() {
        format!("peer-{tail}")
    } else {
        format!("peer-{slug}-{tail}")
    }
}

/// Exchanges `code` for a new connection, once. Anything wrong (unknown, used, expired) is the same
/// answer, so that a guess learns nothing.
pub fn redeem(ctl: &Connection, code: &str, device: &str, now: i64) -> Result<Paired> {
    let refused = || Error::Invalid("this code is unknown, used or expired".into());
    let row: Option<(String, String)> = ctl
        .query_row(
            "SELECT user_id, rights FROM bridge_pairings WHERE hash = ?1 AND used_at IS NULL AND expires_at > ?2",
            params![hash(code), now],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((user_id, spec)) = row else {
        return Err(refused());
    };
    // Claimed first, in one statement: two devices racing with the same code, one wins.
    let agent = agent_name(device);
    let claimed = ctl.execute(
        "UPDATE bridge_pairings SET used_at = ?2, agent = ?3 WHERE hash = ?1 AND used_at IS NULL AND expires_at > ?2",
        params![hash(code), now, agent],
    )?;
    if claimed != 1 {
        return Err(refused());
    }
    let made = Rights::parse(&spec)
        .and_then(|rights| access::connect_for(ctl, &user_id, &agent, &rights, None, false, false));
    let token = match made {
        Ok(t) => t,
        Err(e) => {
            // The person's spaces changed since the code was made: the code can be tried again.
            ctl.execute(
                "UPDATE bridge_pairings SET used_at = NULL, agent = NULL WHERE hash = ?1",
                [hash(code)],
            )?;
            return Err(e);
        }
    };
    let mut spaces: Vec<String> = spec
        .split(',')
        .filter_map(|g| g.split('/').next().map(str::to_string))
        .collect();
    spaces.dedup();
    Ok(Paired {
        agent,
        token,
        spaces,
    })
}
