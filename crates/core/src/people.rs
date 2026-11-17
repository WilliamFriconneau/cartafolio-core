//! Removing a person without losing a memory: **delete = disable + export**.
//!
//! The person is disabled (their keys stop at once), their whole memory is exported and checked into
//! `<archive>/people/`, and the row stays so that the export and the audit still name them. Enabling them again
//! brings everything back. Only an administrator removes someone, never themselves, and never the last
//! administrator who can sign in.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::access::{self, auth_event};
use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::export;
use crate::users::{self, User};
use crate::util;

#[derive(Clone, Debug, Default)]
/// The removal.
pub struct Removal {
    /// The dry run.
    pub dry_run: bool,
    /// The person.
    pub person: String,
    /// The spaces.
    pub spaces: Vec<String>,
    /// The connections.
    pub connections: Vec<String>,
    /// Where the export is (or will be); `None` when the person owns no space.
    pub export: Option<PathBuf>,
}

impl Removal {
    /// To json.
    pub fn to_json(&self) -> Value {
        json!({
            "dry_run": self.dry_run, "person": self.person, "spaces": self.spaces,
            "connections": self.connections,
            "export": self.export.as_ref().map(|p| p.to_string_lossy().to_string()),
        })
    }

    /// Sentence.
    pub fn sentence(&self) -> String {
        format!(
            "{} is disabled: {} connection(s) stop and the sign-in closes. Their {} space(s) are exported and kept; enabling them again brings everything back.",
            self.person,
            self.connections.len(),
            self.spaces.len()
        )
    }
}

/// Disables a person and exports their memory. With `dry_run`, only says what it would do.
pub fn retire_person(
    brain: &Brain,
    actor: &User,
    id: &str,
    dry_run: bool,
    at: i64,
) -> Result<Removal> {
    if !actor.admin {
        return Err(Error::Invalid(
            "only an administrator removes a person".into(),
        ));
    }
    if actor.id == id {
        return Err(Error::Invalid(
            "you cannot remove yourself: ask another administrator".into(),
        ));
    }
    let ctl = brain.open_control()?;
    let who =
        users::user(&ctl, id)?.ok_or_else(|| Error::Invalid(format!("unknown person {id}")))?;
    if who.disabled {
        return Err(Error::Invalid(format!("{} is already removed", who.name)));
    }
    let spaces: Vec<String> = brain
        .layout(&who.id)?
        .iter()
        .map(|p| p.space.to_string())
        .collect();
    let connections: Vec<String> = access::list(&ctl)?
        .into_iter()
        .filter(|c| c.user_id == who.id && c.revoked_at.is_none())
        .map(|c| c.agent)
        .collect();
    let dir =
        brain
            .archive_root()
            .join("people")
            .join(format!("{}-{}", who.name, util::compact(at)));
    let mut r = Removal {
        dry_run,
        person: who.name.clone(),
        spaces,
        connections,
        export: None,
    };
    r.export = (!r.spaces.is_empty()).then(|| dir.clone());
    if dry_run {
        return Ok(r);
    }
    // The last administrator who can sign in cannot go: checked before anything is written.
    if who.admin
        && users::list_users(&ctl)?
            .iter()
            .filter(|u| u.admin && !u.disabled && u.id != who.id)
            .count()
            == 0
    {
        return Err(Error::Invalid(
            "the last administrator who can sign in cannot be removed".into(),
        ));
    }
    if r.export.is_some() {
        fs::create_dir_all(&dir)?;
        let _ = crate::fsx::set_private_dir(&dir);
        if let Some(out) = export::export_all(brain, &who, &dir, at)? {
            export::verify_export(&out)?;
            r.export = Some(out);
        }
    }
    users::set_disabled(&ctl, &actor.id, &who.id, true)?;
    auth_event(
        &ctl,
        Some(&actor.id),
        None,
        "user_delete",
        &format!(
            "{} {}",
            who.name,
            r.export
                .as_ref()
                .map_or(String::new(), |p| p.display().to_string())
        ),
    );
    Ok(r)
}
