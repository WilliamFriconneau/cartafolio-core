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
    Into(Space),
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
    pub label: String,
    pub parent: Option<Space>,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
}

/// What to change in a space. `None` leaves a field as it is.
#[derive(Clone, Debug, Default)]
pub struct SpaceUpdate {
    pub layout: Change,
    pub kind: Option<String>,
    pub rules: rules::Edit,
}

/// A space as its owner reads it.
#[derive(Clone, Debug)]
pub struct SpaceInfo {
    pub space: Space,
    pub label: String,
    pub kind: String,
    pub parent: Option<Space>,
    pub position: i64,
    pub facts: i64,
    pub files: i64,
    pub children: Vec<Space>,
    pub rules: Rules,
}

impl SpaceInfo {
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

/// What a delete did, or (with `dry_run`) would do.
#[derive(Clone, Debug, Default)]
pub struct DeleteReport {
    pub dry_run: bool,
    pub space: String,
    pub label: String,
    pub facts: i64,
    pub files: i64,
    pub tags: usize,
    /// `into` or `archive`.
    pub disposition: String,
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
    pub space: String,
    pub label: String,
    pub kind: String,
    pub at: i64,
    pub facts: i64,
    pub into: Option<String>,
}

impl Archived {
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
        std::env::var_os("MEMCP_ARCHIVE")
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
                "« À trier » is not a space to change or delete".into(),
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
            if k == crate::catalogue::TRIAGE {
                return Err(Error::Invalid("« À trier » is made for you already".into()));
            }
            if !self.catalogue()?.iter().any(|c| &c.key == k) {
                return Err(Error::Invalid(format!("unknown kind {k:?}")));
            }
        }
        let space = self.add_custom_space(user_id, person, &new.label, new.parent.as_ref())?;
        if let Some(k) = &kind {
            self.set_space_kind(&space, k)?;
        }
        if !new.keywords.is_empty() || !new.tags.is_empty() {
            let conn = self.open_space(&space)?;
            rules::edit(
                &conn,
                &space,
                person,
                &rules::Edit {
                    add_keywords: new.keywords.clone(),
                    add_tags: new.tags.clone(),
                    ..Default::default()
                },
            )?;
        }
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
        self.not_triage(space)?;
        if let Some(k) = &u.kind {
            if k == crate::catalogue::TRIAGE {
                return Err(Error::Invalid("a space cannot become « À trier »".into()));
            }
            self.set_space_kind(space, k)?;
        }
        let l = &u.layout;
        if l.label.is_some() || l.parent.is_some() || l.position.is_some() {
            self.set_layout(user_id, space, l)?;
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
        // 2. Into: the merge engine copies what is active; the keywords and tags join the target's.
        if let Some(t) = &into {
            let before = rules::read(&self.open_space(t)?).1;
            let plan = Plan {
                export: exported.clone(),
                user: who.name.clone(),
                created_at: util::iso(at),
                lines: vec![Line {
                    source: space.to_string(),
                    kind: kind.clone(),
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
            report.merged = Some(json!({
                "added": line.added, "tagged": line.tagged, "files": line.files, "left": line.left,
            }));
            let mut joined = Rules {
                tags: before.tags,
                keywords: before.keywords,
            };
            joined.tags.extend(info.rules.tags.clone());
            joined.keywords.extend(info.rules.keywords.clone());
            if let Some(k) = rules::clean_keyword(&info.label) {
                joined.keywords.push(k);
            }
            rules::write(&self.open_space(t)?, t, &who.name, &joined)?;
        }
        // 3. The children move up.
        for (c, p) in &kids {
            self.set_layout(
                user_id,
                c,
                &Change {
                    parent: Some(p.clone()),
                    ..Default::default()
                },
            )?;
        }
        // 4. Aside, not erased.
        self.move_aside(user_id, space, &archive, &info, &report, at)?;
        auth_event(
            &self.open_control()?,
            Some(user_id),
            None,
            "space_delete",
            &format!("{space} {} {}", report.disposition, name),
        );
        Ok(report)
    }

    fn move_aside(
        &self,
        user_id: &str,
        space: &Space,
        archive: &Path,
        info: &SpaceInfo,
        report: &DeleteReport,
        at: i64,
    ) -> Result<()> {
        let ctl = self.open_control()?;
        let grants: Vec<Value> = ctl
            .prepare(
                "SELECT c.agent, g.can_write FROM grants g JOIN connections c ON c.id = g.connection_id
                 WHERE g.space = ?1 ORDER BY c.agent",
            )?
            .query_map([space.as_str()], |r| {
                Ok(json!({ "agent": r.get::<_, String>(0)?, "write": r.get::<_, i64>(1)? != 0 }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let created: i64 = ctl
            .query_row(
                "SELECT created_at FROM spaces WHERE name = ?1",
                [space.as_str()],
                |r| r.get(0),
            )
            .unwrap_or(at);
        let manifest = json!({
            "format": "memcp-archived-space", "version": 1,
            "space": space.as_str(), "label": info.label, "kind": info.kind,
            "parent": info.parent.as_ref().map(Space::as_str), "position": info.position,
            "owner": user_id, "created_at": created, "archived_at": at,
            "disposition": report.disposition, "into": report.into, "facts": info.facts,
            "grants": grants,
        });
        // Fold the write-ahead log into the file so that one file is the whole database.
        {
            let c = self.open_space(space)?;
            let _: rusqlite::Result<i64> =
                c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(1));
        }
        let live = archive.join("live");
        fs::create_dir_all(&live)?;
        let db = self.dir().join(space.file_name());
        let blobs = self.dir().join("files").join(space.as_str());
        let tx =
            rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
        for sql in [
            "DELETE FROM grant_paths WHERE space = ?1",
            "DELETE FROM grants WHERE space = ?1",
            "DELETE FROM group_spaces WHERE space = ?1",
            "DELETE FROM pairing_codes WHERE space = ?1",
            "DELETE FROM devices WHERE space = ?1",
            "DELETE FROM sync_state WHERE space = ?1",
            "DELETE FROM space_members WHERE space = ?1",
            "DELETE FROM spaces WHERE name = ?1",
        ] {
            tx.execute(sql, [space.as_str()])?;
        }
        fs::write(
            archive.join("archive.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )?;
        fs::rename(&db, live.join(space.file_name()))?;
        for ext in ["-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{ext}", db.display()));
        }
        if blobs.is_dir() {
            fs::rename(&blobs, live.join("files"))?;
        }
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
            let Ok(text) = fs::read_to_string(e.path().join("archive.json")) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if v["owner"].as_str() != Some(user_id) {
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
            &fs::read_to_string(dir.join("archive.json"))
                .map_err(|_| Error::Invalid(format!("no archived space {archive}")))?,
        )
        .map_err(|e| Error::Invalid(format!("the archive is unreadable: {e}")))?;
        if v["owner"].as_str() != Some(user_id) {
            return Err(Error::Invalid(format!("no archived space {archive}")));
        }
        let space = Space::new(v["space"].as_str().unwrap_or_default())?;
        if self.has_space(&space)? || self.dir().join(space.file_name()).exists() {
            return Err(Error::Invalid(format!(
                "{space} exists already: delete or rename it first"
            )));
        }
        let live = dir.join("live");
        if !live.join(space.file_name()).is_file() {
            return Err(Error::Invalid("the archive holds no database".into()));
        }
        if dry_run {
            return Ok(space);
        }
        let ctl = self.open_control()?;
        let tx =
            rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
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
        tx.execute(
            "INSERT INTO spaces (name, created_at, kind, label, parent, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                space.as_str(),
                v["created_at"].as_i64().unwrap_or_else(util::now),
                v["kind"].as_str().unwrap_or("custom"),
                v["label"].as_str(),
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
        }
        fs::rename(
            live.join(space.file_name()),
            self.dir().join(space.file_name()),
        )?;
        let blobs = live.join("files");
        if blobs.is_dir() {
            fs::create_dir_all(self.dir().join("files"))?;
            fs::rename(&blobs, self.dir().join("files").join(space.as_str()))?;
        }
        tx.commit()?;
        // The file must declare this space and open cleanly.
        self.open_space(&space)?;
        let _ = fs::rename(dir.join("archive.json"), dir.join("restored.json"));
        auth_event(
            &self.open_control()?,
            Some(user_id),
            None,
            "space_restore",
            &format!("{space} from {archive}"),
        );
        Ok(space)
    }
}
