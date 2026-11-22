//! The life of a space: create it, change it, and delete it without ever destroying it.
//!
//! A space is deleted with a choice. `Into` another space: every active note, file and tag goes there through the merge
//! engine (nothing is deleted, merging twice changes nothing), and the keywords and tags of the space join the
//! target's. `Archive`: a full export is written to the archive folder, then the live database and its files are moved
//! aside (not erased) and the space leaves the layout. In both cases the space is archived at the end, so its whole
//! history is kept and `restore_space` brings it back. A space with children asks what to do with them.
//!
//! The archive folder is `<data>/archive` (`MEMCP_ARCHIVE` moves it): one folder per archived space,
//! `<space>-<time>/`, with `archive.json` (what the space was), `export/` (the export, checked) and `live/` (the
//! database and its files).
//!
//! `archive.json` is also the journal of the deletion and of the restore (`state`: `deleting`, `archived`,
//! `restoring`): each step is skipped once done, so [`Brain::recover`] finishes one a crash cut off (see
//! `docs/CRASH.md`).

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::access::auth_event;
use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::layout::Change;
use crate::merge::{self, Action, Line, Plan, Policy, Preview};
use crate::rules::{self, Rules};
use crate::space::Space;
use crate::triage;
use crate::users::{self, is_space_owner};
use crate::util;

/// Where the notes of a deleted space go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Everything goes into this other space.
    Into(Space),
    /// The space is archived: a full export, the database moved aside.
    Archive,
}

/// What becomes of the children of a deleted space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Children {
    /// Under the deleted space's own parent (the default).
    #[default]
    Grandparent,
    /// To the top of the tree.
    Top,
    /// Under another space.
    Under(Space),
}

impl Children {
    /// Parse.
    pub fn parse(s: &str) -> Result<Children> {
        match s {
            "grandparent" => Ok(Children::Grandparent),
            "top" => Ok(Children::Top),
            other => match other.strip_prefix("under:") {
                Some(p) => Ok(Children::Under(Space::new(p)?)),
                None => Err(Error::Invalid(
                    "children: grandparent, top or under:SPACE".into(),
                )),
            },
        }
    }
}

/// A new space.
#[derive(Clone, Debug, Default)]
pub struct SpaceNew {
    /// A kind of the catalogue; `custom` when none.
    pub kind: Option<String>,
    /// The label.
    pub label: String,
    /// The parent.
    pub parent: Option<Space>,
    /// The keywords.
    pub keywords: Vec<String>,
    /// The tags.
    pub tags: Vec<String>,
}

/// What to change in a space. `None` leaves a field as it is.
#[derive(Clone, Debug, Default)]
pub struct SpaceUpdate {
    /// The layout.
    pub layout: Change,
    /// The kind.
    pub kind: Option<String>,
    /// The rules.
    pub rules: rules::Edit,
}

/// A space as its owner reads it.
#[derive(Clone, Debug)]
pub struct SpaceInfo {
    /// The space.
    pub space: Space,
    /// The label.
    pub label: String,
    /// The kind.
    pub kind: String,
    /// The parent.
    pub parent: Option<Space>,
    /// The position.
    pub position: i64,
    /// The facts.
    pub facts: i64,
    /// The files.
    pub files: i64,
    /// The children.
    pub children: Vec<Space>,
    /// The rules.
    pub rules: Rules,
}

impl SpaceInfo {
    /// To json.
    pub fn to_json(&self) -> Value {
        json!({
            "space": self.space.as_str(), "label": self.label, "kind": self.kind,
            "parent": self.parent.as_ref().map(Space::as_str), "position": self.position,
            "facts": self.facts, "files": self.files,
            "children": self.children.iter().map(Space::as_str).collect::<Vec<_>>(),
            "tags": self.rules.tags, "keywords": self.rules.keywords,
        })
    }
}

/// What a rename did, or (with `dry_run`) would do.
#[derive(Clone, Debug, Default)]
pub struct Renamed {
    /// The dry run.
    pub dry_run: bool,
    /// The old name.
    pub from: String,
    /// The new name.
    pub to: String,
    /// The kind of the space (`inbox` once renamed, for a box that had the old kind).
    pub kind: String,
    /// Grants of connections on the space.
    pub grants: i64,
    /// Path limits of connections on the space.
    pub path_limits: i64,
    /// Sub-spaces under it.
    pub sub_spaces: i64,
    /// Whether it has a folder of stored files.
    pub has_files: bool,
}

/// What a delete did, or (with `dry_run`) would do.
#[derive(Clone, Debug, Default)]
pub struct DeleteReport {
    /// The dry run.
    pub dry_run: bool,
    /// The space.
    pub space: String,
    /// The label.
    pub label: String,
    /// The facts.
    pub facts: i64,
    /// The files.
    pub files: i64,
    /// The tags.
    pub tags: usize,
    /// `into` or `archive`.
    pub disposition: String,
    /// The into.
    pub into: Option<String>,
    /// Notes of the space the target already holds (same text), so not copied again.
    pub already: i64,
    /// Each child and the space it goes under (`None`: the top).
    pub children: Vec<(String, Option<String>)>,
    /// Connections that lose access to the space.
    pub connections: Vec<String>,
    /// Where the archive is (or will be).
    pub archive: Option<PathBuf>,
    /// What the merge did (not a dry run, `into` only).
    pub merged: Option<Value>,
}

impl DeleteReport {
    /// To json.
    pub fn to_json(&self) -> Value {
        json!({
            "dry_run": self.dry_run, "space": self.space, "label": self.label,
            "facts": self.facts, "files": self.files, "tags": self.tags,
            "disposition": self.disposition, "into": self.into, "already": self.already,
            "children": self.children.iter().map(|(c, p)| json!({ "space": c, "goes_under": p })).collect::<Vec<_>>(),
            "connections": self.connections,
            "archive": self.archive.as_ref().map(|p| p.to_string_lossy().to_string()),
            "merged": self.merged,
        })
    }

    /// One sentence for a confirmation.
    pub fn sentence(&self) -> String {
        let what = match &self.into {
            Some(t) => format!(
                "{} notes and {} files of {} go into {t} ({} are already there)",
                self.facts, self.files, self.label, self.already
            ),
            None => format!(
                "{} notes and {} files of {} are archived",
                self.facts, self.files, self.label
            ),
        };
        let kids = if self.children.is_empty() {
            String::new()
        } else {
            format!("; {} sub-space(s) move up", self.children.len())
        };
        let conns = if self.connections.is_empty() {
            String::new()
        } else {
            format!("; {} connection(s) lose access", self.connections.len())
        };
        format!("{what}{kids}{conns}. Nothing is erased: the space can be restored.")
    }
}

/// An archived space.
#[derive(Clone, Debug)]
pub struct Archived {
    /// The folder name, what `restore_space` takes.
    pub name: String,
    /// The space.
    pub space: String,
    /// The label.
    pub label: String,
    /// The kind.
    pub kind: String,
    /// The at.
    pub at: i64,
    /// The facts.
    pub facts: i64,
    /// The into.
    pub into: Option<String>,
}

impl Archived {
    /// To json.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name, "space": self.space, "label": self.label, "kind": self.kind,
            "archived_at": util::iso(self.at), "facts": self.facts, "into": self.into,
        })
    }
}

impl Brain {
    /// The archive folder.
    pub fn archive_root(&self) -> PathBuf {
        crate::env::var_os("MEMCP_ARCHIVE")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.dir().join("archive"))
    }

    fn owned_user(&self, user_id: &str) -> Result<users::User> {
        let ctl = self.open_control()?;
        users::user(&ctl, user_id)?
            .ok_or_else(|| Error::Invalid(format!("unknown person {user_id}")))
    }

    fn owned_space(&self, user_id: &str, space: &Space) -> Result<()> {
        let ctl = self.open_control()?;
        if !is_space_owner(&ctl, space, user_id)? {
            return Err(Error::Invalid(format!(
                "{space} is not a space of this person"
            )));
        }
        Ok(())
    }

    fn not_triage(&self, space: &Space) -> Result<()> {
        if triage::is_triage(self, space) {
            return Err(Error::Invalid(
                "the Inbox is not a space to change or delete".into(),
            ));
        }
        Ok(())
    }

    /// Reads one of a person's spaces.
    pub fn space_info(&self, user_id: &str, space: &Space) -> Result<SpaceInfo> {
        self.owned_space(user_id, space)?;
        let places = self.layout(user_id)?;
        let p = places
            .iter()
            .find(|p| &p.space == space)
            .ok_or_else(|| Error::UnknownSpace(space.to_string()))?;
        let conn = self.open_space(space)?;
        let n = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        Ok(SpaceInfo {
            space: space.clone(),
            label: p.label.clone(),
            kind: p.kind.clone(),
            parent: p.parent.clone(),
            position: p.position,
            facts: n("SELECT count(*) FROM facts WHERE status = 'active'"),
            files: n("SELECT count(*) FROM files WHERE status = 'active'"),
            children: places
                .iter()
                .filter(|c| c.parent.as_ref() == Some(space))
                .map(|c| c.space.clone())
                .collect(),
            rules: rules::read(&conn).1,
        })
    }

    /// Creates a space of the person's own: a kind, a label, perhaps under a parent, with keywords.
    pub fn create_space_with(&self, user_id: &str, person: &str, new: &SpaceNew) -> Result<Space> {
        let kind = new.kind.clone().filter(|k| k != crate::catalogue::CUSTOM);
        if let Some(k) = &kind {
            if crate::catalogue::is_inbox_kind(k) {
                return Err(Error::Invalid("the Inbox is made for you already".into()));
            }
            if k != crate::catalogue::FOLDER && !self.catalogue()?.iter().any(|c| &c.key == k) {
                return Err(Error::Invalid(format!("unknown kind {k:?}")));
            }
        }
        // Its kind, label, place, keywords and tags come with it, in one creation.
        let kind = kind.unwrap_or_else(|| crate::catalogue::CUSTOM.to_string());
        let space = self.add_space_placed(
            user_id,
            person,
            &new.label,
            new.parent.as_ref(),
            &kind,
            &|conn| {
                if new.keywords.is_empty() && new.tags.is_empty() {
                    return Ok(());
                }
                let declared: String =
                    conn.query_row("SELECT v FROM meta WHERE k = 'space'", [], |r| r.get(0))?;
                rules::edit(
                    conn,
                    &Space::new(&declared)?,
                    person,
                    &rules::Edit {
                        add_keywords: new.keywords.clone(),
                        add_tags: new.tags.clone(),
                        ..Default::default()
                    },
                )?;
                Ok(())
            },
        )?;
        auth_event(
            &self.open_control()?,
            Some(user_id),
            None,
            "space_create",
            &format!("{space} {}", new.label),
        );
        Ok(space)
    }

    /// Changes a space: label, parent, position, kind, keywords and tags.
    pub fn update_space(
        &self,
        user_id: &str,
        person: &str,
        space: &Space,
        u: &SpaceUpdate,
    ) -> Result<SpaceInfo> {
        self.owned_space(user_id, space)?;
        // The Inbox may be given another label, nothing else of it changes (its kind, place, rules are its own).
        let only_label = u.kind.is_none()
            && u.layout.parent.is_none()
            && u.layout.position.is_none()
            && u.rules.add_tags.is_empty()
            && u.rules.remove_tags.is_empty()
            && u.rules.add_keywords.is_empty()
            && u.rules.remove_keywords.is_empty();
        if !(triage::is_triage(self, space) && only_label) {
            self.not_triage(space)?;
        }
        if u.kind
            .as_deref()
            .is_some_and(crate::catalogue::is_inbox_kind)
        {
            return Err(Error::Invalid("a space cannot become the Inbox".into()));
        }
        let l = &u.layout;
        if l.label.is_some() || l.parent.is_some() || l.position.is_some() || u.kind.is_some() {
            self.set_layout_and_kind(user_id, space, l, u.kind.as_deref())?;
        }
        let e = &u.rules;
        if !(e.add_tags.is_empty()
            && e.remove_tags.is_empty()
            && e.add_keywords.is_empty()
            && e.remove_keywords.is_empty())
        {
            rules::edit(&self.open_space(space)?, space, person, e)?;
        }
        auth_event(
            &self.open_control()?,
            Some(user_id),
            None,
            "space_update",
            space.as_str(),
        );
        self.space_info(user_id, space)
    }

    fn children_plan(
        &self,
        user_id: &str,
        space: &Space,
        policy: &Children,
    ) -> Result<Vec<(Space, Option<Space>)>> {
        let places = self.layout(user_id)?;
        let me = places.iter().find(|p| &p.space == space);
        let grand = me.and_then(|p| p.parent.clone());
        let target = match policy {
            Children::Grandparent => grand,
            Children::Top => None,
            Children::Under(p) => {
                if p == space {
                    return Err(Error::Invalid(
                        "the children cannot go under the space being deleted".into(),
                    ));
                }
                self.owned_space(user_id, p)?;
                Some(p.clone())
            }
        };
        Ok(places
            .iter()
            .filter(|p| p.parent.as_ref() == Some(space))
            .map(|p| (p.space.clone(), target.clone()))
            .collect())
    }

    /// Renames a space: its name, on disk (the database, the folder of its files) and in every row of the control
    /// database that names it (its owner and members, the grants and path limits of the connections, the folders, the sync
    /// state and policy, the approvals of guides, the sub-spaces under it). The label, the notes and their ids do not
    /// change. A box of the kind the Inbox had before it was in English takes the kind `inbox`. With `dry_run`, only says
    /// what it would do. Do it while the node is quiet: a connection that holds the old name is refused after it.
    pub fn rename_space(
        &self,
        user_id: &str,
        old: &Space,
        new: &Space,
        dry_run: bool,
    ) -> Result<Renamed> {
        self.owned_space(user_id, old)?;
        if old == new {
            return Err(Error::Invalid("the new name is the old one".into()));
        }
        if self.has_space(new)? {
            return Err(Error::Invalid(format!(
                "a space named {new} already exists"
            )));
        }
        let db_old = self.dir().join(old.file_name());
        let db_new = self.dir().join(new.file_name());
        let dir_old = self.dir().join("files").join(old.as_str());
        let dir_new = self.dir().join("files").join(new.as_str());
        if db_new.exists() || dir_new.exists() {
            return Err(Error::Invalid(format!(
                "{new} is the name of something on disk already: nothing was changed"
            )));
        }
        let ctl = self.open_control()?;
        let count = |sql: &str| -> i64 {
            ctl.query_row(sql, [old.as_str()], |r| r.get(0))
                .unwrap_or(0)
        };
        let mut report = Renamed {
            dry_run,
            from: old.to_string(),
            to: new.to_string(),
            kind: self.space_kind(old).unwrap_or_default(),
            grants: count("SELECT count(*) FROM grants WHERE space = ?1"),
            path_limits: count("SELECT count(*) FROM grant_paths WHERE space = ?1"),
            sub_spaces: count("SELECT count(*) FROM spaces WHERE parent = ?1"),
            has_files: dir_old.exists(),
        };
        if dry_run {
            return Ok(report);
        }
        let _busy = crate::recover::busy(self)?;
        // 1. The notes name their space; the log is folded into the file so that it is the whole database.
        {
            let conn = self.open_space(old)?;
            conn.execute("UPDATE facts SET space = ?1", [new.as_str()])?;
            // The file names its space: it is refused under any other name.
            conn.execute("UPDATE meta SET v = ?1 WHERE k = 'space'", [new.as_str()])?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        }
        // 2. The files on disk (the database first: a crash after it leaves the old name in the control database, and
        // running the command again finds the database under its new name and finishes the rest).
        fs::rename(&db_old, &db_new)?;
        for ext in ["-wal", "-shm"] {
            let _ = fs::remove_file(self.dir().join(format!("{}{ext}", old.file_name())));
        }
        if dir_old.exists() {
            fs::rename(&dir_old, &dir_new)?;
        }
        crate::fsx::sync_dir(self.dir())?;
        // 3. The control database, in one transaction.
        let done = (|| -> Result<()> {
            let tx = rusqlite::Transaction::new_unchecked(
                &ctl,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
            let (o, n) = (old.as_str(), new.as_str());
            let label: Option<String> =
                tx.query_row("SELECT label FROM spaces WHERE name = ?1", [o], |r| {
                    r.get(0)
                })?;
            if label.is_none() && crate::layout::builtin_labels(&report.kind).is_none() {
                let owner = crate::layout::owner_of(&tx, &Space::new(o)?)?;
                crate::layout::check_unique_label(&tx, new, &report.kind, None, &owner)?;
            }
            tx.execute("UPDATE spaces SET name = ?2 WHERE name = ?1", [o, n])?;
            for table in [
                "spaces SET parent",
                "space_members SET space",
                "grants SET space",
                "grant_paths SET space",
                "guide_approvals SET space",
                "sync_state SET space",
                "sync_policy SET space",
                "space_folders SET space",
                "folder_state SET space",
            ] {
                tx.execute(
                    &format!(
                        "UPDATE {table} = ?2 WHERE {} = ?1",
                        table.rsplit(' ').next().unwrap_or("space")
                    ),
                    [o, n],
                )?;
            }
            // The Inbox of a memory made before the kind had an English name takes it, and shows as « Inbox ».
            if report.kind == crate::catalogue::TRIAGE_LEGACY {
                tx.execute(
                    "UPDATE spaces SET kind = ?2 WHERE name = ?1",
                    [n, crate::catalogue::TRIAGE],
                )?;
                report.kind = crate::catalogue::TRIAGE.to_string();
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(e) = done {
            // Put the disk back as it was, so that the node still finds the space under its name.
            let _ = fs::rename(&db_new, &db_old);
            if let Ok(conn) = rusqlite::Connection::open(&db_old) {
                let _ = conn.execute("UPDATE meta SET v = ?1 WHERE k = 'space'", [old.as_str()]);
                let _ = conn.execute("UPDATE facts SET space = ?1", [old.as_str()]);
            }
            if dir_new.exists() {
                let _ = fs::rename(&dir_new, &dir_old);
            }
            return Err(e);
        }
        auth_event(
            &ctl,
            Some(user_id),
            None,
            "space_rename",
            &format!("{old} {new}"),
        );
        Ok(report)
    }

    /// Deletes a space with a choice (see the module). With `dry_run`, only says what it would do.
    pub fn delete_space(
        &self,
        user_id: &str,
        space: &Space,
        disposition: &Disposition,
        children: &Children,
        dry_run: bool,
        at: i64,
    ) -> Result<DeleteReport> {
        self.owned_space(user_id, space)?;
        self.not_triage(space)?;
        let who = self.owned_user(user_id)?;
        let info = self.space_info(user_id, space)?;
        let into = match disposition {
            Disposition::Into(t) => {
                if t == space {
                    return Err(Error::Invalid("a space cannot go into itself".into()));
                }
                self.owned_space(user_id, t)?;
                self.not_triage(t)?;
                Some(t.clone())
            }
            Disposition::Archive => None,
        };
        let kids = self.children_plan(user_id, space, children)?;
        let ctl = self.open_control()?;
        let connections: Vec<String> = ctl
            .prepare(
                "SELECT DISTINCT c.agent FROM connections c JOIN grants g ON g.connection_id = c.id
                 WHERE g.space = ?1 AND c.revoked_at IS NULL ORDER BY c.agent",
            )?
            .query_map([space.as_str()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let name = format!("{space}-{}", util::compact(at));
        let archive = self.archive_root().join(&name);
        let mut report = DeleteReport {
            dry_run,
            space: space.to_string(),
            label: info.label.clone(),
            facts: info.facts,
            files: info.files,
            tags: info.rules.tags.len() + info.rules.keywords.len(),
            disposition: if into.is_some() { "into" } else { "archive" }.into(),
            into: into.as_ref().map(Space::to_string),
            children: kids
                .iter()
                .map(|(c, p)| (c.to_string(), p.as_ref().map(Space::to_string)))
                .collect(),
            connections,
            archive: Some(archive.clone()),
            ..Default::default()
        };
        if let Some(t) = &into {
            let src = self.open_space(space)?;
            let dst = self.open_space(t)?;
            let mut st = src.prepare("SELECT text_hash FROM facts WHERE status = 'active'")?;
            let hashes: Vec<String> = st
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for h in hashes {
                let there: Option<i64> = dst
                    .query_row(
                        "SELECT 1 FROM facts WHERE status = 'active' AND text_hash = ?1",
                        [&h],
                        |r| r.get(0),
                    )
                    .optional()?;
                if there.is_some() {
                    report.already += 1;
                }
            }
        }
        if dry_run {
            return Ok(report);
        }
        let _busy = crate::recover::busy(self)?;
        if archive.exists() {
            return Err(Error::Invalid(format!(
                "{} already exists: try again in a second",
                archive.display()
            )));
        }
        fs::create_dir_all(&archive)?;
        let _ = crate::fsx::set_private_dir(&archive);
        // 1. The export, checked, before anything moves.
        let kind = info.kind.clone();
        let exported =
            crate::export::export_space(self, &who, space, &kind, &archive.join("export"), at)?;
        crate::export::verify_export(&exported)?;
        // 2. The journal: from here on the deletion ends done, at the latest when the node starts again.
        let journal = self.deletion(
            user_id, &who.name, space, &info, &report, &kids, &exported, at,
        )?;
        write_journal(&archive, &journal)?;
        crate::failpoint::hit(crate::failpoint::at::ARCHIVE_JOURNAL)?;
        report.merged = self.finish_delete(&archive, &journal)?;
        auth_event(
            &self.open_control()?,
            Some(user_id),
            None,
            "space_delete",
            &format!("{space} {} {}", report.disposition, name),
        );
        Ok(report)
    }

    /// What the journal of a deletion holds: what the space was (the manifest of the archive), and what is left to
    /// do (`state` `deleting`, the target, the children, the export).
    #[allow(clippy::too_many_arguments)]
    fn deletion(
        &self,
        user_id: &str,
        user_name: &str,
        space: &Space,
        info: &SpaceInfo,
        report: &DeleteReport,
        kids: &[(Space, Option<Space>)],
        exported: &Path,
        at: i64,
    ) -> Result<Value> {
        let ctl = self.open_control()?;
        let mut grants: Vec<Value> = ctl
            .prepare(
                "SELECT c.agent, g.can_write, c.id FROM grants g JOIN connections c ON c.id = g.connection_id
                 WHERE g.space = ?1 ORDER BY c.agent",
            )?
            .query_map([space.as_str()], |r| {
                Ok(json!({ "agent": r.get::<_, String>(0)?, "write": r.get::<_, i64>(1)? != 0,
                           "id": r.get::<_, String>(2)? }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        // A connection limited to paths of the space keeps those limits: without them it would come back reaching the
        // whole space.
        for g in &mut grants {
            let id = g["id"].as_str().unwrap_or_default().to_string();
            let paths: Vec<Value> = ctl
                .prepare(
                    "SELECT path, can_write FROM grant_paths WHERE connection_id = ?1 AND space = ?2 ORDER BY path",
                )?
                .query_map(params![id, space.as_str()], |r| {
                    Ok(json!({ "path": r.get::<_, String>(0)?, "write": r.get::<_, i64>(1)? != 0 }))
                })?
                .collect::<rusqlite::Result<_>>()?;
            if let Some(o) = g.as_object_mut() {
                o.remove("id");
                if !paths.is_empty() {
                    o.insert("paths".into(), Value::Array(paths));
                }
            }
        }
        let created: i64 = ctl
            .query_row(
                "SELECT created_at FROM spaces WHERE name = ?1",
                [space.as_str()],
                |r| r.get(0),
            )
            .unwrap_or(at);
        let stored_label: Option<String> = ctl.query_row(
            "SELECT label FROM spaces WHERE name = ?1",
            [space.as_str()],
            |r| r.get(0),
        )?;
        Ok(json!({
            "stored_label": stored_label,
            "format": "memcp-archived-space", "version": 1, "state": DELETING,
            "space": space.as_str(), "label": info.label, "kind": info.kind,
            "parent": info.parent.as_ref().map(Space::as_str), "position": info.position,
            "owner": user_id, "created_at": created, "archived_at": at,
            "disposition": report.disposition, "into": report.into, "facts": info.facts,
            "grants": grants,
            "user_name": user_name,
            "export": exported.to_string_lossy(),
            "tags": info.rules.tags, "keywords": info.rules.keywords,
            "children": kids.iter().map(|(c, p)| json!({ "space": c.as_str(), "goes_under": p.as_ref().map(Space::as_str) })).collect::<Vec<_>>(),
        }))
    }

    /// Does what the journal of a deletion says is left: into another space, the merge (merging twice changes
    /// nothing) and the rules joined; the children moved up; the space unregistered with the mark of its archive, in
    /// one control transaction; its database and files set aside; the journal closed (`archived`). Each step is
    /// skipped once done, so it runs again after a crash at any point.
    fn finish_delete(&self, archive: &Path, j: &Value) -> Result<Option<Value>> {
        let space = Space::new(j["space"].as_str().unwrap_or_default())?;
        let owner = j["owner"].as_str().unwrap_or_default();
        let user_name = j["user_name"].as_str().unwrap_or(owner);
        let at = j["archived_at"].as_i64().unwrap_or_else(util::now);
        let mut merged = None;
        if self.has_space(&space)? {
            if let Some(t) = j["into"].as_str() {
                let t = Space::new(t)?;
                let plan = Plan {
                    export: PathBuf::from(j["export"].as_str().unwrap_or_default()),
                    user: user_name.to_string(),
                    created_at: util::iso(at),
                    lines: vec![Line {
                        source: space.to_string(),
                        kind: j["kind"].as_str().unwrap_or("custom").to_string(),
                        action: Action::Merge,
                        target: t.to_string(),
                        rename: None,
                        policy: Policy::KeepBoth,
                        preview: Preview::default(),
                        note: "space deleted into another".into(),
                        guide: "none".into(),
                    }],
                };
                let r = merge::apply_in_place(self, &plan)?;
                let line = r.lines.first().cloned().unwrap_or_default();
                merged = Some(json!({
                    "added": line.added, "tagged": line.tagged, "files": line.files, "left": line.left,
                }));
                crate::failpoint::hit(crate::failpoint::at::ARCHIVE_MERGED)?;
                // The keywords and tags join the target's (written again, they change nothing).
                let before = rules::read(&self.open_space(&t)?).1;
                let mut joined = Rules {
                    tags: before.tags,
                    keywords: before.keywords,
                    sources: before.sources,
                };
                joined.tags.extend(strings(&j["tags"]));
                joined.keywords.extend(strings(&j["keywords"]));
                if let Some(k) = rules::clean_keyword(j["label"].as_str().unwrap_or_default()) {
                    joined.keywords.push(k);
                }
                rules::write(&self.open_space(&t)?, &t, user_name, &joined)?;
            }
            // The children move up (those still under it).
            for c in j["children"].as_array().into_iter().flatten() {
                let child = Space::new(c["space"].as_str().unwrap_or_default())?;
                let under: Option<Option<String>> = self
                    .open_control()?
                    .query_row(
                        "SELECT parent FROM spaces WHERE name = ?1",
                        [child.as_str()],
                        |r| r.get(0),
                    )
                    .optional()?;
                if under.flatten().as_deref() != Some(space.as_str()) {
                    continue;
                }
                let goes = match c["goes_under"].as_str() {
                    Some(p) => Some(Space::new(p)?),
                    None => None,
                };
                self.set_layout(
                    owner,
                    &child,
                    &Change {
                        parent: Some(goes),
                        ..Default::default()
                    },
                )?;
            }
            self.unregister(&space, owner, at)?;
            crate::failpoint::hit(crate::failpoint::at::ARCHIVE_UNREGISTERED)?;
        }
        // Aside, not erased.
        shift(
            &pieces(self, &space, &archive.join("live")),
            true,
            Some(crate::failpoint::at::ARCHIVE_DB),
        )?;
        let mut done = j.clone();
        done["state"] = json!(ARCHIVED);
        write_journal(archive, &done)?;
        Ok(merged)
    }

    /// The space leaves the control database, and the mark of its archive is kept for the other nodes (see `swarm`),
    /// in one transaction: a space unregistered here is always one the peers are told is archived.
    fn unregister(&self, space: &Space, owner: &str, at: i64) -> Result<()> {
        // Fold the write-ahead log into the file so that one file is the whole database.
        {
            let c = self.open_space(space)?;
            let _: rusqlite::Result<i64> =
                c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(1));
        }
        let ctl = self.open_control()?;
        let tx =
            rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
        for sql in [
            "DELETE FROM grant_paths WHERE space = ?1",
            "DELETE FROM grants WHERE space = ?1",
            "DELETE FROM group_spaces WHERE space = ?1",
            "DELETE FROM pairing_codes WHERE space = ?1",
            "DELETE FROM devices WHERE space = ?1",
            "DELETE FROM sync_state WHERE space = ?1",
            "DELETE FROM folder_state WHERE space = ?1",
            "DELETE FROM space_folders WHERE space = ?1",
            "DELETE FROM space_members WHERE space = ?1",
            "DELETE FROM spaces WHERE name = ?1",
        ] {
            tx.execute(sql, [space.as_str()])?;
        }
        tx.execute(
            "DELETE FROM meta WHERE k = ?1",
            [crate::layout::pending_key(space)],
        )?;
        crate::swarm::mark_archived_in(&tx, space, owner, at)?;
        tx.commit()?;
        Ok(())
    }

    /// The archived spaces of a person, newest first.
    pub fn archived_spaces(&self, user_id: &str) -> Result<Vec<Archived>> {
        let mut out = vec![];
        let Ok(rd) = fs::read_dir(self.archive_root()) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            let Ok(text) = fs::read_to_string(e.path().join(JOURNAL)) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if v["owner"].as_str() != Some(user_id)
                || v["state"].as_str().is_some_and(|s| s != ARCHIVED)
            {
                continue;
            }
            out.push(Archived {
                name: e.file_name().to_string_lossy().to_string(),
                space: v["space"].as_str().unwrap_or_default().into(),
                label: v["label"].as_str().unwrap_or_default().into(),
                kind: v["kind"].as_str().unwrap_or("custom").into(),
                at: v["archived_at"].as_i64().unwrap_or(0),
                facts: v["facts"].as_i64().unwrap_or(0),
                into: v["into"].as_str().map(String::from),
            });
        }
        out.sort_by(|a, b| b.at.cmp(&a.at).then(a.name.cmp(&b.name)));
        Ok(out)
    }

    /// Brings an archived space back: its database and files move back and it is registered again with its label,
    /// kind and place. Refused when the name is in use.
    pub fn restore_space(&self, user_id: &str, archive: &str, dry_run: bool) -> Result<Space> {
        if archive.contains('/') || archive.contains("..") || archive.is_empty() {
            return Err(Error::Invalid("not an archive name".into()));
        }
        let dir = self.archive_root().join(archive);
        let v: Value = serde_json::from_str(
            &fs::read_to_string(dir.join(JOURNAL))
                .map_err(|_| Error::Invalid(format!("no archived space {archive}")))?,
        )
        .map_err(|e| Error::Invalid(format!("the archive is unreadable: {e}")))?;
        if v["owner"].as_str() != Some(user_id) {
            return Err(Error::Invalid(format!("no archived space {archive}")));
        }
        if let Some(s) = v["state"].as_str().filter(|s| *s != ARCHIVED) {
            return Err(Error::Invalid(format!(
                "{archive} is {s}: the node finishes it when it starts"
            )));
        }
        let space = Space::new(v["space"].as_str().unwrap_or_default())?;
        let in_use =
            || Error::Invalid(format!("{space} exists already: delete or rename it first"));
        if self.has_space(&space)? || crate::recover::in_flight(self, &space)? {
            return Err(in_use());
        }
        let live = dir.join("live");
        if !live.join(space.file_name()).is_file() {
            return Err(Error::Invalid("the archive holds no database".into()));
        }
        crate::layout::check_unique_label(
            &self.open_control()?,
            &space,
            v["kind"].as_str().unwrap_or("custom"),
            v.get("stored_label").unwrap_or(&v["label"]).as_str(),
            user_id,
        )?;
        if dry_run {
            return Ok(space);
        }
        let _busy = crate::recover::busy(self)?;
        if self.dir().join(space.file_name()).exists()
            && !crate::recover::clear_orphan(self, &space)?
        {
            return Err(in_use());
        }
        let mut j = v.clone();
        j["state"] = json!(RESTORING);
        write_journal(&dir, &j)?;
        crate::failpoint::hit(crate::failpoint::at::RESTORE_JOURNAL)?;
        self.finish_restore(&dir, &j)?;
        auth_event(
            &self.open_control()?,
            Some(user_id),
            None,
            "space_restore",
            &format!("{space} from {archive}"),
        );
        Ok(space)
    }

    /// Does what the journal of a restore says is left: the database and files move back (never over a file there),
    /// then one control transaction registers the space with its place and grants and the mark that it is back; the
    /// journal closes (`restored.json`). Once registered, the space is the restored one: no other creation takes a
    /// name whose restore is under way.
    fn finish_restore(&self, dir: &Path, j: &Value) -> Result<Space> {
        let space = Space::new(j["space"].as_str().unwrap_or_default())?;
        let owner = j["owner"].as_str().unwrap_or_default();
        let live = dir.join("live");
        if self.has_space(&space)? {
            shift(&pieces(self, &space, &live), false, None)?;
        } else {
            let ctl = self.open_control()?;
            let tx = rusqlite::Transaction::new_unchecked(
                &ctl,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let kind = j["kind"].as_str().unwrap_or(crate::catalogue::CUSTOM);
            let label = j.get("stored_label").unwrap_or(&j["label"]).as_str();
            let paths = pieces(self, &space, &live);
            match crate::layout::check_unique_label(&tx, &space, kind, label, owner) {
                Ok(()) => {}
                Err(e @ Error::Invalid(_)) => {
                    // A conflicting label may have been entered while this interrupted restore was pending.
                    shift(&paths, true, None)?;
                    let mut archived = j.clone();
                    archived["state"] = json!(ARCHIVED);
                    write_journal(dir, &archived)?;
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
            shift(&paths, false, Some(crate::failpoint::at::RESTORE_DB))?;
            crate::failpoint::hit(crate::failpoint::at::RESTORE_FILES)?;
            Self::reregister(&tx, &space, owner, j)?;
            tx.commit()?;
            crate::failpoint::hit(crate::failpoint::at::RESTORE_REGISTERED)?;
        }
        // The file must declare this space and open cleanly.
        self.open_space(&space)?;
        fs::rename(dir.join(JOURNAL), dir.join("restored.json"))?;
        crate::fsx::sync_dir(dir)?;
        Ok(space)
    }

    /// The control transaction of a restore: the space, its owner, the grants that still have a connection, and the
    /// mark that it is back (newer than the archive, so the other nodes take it again).
    fn reregister(
        tx: &rusqlite::Connection,
        space: &Space,
        user_id: &str,
        v: &Value,
    ) -> Result<()> {
        let parent = v["parent"]
            .as_str()
            .filter(|p| {
                tx.query_row(
                    "SELECT 1 FROM space_members WHERE space = ?1 AND user_id = ?2 AND owner = 1",
                    params![p, user_id],
                    |_| Ok(()),
                )
                .is_ok()
            })
            .map(String::from);
        let label = v.get("stored_label").unwrap_or(&v["label"]).as_str();
        tx.execute(
            "INSERT INTO spaces (name, created_at, kind, label, parent, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                space.as_str(),
                v["created_at"].as_i64().unwrap_or_else(util::now),
                v["kind"].as_str().unwrap_or("custom"),
                label,
                parent,
                v["position"].as_i64().unwrap_or(0)
            ],
        )?;
        tx.execute(
            "INSERT INTO space_members (space, user_id, can_write, owner) VALUES (?1, ?2, 1, 1)",
            params![space.as_str(), user_id],
        )?;
        // The connections that reached it and still exist get their access back.
        for g in v["grants"].as_array().into_iter().flatten() {
            tx.execute(
                "INSERT OR IGNORE INTO grants (connection_id, space, can_read, can_write)
                 SELECT id, ?2, 1, ?3 FROM connections WHERE agent = ?1 AND revoked_at IS NULL
                 AND coalesce(user_id, 'owner') = ?4",
                params![
                    g["agent"].as_str().unwrap_or_default(),
                    space.as_str(),
                    i64::from(g["write"].as_bool().unwrap_or(false)),
                    user_id
                ],
            )?;
            for p in g["paths"].as_array().into_iter().flatten() {
                tx.execute(
                    "INSERT OR IGNORE INTO grant_paths (connection_id, space, path, can_write)
                     SELECT id, ?2, ?3, ?4 FROM connections WHERE agent = ?1 AND revoked_at IS NULL
                     AND coalesce(user_id, 'owner') = ?5",
                    params![
                        g["agent"].as_str().unwrap_or_default(),
                        space.as_str(),
                        p["path"].as_str().unwrap_or_default(),
                        i64::from(p["write"].as_bool().unwrap_or(false)),
                        user_id
                    ],
                )?;
            }
        }
        crate::swarm::mark_restored_in(tx, space, util::now())?;
        Ok(())
    }

    /// Finishes the archives and restores a crash cut off (run by [`Brain::recover`]): a deletion goes on to its end
    /// (into a target that is gone, it is given up and the space stays); a restore goes on to its end. An archive
    /// written by an earlier version, which moved the files before its control transaction, is put right: a space
    /// still registered gets its files back and the archive is given up (`aborted.json`); files a restore had begun
    /// to move back go back into the archive. Answers how many it settled, and what it could not.
    pub(crate) fn settle_archives(&self) -> (usize, Vec<String>) {
        let mut done = 0;
        let mut failed = vec![];
        let Ok(rd) = fs::read_dir(self.archive_root()) else {
            return (done, failed);
        };
        for e in rd.flatten() {
            let dir = e.path();
            let Ok(text) = fs::read_to_string(dir.join(JOURNAL)) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let r = match v["state"].as_str() {
                Some(DELETING) => self.settle_deletion(&dir, &v),
                Some(RESTORING) => self.finish_restore(&dir, &v).map(|_| true),
                Some(ARCHIVED) | None => self.settle_legacy(&dir, &v),
                Some(_) => Ok(false),
            };
            match r {
                Ok(true) => done += 1,
                Ok(false) => {}
                Err(e) => failed.push(format!("{}: {e}", e_name(&dir))),
            }
        }
        (done, failed)
    }

    fn settle_deletion(&self, dir: &Path, v: &Value) -> Result<bool> {
        let space = Space::new(v["space"].as_str().unwrap_or_default())?;
        let target_gone = match v["into"].as_str() {
            Some(t) => !self.has_space(&Space::new(t)?)?,
            None => false,
        };
        if target_gone && self.has_space(&space)? {
            // Nothing went into it: the space stays as it was.
            fs::rename(dir.join(JOURNAL), dir.join("aborted.json"))?;
            return Ok(true);
        }
        self.finish_delete(dir, v)?;
        Ok(true)
    }

    fn settle_legacy(&self, dir: &Path, v: &Value) -> Result<bool> {
        let Ok(space) = Space::new(v["space"].as_str().unwrap_or_default()) else {
            return Ok(false);
        };
        let live = dir.join("live");
        let in_live = live.join(space.file_name()).is_file();
        let in_data = self.dir().join(space.file_name()).is_file();
        if self.has_space(&space)? {
            // A space of that name made or brought back after the archive is another copy: left alone.
            let at = v["archived_at"].as_i64().unwrap_or(i64::MAX);
            if crate::swarm::since(self, &space)? > at || in_live == in_data {
                return Ok(false);
            }
            // The archive's control transaction never committed: the space stays, its files come back.
            shift(&pieces(self, &space, &live), false, None)?;
            fs::rename(dir.join(JOURNAL), dir.join("aborted.json"))?;
            crate::fsx::sync_dir(dir)?;
            return Ok(true);
        }
        if in_data && !in_live {
            // A restore whose control transaction never committed: the files go back into the archive.
            shift(&pieces(self, &space, &live), true, None)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Whether an archive or a restore of `space` is under way (its journal says `deleting` or `restoring`).
    pub(crate) fn archive_in_flight(&self, space: &Space) -> bool {
        let Ok(rd) = fs::read_dir(self.archive_root()) else {
            return false;
        };
        rd.flatten().any(|e| {
            fs::read_to_string(e.path().join(JOURNAL))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| {
                    v["space"].as_str() == Some(space.as_str())
                        && matches!(v["state"].as_str(), Some(DELETING | RESTORING))
                })
        })
    }
}

/// The journal of an archived space, and its manifest once archived.
const JOURNAL: &str = "archive.json";
/// The deletion is under way.
const DELETING: &str = "deleting";
/// The space is archived: its database and files are in `live/`.
const ARCHIVED: &str = "archived";
/// The restore is under way.
const RESTORING: &str = "restoring";

fn e_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect()
}

/// Writes the journal whole or not at all: a temporary file, made durable, renamed in.
fn write_journal(dir: &Path, v: &Value) -> Result<()> {
    use crate::fsx::PrivateFile as _;
    use std::io::Write as _;
    let tmp = dir.join(".archive.json.tmp");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .private_file()
        .open(&tmp)?;
    f.write_all(
        serde_json::to_string_pretty(v)
            .unwrap_or_default()
            .as_bytes(),
    )?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, dir.join(JOURNAL))?;
    crate::fsx::sync_dir(dir)?;
    Ok(())
}

/// The pieces of a space on disk, each with its place in the archive: the database, its log, its shared memory, its
/// folder of files.
fn pieces(brain: &Brain, space: &Space, live: &Path) -> Vec<(PathBuf, PathBuf)> {
    let db = brain.dir().join(space.file_name());
    let mut out = vec![(db.clone(), live.join(space.file_name()))];
    for ext in ["-wal", "-shm"] {
        out.push((
            PathBuf::from(format!("{}{ext}", db.display())),
            live.join(format!("{}{ext}", space.file_name())),
        ));
    }
    out.push((
        brain.dir().join("files").join(space.as_str()),
        live.join("files"),
    ));
    out
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn empty_dir(p: &Path) -> bool {
    fs::read_dir(p).is_ok_and(|mut d| d.next().is_none())
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_: &Path, _: &Path) -> bool {
    false
}

/// Moves the pieces of a space, `aside` into the archive or back out of it, each once and never over another file:
/// a file is linked under its new name then unlinked (a crash between leaves two names of one file, which the next
/// pass ends), a folder is renamed. A piece already moved is skipped, so it runs again after a crash. `point` stops
/// it after its first piece (the tests).
fn shift(pairs: &[(PathBuf, PathBuf)], aside: bool, point: Option<&'static str>) -> Result<()> {
    let mut first = true;
    for (a, b) in pairs {
        let (src, dst) = if aside { (a, b) } else { (b, a) };
        if !exists(src) {
            continue;
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        if exists(dst) {
            if src.is_dir() && dst.is_dir() {
                if empty_dir(dst) {
                    fs::remove_dir(dst)?;
                    fs::rename(src, dst)?;
                } else if empty_dir(src) {
                    fs::remove_dir(src)?;
                } else {
                    return Err(Error::Schema(format!(
                        "{} and {} both hold files: the node will not choose",
                        src.display(),
                        dst.display()
                    )));
                }
            } else if same_file(src, dst) {
                fs::remove_file(src)?;
            } else {
                return Err(Error::Schema(format!(
                    "{} and {} both exist: the node will not choose",
                    src.display(),
                    dst.display()
                )));
            }
        } else if src.is_dir() {
            fs::rename(src, dst)?;
        } else {
            fs::hard_link(src, dst)?;
            fs::remove_file(src)?;
        }
        for p in [src.parent(), dst.parent()].into_iter().flatten() {
            crate::fsx::sync_dir(p)?;
        }
        if first {
            first = false;
            if let Some(p) = point {
                crate::failpoint::hit(p)?;
            }
        }
    }
    Ok(())
}
