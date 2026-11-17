//! People and the vaults they own.
//!
//! A person reaches the vaults they own (`space_members`), and nothing of anyone else's: there are
//! no shares and no groups. Every connection belongs to a person and never goes beyond that
//! person's access. Administrators manage people and see who has what, they do not read anyone's
//! content through this module.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::access::Grant;
use crate::error::{Error, Result};
use crate::space::Space;
use crate::text::hex;
use crate::util;

/// The bootstrap administrator, created by the control migration.
pub const OWNER: &str = "owner";

/// A person who owns vaults and connections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    /// Opaque identifier (`u_...`), stable; the bootstrap administrator is [`OWNER`].
    pub id: String,
    /// Short login name: 1 to 40 lowercase letters, digits or dashes, starting with a letter.
    pub name: String,
    /// Display name (80 characters at most, no control characters).
    pub display: String,
    /// Whether the person is an administrator (manages people, not their content).
    pub admin: bool,
    /// The Home Assistant user this person signs in as, if linked.
    pub ha_user: Option<String>,
    /// Creation date (seconds since the epoch).
    pub created_at: i64,
    /// A disabled person's connections stop working at once; nothing is deleted.
    pub disabled: bool,
}

fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn bad_name() -> Error {
    Error::Invalid(
        "name: 1 to 40 lowercase letters, digits or dashes, starting with a letter".into(),
    )
}

fn clean_display(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .trim()
        .to_string()
}

fn user_of(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        name: r.get(1)?,
        display: r.get(2)?,
        admin: r.get::<_, String>(3)? == "admin",
        ha_user: r.get(4)?,
        created_at: r.get(5)?,
        disabled: r.get::<_, Option<i64>>(6)?.is_some(),
    })
}

const USER_COLS: &str = "id, name, display, role, ha_user, created_at, disabled_at";

pub(crate) fn audit(ctl: &Connection, actor: &str, event: &str, detail: &str) {
    crate::access::auth_event(ctl, Some(actor), None, event, detail);
}

/// Creates a person. `actor` is recorded in the control audit log.
///
/// # Errors
/// [`Error::Invalid`] for a bad `name` or a name already taken.
///
/// # Effects
/// A row in `users` and one `user_create` row in the control audit log. The person owns no vault yet.
pub fn create_user(
    ctl: &Connection,
    actor: &str,
    name: &str,
    display: &str,
    admin: bool,
) -> Result<User> {
    if !valid_name(name) {
        return Err(bad_name());
    }
    let id = format!("u_{}", hex(&util::random_bytes::<6>()));
    let n = ctl.execute(
        "INSERT OR IGNORE INTO users (id, name, display, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, clean_display(display), if admin { "admin" } else { "member" }, util::now()],
    )?;
    if n == 0 {
        return Err(Error::Invalid(format!("user {name} already exists")));
    }
    audit(ctl, actor, "user_create", name);
    user(ctl, &id)?.ok_or_else(|| Error::Schema("user not found after creation".into()))
}

/// A person by id (also a disabled one); `Ok(None)` if unknown. Read-only.
pub fn user(ctl: &Connection, id: &str) -> Result<Option<User>> {
    Ok(ctl
        .query_row(
            &format!("SELECT {USER_COLS} FROM users WHERE id = ?1"),
            [id],
            user_of,
        )
        .optional()?)
}

/// A person by login name (also a disabled one); `Ok(None)` if unknown. Read-only.
pub fn user_by_name(ctl: &Connection, name: &str) -> Result<Option<User>> {
    Ok(ctl
        .query_row(
            &format!("SELECT {USER_COLS} FROM users WHERE name = ?1"),
            [name],
            user_of,
        )
        .optional()?)
}

/// The id of an active user given by id or name.
pub fn resolve(ctl: &Connection, id_or_name: &str) -> Result<User> {
    let u = match user(ctl, id_or_name)? {
        Some(u) => Some(u),
        None => user_by_name(ctl, id_or_name)?,
    };
    match u {
        Some(u) if !u.disabled => Ok(u),
        Some(_) => Err(Error::Invalid(format!("user {id_or_name} is disabled"))),
        None => Err(Error::Invalid(format!("unknown user: {id_or_name}"))),
    }
}

/// Every person, oldest first, disabled ones included. Read-only.
pub fn list_users(ctl: &Connection) -> Result<Vec<User>> {
    let mut st = ctl.prepare(&format!(
        "SELECT {USER_COLS} FROM users ORDER BY created_at, name"
    ))?;
    Ok(st
        .query_map([], user_of)?
        .collect::<rusqlite::Result<_>>()?)
}

fn admins_left(ctl: &Connection, without: &str) -> Result<i64> {
    Ok(ctl.query_row(
        "SELECT count(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL AND id <> ?1",
        [without],
        |r| r.get(0),
    )?)
}

/// Whether taking `id` out as an administrator (their role, their being enabled, their link to a login) would leave
/// nobody able to open the administration page. An administrator who is not linked to a login cannot sign in, so
/// they do not count as a way back for one who is: the installation would be locked. Where no administrator has a
/// login at all (console keys only), every active administrator counts.
fn would_lock_out(ctl: &Connection, id: &str) -> Result<bool> {
    let Some(u) = user(ctl, id)? else {
        return Ok(false);
    };
    if !u.admin || u.disabled {
        return Ok(false);
    }
    if u.ha_user.is_some() {
        let others: i64 = ctl.query_row(
            "SELECT count(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL AND ha_user IS NOT NULL AND id <> ?1",
            [id],
            |r| r.get(0),
        )?;
        Ok(others == 0)
    } else {
        Ok(admins_left(ctl, id)? == 0)
    }
}

/// Changes the role. The last active administrator cannot lose the role.
pub fn set_admin(ctl: &Connection, actor: &str, id: &str, admin: bool) -> Result<bool> {
    if !admin && would_lock_out(ctl, id)? {
        return Err(Error::Invalid(
            "the last administrator who can sign in cannot lose the role".into(),
        ));
    }
    let n = ctl.execute(
        "UPDATE users SET role = ?2 WHERE id = ?1",
        params![id, if admin { "admin" } else { "member" }],
    )?;
    audit(
        ctl,
        actor,
        "user_role",
        &format!("{id} {}", if admin { "admin" } else { "member" }),
    );
    Ok(n == 1)
}

/// Changes the display name of the person `id`. Returns `false` if the id is unknown.
pub fn set_display(ctl: &Connection, id: &str, display: &str) -> Result<bool> {
    Ok(ctl.execute(
        "UPDATE users SET display = ?2 WHERE id = ?1",
        params![id, clean_display(display)],
    )? == 1)
}

/// Disables a person: their connections stop working at once.
/// Nothing is deleted. The last active administrator cannot be disabled.
pub fn set_disabled(ctl: &Connection, actor: &str, id: &str, disabled: bool) -> Result<bool> {
    if disabled && would_lock_out(ctl, id)? {
        return Err(Error::Invalid(
            "the last administrator who can sign in cannot be disabled".into(),
        ));
    }
    let n = if disabled {
        ctl.execute(
            "UPDATE users SET disabled_at = ?2 WHERE id = ?1 AND disabled_at IS NULL",
            params![id, util::now()],
        )?
    } else {
        ctl.execute("UPDATE users SET disabled_at = NULL WHERE id = ?1", [id])?
    };
    audit(
        ctl,
        actor,
        if disabled {
            "user_disable"
        } else {
            "user_enable"
        },
        id,
    );
    Ok(n == 1)
}

/// The person behind a Home Assistant login, created as a member on first sight
/// (or as an administrator when listed in `admins`). Returns `None` when disabled.
pub fn from_home_assistant(
    ctl: &Connection,
    ha_id: &str,
    ha_name: &str,
    display: &str,
    admins: &[String],
) -> Result<Option<User>> {
    let wanted_admin = admins.iter().any(|a| a == ha_name);
    if let Some(u) = ctl
        .query_row(
            &format!("SELECT {USER_COLS} FROM users WHERE ha_user = ?1"),
            [ha_id],
            user_of,
        )
        .optional()?
    {
        if wanted_admin && !u.admin {
            set_admin(ctl, "home-assistant", &u.id, true)?;
            return user(ctl, &u.id);
        }
        return Ok((!u.disabled).then_some(u));
    }
    // A readable name from the Home Assistant login.
    let base: String = text_slug(ha_name);
    // An existing person of that name, never linked to Home Assistant, becomes this login
    // (an installation migrated from the command line keeps its owner). Home Assistant
    // users are created by its administrators, so a name is not claimed by a stranger.
    if let Some(u) = user_by_name(ctl, &base)?
        && u.ha_user.is_none()
        && !u.disabled
    {
        ctl.execute(
            "UPDATE users SET ha_user = ?2 WHERE id = ?1",
            params![u.id, ha_id],
        )?;
        audit(ctl, "home-assistant", "user_link", &u.id);
        if wanted_admin && !u.admin {
            set_admin(ctl, "home-assistant", &u.id, true)?;
        }
        return user(ctl, &u.id);
    }
    // A login named among the administrators takes up the person of that name even when it was
    // linked to another account: Home Assistant accounts are made by its own administrators, and the
    // list is theirs. This is the way back in after an account was deleted and made again, which
    // has another identifier.
    if wanted_admin
        && let Some(u) = user_by_name(ctl, &base)?
        && !u.disabled
    {
        ctl.execute(
            "UPDATE users SET ha_user = ?2 WHERE id = ?1",
            params![u.id, ha_id],
        )?;
        audit(ctl, "home-assistant", "user_relink", &u.id);
        if !u.admin {
            set_admin(ctl, "home-assistant", &u.id, true)?;
        }
        return user(ctl, &u.id);
    }
    // No administrator named in the options and the bootstrap administrator not
    // linked yet: the first login becomes that administrator (a fresh installation).
    if admins.is_empty()
        && let Some(owner) = user(ctl, OWNER)?
        && owner.ha_user.is_none()
        && !owner.disabled
    {
        ctl.execute(
            "UPDATE users SET ha_user = ?2, name = ?3, display = ?4 WHERE id = ?1",
            params![
                OWNER,
                ha_id,
                unique_name(ctl, &base)?,
                clean_display(display)
            ],
        )?;
        audit(ctl, "home-assistant", "user_link", OWNER);
        return user(ctl, OWNER);
    }
    // Otherwise a new person, with a unique name.
    let name = unique_name(ctl, &base)?;
    let u = create_user(ctl, "home-assistant", &name, display, wanted_admin)?;
    ctl.execute(
        "UPDATE users SET ha_user = ?2 WHERE id = ?1",
        params![u.id, ha_id],
    )?;
    user(ctl, &u.id)
}

/// What a Home Assistant login leads to.
#[derive(Debug)]
pub enum HaLogin {
    /// A person: already linked, or linked now by one of the approved ways.
    Person(User),
    /// Unknown: waiting for an administrator.
    Pending,
    /// An administrator refused it.
    Refused,
    /// The person exists and is disabled.
    Disabled,
}

/// Turns the bootstrap user of a fresh data directory into a person: a name, a display name, the
/// Home Assistant login it answers to, and a plain member (an installation's administrators live
/// in the registry of whoever holds several people, never in a person's own directory).
pub fn claim_owner(
    ctl: &Connection,
    name: &str,
    display: &str,
    ha_user: Option<&str>,
) -> Result<User> {
    if !valid_name(name) {
        return Err(bad_name());
    }
    ctl.execute(
        "UPDATE users SET name = ?2, display = ?3, ha_user = ?4, role = 'member' WHERE id = ?1",
        params![OWNER, name, clean_display(display), ha_user],
    )?;
    user(ctl, OWNER)?.ok_or_else(|| Error::Schema("the bootstrap user is missing".into()))
}

/// Points an existing person at another Home Assistant login (or none).
pub fn set_ha_user(ctl: &Connection, id: &str, ha_user: Option<&str>) -> Result<bool> {
    if ha_user.is_none() && would_lock_out(ctl, id)? {
        return Err(Error::Invalid(
            "the last administrator who can sign in cannot be unlinked from their login".into(),
        ));
    }
    Ok(ctl.execute(
        "UPDATE users SET ha_user = ?2 WHERE id = ?1",
        params![id, ha_user],
    )? == 1)
}

/// A login that would become a new member of the installation: not linked, not named in
/// `admins`, no person of that name waiting for it, and not the first login of a fresh
/// installation. Those four are approved ways in; this is the case that must wait.
fn is_new_member(ctl: &Connection, ha_id: &str, ha_name: &str, admins: &[String]) -> Result<bool> {
    let linked = ctl
        .query_row(
            "SELECT 1 FROM users WHERE ha_user = ?1",
            [ha_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if linked || admins.iter().any(|a| a == ha_name) {
        return Ok(false);
    }
    if let Some(u) = user_by_name(ctl, &text_slug(ha_name))?
        && u.ha_user.is_none()
        && !u.disabled
    {
        return Ok(false);
    }
    if admins.is_empty()
        && let Some(owner) = user(ctl, OWNER)?
        && owner.ha_user.is_none()
        && !owner.disabled
    {
        return Ok(false);
    }
    Ok(true)
}

/// Like [`from_home_assistant`], except that a login which would become a new member does
/// not: it is recorded as a request and waits for an administrator.
pub fn ha_login(
    ctl: &Connection,
    ha_id: &str,
    ha_name: &str,
    display: &str,
    admins: &[String],
    auto_create: bool,
) -> Result<HaLogin> {
    if !is_new_member(ctl, ha_id, ha_name, admins)? {
        return Ok(
            match from_home_assistant(ctl, ha_id, ha_name, display, admins)? {
                Some(u) => HaLogin::Person(u),
                None => HaLogin::Disabled,
            },
        );
    }
    let now = crate::util::now();
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control()).take(60).collect() };
    let known: Option<String> = ctl
        .query_row(
            "SELECT status FROM person_requests WHERE ha_user = ?1",
            [ha_id],
            |r| r.get(0),
        )
        .optional()?;
    // Let in without asking, unless an administrator already refused this account: every Home
    // Assistant user gets a memory of their own, empty and private, and nothing else.
    if auto_create && known.as_deref() != Some("refused") {
        ctl.execute("DELETE FROM person_requests WHERE ha_user = ?1", [ha_id])?;
        return Ok(
            match from_home_assistant(ctl, ha_id, ha_name, display, admins)? {
                Some(u) => HaLogin::Person(u),
                None => HaLogin::Disabled,
            },
        );
    }
    match known {
        Some(status) => {
            ctl.execute(
                "UPDATE person_requests SET last_seen = ?2 WHERE ha_user = ?1",
                params![ha_id, now],
            )?;
            Ok(if status == "refused" {
                HaLogin::Refused
            } else {
                HaLogin::Pending
            })
        }
        None => {
            let waiting: i64 =
                ctl.query_row("SELECT count(*) FROM person_requests", [], |r| r.get(0))?;
            // A flood of unknown logins fills nothing: past a hundred, new ones are not kept.
            if waiting < 100 {
                ctl.execute(
                    "INSERT INTO person_requests (ha_user, name, display, first_seen, last_seen) VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![ha_id, clean(ha_name), clean(display), now],
                )?;
            }
            Ok(HaLogin::Pending)
        }
    }
}

/// A login waiting for, or refused by, an administrator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonRequest {
    /// The Home Assistant user id that asked.
    pub ha_user: String,
    /// Login name proposed for the new person.
    pub name: String,
    /// Display name proposed.
    pub display: String,
    /// First time the login was seen.
    pub first_seen: i64,
    /// Last time it was seen.
    pub last_seen: i64,
    /// `pending` (waiting for an administrator) or `refused`.
    pub status: String,
}

/// The logins waiting for, or refused by, an administrator: refused ones first, then pending ones, each by first sighting. Read-only.
pub fn list_requests(ctl: &Connection) -> Result<Vec<PersonRequest>> {
    let mut st = ctl.prepare(
        "SELECT ha_user, name, display, first_seen, last_seen, status FROM person_requests ORDER BY status DESC, first_seen",
    )?;
    let rows = st.query_map([], |r| {
        Ok(PersonRequest {
            ha_user: r.get(0)?,
            name: r.get(1)?,
            display: r.get(2)?,
            first_seen: r.get(3)?,
            last_seen: r.get(4)?,
            status: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `pending` or `refused`, if this login has a request.
pub fn request_status(ctl: &Connection, ha_id: &str) -> Result<Option<String>> {
    Ok(ctl
        .query_row(
            "SELECT status FROM person_requests WHERE ha_user = ?1",
            [ha_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// An administrator approves a pending login: the person is created (a member, or an
/// administrator if asked), linked to the login, and the request is removed.
pub fn approve_request(ctl: &Connection, actor: &str, ha_id: &str, admin: bool) -> Result<User> {
    let tx = rusqlite::Transaction::new_unchecked(ctl, rusqlite::TransactionBehavior::Immediate)?;
    let (name, display): (String, String) = tx
        .query_row(
            "SELECT name, display FROM person_requests WHERE ha_user = ?1 AND status = 'pending'",
            [ha_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::Invalid("no pending request for this login".into()))?;
    let unique = unique_name(&tx, &text_slug(&name))?;
    let u = create_user(&tx, actor, &unique, &display, admin)?;
    tx.execute(
        "UPDATE users SET ha_user = ?2 WHERE id = ?1",
        params![u.id, ha_id],
    )?;
    tx.execute("DELETE FROM person_requests WHERE ha_user = ?1", [ha_id])?;
    audit(&tx, actor, "request_approve", &u.id);
    tx.commit()?;
    user(ctl, &u.id)?.ok_or_else(|| Error::Invalid("person not found after approval".into()))
}

/// An administrator refuses a pending login. The row stays, marked, so it does not come back.
pub fn refuse_request(ctl: &Connection, actor: &str, ha_id: &str) -> Result<bool> {
    let n = ctl.execute(
        "UPDATE person_requests SET status = 'refused' WHERE ha_user = ?1 AND status = 'pending'",
        [ha_id],
    )?;
    if n == 1 {
        audit(ctl, actor, "request_refuse", ha_id);
    }
    Ok(n == 1)
}

fn unique_name(ctl: &Connection, base: &str) -> Result<String> {
    let mut name = base.to_string();
    let mut n = 1;
    while user_by_name(ctl, &name)?.is_some() {
        n += 1;
        name = format!("{base}-{n}");
    }
    Ok(name)
}

fn text_slug(s: &str) -> String {
    let folded = crate::text::fold(s);
    let mut out: String = folded
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if !out.starts_with(|c: char| c.is_ascii_lowercase()) {
        out = format!("user-{out}");
    }
    out.truncate(32);
    out.trim_end_matches('-').to_string()
}

/// Whether `user_id` owns `space`. Read-only.
pub fn is_space_owner(ctl: &Connection, space: &Space, user_id: &str) -> Result<bool> {
    Ok(ctl
        .query_row(
            "SELECT 1 FROM space_members WHERE space = ?1 AND user_id = ?2 AND owner = 1",
            params![space.as_str(), user_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

// --- effective access -------------------------------------------------------------------------

/// Every space a person reaches: the ones they own, with the right to write. Empty for a disabled or
/// unknown person. (There is no sharing: a space is its owner's alone.)
pub fn access_of(ctl: &Connection, user_id: &str) -> Result<BTreeMap<Space, Grant>> {
    let mut out: BTreeMap<Space, Grant> = BTreeMap::new();
    if user(ctl, user_id)?.is_none_or(|u| u.disabled) {
        return Ok(out);
    }
    let mut st = ctl.prepare("SELECT space FROM space_members WHERE user_id = ?1 AND owner = 1")?;
    let rows = st.query_map([user_id], |r| r.get::<_, String>(0))?;
    for row in rows {
        if let Ok(s) = Space::new(&row?) {
            out.insert(s, crate::access::RW);
        }
    }
    Ok(out)
}

// --- not losing a memory with its account ----------------------------------------------------------

/// How long a person must have been away before their heir may attach their memory elsewhere.
pub const HEIR_WAIT_SECS: i64 = 30 * 86_400;

/// Names the person who may take over (or none). Not oneself, and someone who exists.
pub fn set_heir(ctl: &Connection, id: &str, heir: Option<&str>) -> Result<()> {
    if let Some(h) = heir {
        if h == id {
            return Err(Error::Invalid("you cannot be your own heir".into()));
        }
        if user(ctl, h)?.is_none_or(|u| u.disabled) {
            return Err(Error::Invalid("the heir must be an active person".into()));
        }
    }
    ctl.execute(
        "UPDATE users SET heir = ?2 WHERE id = ?1",
        params![id, heir],
    )?;
    audit(ctl, id, "heir_set", heir.unwrap_or("none"));
    Ok(())
}

/// The person `id` named as their heir, if any. Read-only.
pub fn heir_of(ctl: &Connection, id: &str) -> Result<Option<String>> {
    Ok(ctl
        .query_row("SELECT heir FROM users WHERE id = ?1", [id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// The people who named this person their heir.
pub fn heir_to(ctl: &Connection, id: &str) -> Result<Vec<User>> {
    let mut st = ctl.prepare(&format!(
        "SELECT {USER_COLS} FROM users WHERE heir = ?1 AND disabled_at IS NULL ORDER BY name"
    ))?;
    Ok(st
        .query_map([id], user_of)?
        .collect::<rusqlite::Result<_>>()?)
}

/// Notes that the person opened their page (at most once a minute).
pub fn touch_seen(ctl: &Connection, id: &str, now: i64) -> Result<()> {
    ctl.execute(
        "UPDATE users SET last_seen = ?2 WHERE id = ?1 AND (last_seen IS NULL OR last_seen < ?2 - 60)",
        params![id, now],
    )?;
    Ok(())
}

/// When the person was last seen, or when they were made if never.
pub fn last_seen(ctl: &Connection, id: &str) -> Result<i64> {
    Ok(ctl.query_row(
        "SELECT coalesce(last_seen, created_at) FROM users WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?)
}

fn key_digest(key: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let clean: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    Sha256::digest(clean.as_bytes()).to_vec()
}

/// Makes a recovery key for the person (replacing any earlier one) and returns it: shown once,
/// only its digest is kept. With it, a new account takes the memory over.
pub fn make_recovery_key(ctl: &Connection, id: &str) -> Result<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let bytes: [u8; 20] = util::random_bytes();
    let chars: String = bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect();
    let key = chars
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("-");
    ctl.execute(
        "UPDATE users SET recovery_hash = ?2 WHERE id = ?1",
        params![id, key_digest(&key)],
    )?;
    audit(ctl, id, "recovery_key_made", "");
    Ok(key)
}

/// Removes the person's recovery key: no new account can take their memory over with it any more.
pub fn clear_recovery_key(ctl: &Connection, id: &str) -> Result<()> {
    ctl.execute("UPDATE users SET recovery_hash = NULL WHERE id = ?1", [id])?;
    audit(ctl, id, "recovery_key_cleared", "");
    Ok(())
}

/// Whether a recovery key was made for the person (the key itself is never stored, only its digest). Read-only.
pub fn has_recovery_key(ctl: &Connection, id: &str) -> Result<bool> {
    Ok(ctl
        .query_row(
            "SELECT recovery_hash IS NOT NULL FROM users WHERE id = ?1",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// A Home Assistant account that holds a recovery key takes over the person it belongs to. The
/// key is used up. Compared in constant time over every key.
pub fn claim_with_recovery_key(ctl: &Connection, ha_id: &str, key: &str) -> Result<Option<User>> {
    let want = key_digest(key);
    let mut st =
        ctl.prepare("SELECT id, recovery_hash FROM users WHERE recovery_hash IS NOT NULL AND disabled_at IS NULL")?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut found: Option<String> = None;
    for row in rows {
        let (id, hash) = row?;
        if util::ct_eq(&hash, &want) {
            found = Some(id);
        }
    }
    let Some(id) = found else {
        return Ok(None);
    };
    ctl.execute(
        "UPDATE users SET ha_user = ?2, recovery_hash = NULL WHERE id = ?1",
        params![id, ha_id],
    )?;
    ctl.execute("DELETE FROM person_requests WHERE ha_user = ?1", [ha_id])?;
    audit(ctl, &id, "recovered", "");
    user(ctl, &id)
}

/// The heir of `person` attaches a waiting Home Assistant account to them, when the person has not
/// been seen for [`HEIR_WAIT_SECS`]. The account takes the memory over.
pub fn attach_request(
    ctl: &Connection,
    heir_id: &str,
    ha_id: &str,
    person_id: &str,
    now: i64,
) -> Result<User> {
    if heir_of(ctl, person_id)?.as_deref() != Some(heir_id) {
        return Err(Error::Invalid("you are not this person's heir".into()));
    }
    if now - last_seen(ctl, person_id)? < HEIR_WAIT_SECS {
        return Err(Error::Invalid(
            "this person was seen less than 30 days ago: their heir cannot attach their memory yet"
                .into(),
        ));
    }
    if request_status(ctl, ha_id)?.as_deref() != Some("pending") {
        return Err(Error::Invalid("no such waiting account".into()));
    }
    ctl.execute(
        "UPDATE users SET ha_user = ?2 WHERE id = ?1",
        params![person_id, ha_id],
    )?;
    ctl.execute("DELETE FROM person_requests WHERE ha_user = ?1", [ha_id])?;
    audit(ctl, heir_id, "attached_by_heir", person_id);
    user(ctl, person_id)?.ok_or_else(|| Error::Schema("person vanished".into()))
}
