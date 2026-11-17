//! Merging two memories: the other one arrives as an export, and each of its spaces joins a
//! space of this installation, is copied as a new one, or is left out.
//!
//! Two steps, so that nothing is written by surprise:
//!
//! 1. [`plan`] reads the export and writes a *plan*: one line per space of the other memory, with
//!    its target, its action (`merge`, `copy`, `skip`), what to do with conflicts, and a preview
//!    (how many notes are new, already there, differing, how many files). It writes nothing.
//! 2. [`apply`] takes the plan (edited or not), makes a consistent backup, applies every line in
//!    one go, verifies the counts, and only then commits. Any failure changes nothing.
//!
//! Rules:
//! - Nothing is deleted. A replaced note is superseded: its text stays in its history.
//! - A note is *already there* when an active note of the target has the same text: it is not
//!   copied again, and only its missing tags are added (tags merge as sets). So merging twice
//!   changes nothing. A note the target has since retired or replaced is left retired.
//! - A *conflict* is a note whose id exists in the target, active, with another text (the two
//!   memories share an ancestor and drifted), or a dashboard note when the target has one.
//! - Only active notes travel; the history of the other memory stays in its export. Notes that
//!   describe a file travel with the file.
//! - A file already present (same fingerprint and name) is not copied again.
//! - The target's instructions (guides) and dashboard stay unless the policy is `replace`; the
//!   other memory's guides are reported as proposals. A space created by `copy` takes its guide.
//! - Connections and the layout of the other memory never travel.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::{self, Brain};
use crate::error::{Error, Result};
use crate::export;
use crate::facts::{self, Fact, NewFact};
use crate::fsx::PrivateDir as _;
use crate::guide;
use crate::schema::SPACE_MIGRATIONS;
use crate::space::Space;
use crate::text::{self, hex};
use crate::users::{self, User};
use crate::util;

pub const PLAN_FORMAT: &str = "memcp-merge-plan";
pub const PLAN_VERSION: u32 = 1;

/// What a line does with its space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Join an existing space of this installation.
    Merge,
    /// Become a new space (or, if the name has since been taken by a space of the person, join it).
    Copy,
    /// Left out.
    Skip,
}

impl Action {
    pub fn word(self) -> &'static str {
        match self {
            Action::Merge => "merge",
            Action::Copy => "copy",
            Action::Skip => "skip",
        }
    }

    pub fn parse(s: &str) -> Result<Action> {
        match s {
            "merge" => Ok(Action::Merge),
            "copy" => Ok(Action::Copy),
            "skip" => Ok(Action::Skip),
            _ => Err(Error::Invalid(format!(
                "unknown action {s:?} (merge, copy, skip)"
            ))),
        }
    }
}

/// What happens when the same note is on both sides with a different text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Both versions stay active.
    KeepBoth,
    /// The target's version stays; the other is not taken.
    KeepMine,
    /// The other's version replaces the target's, which goes to history.
    TakeTheirs,
    /// As `take-theirs`, and the other's dashboard note and guide replace the target's too.
    Replace,
}

impl Policy {
    pub fn word(self) -> &'static str {
        match self {
            Policy::KeepBoth => "keep-both",
            Policy::KeepMine => "keep-mine",
            Policy::TakeTheirs => "take-theirs",
            Policy::Replace => "replace",
        }
    }

    pub fn parse(s: &str) -> Result<Policy> {
        match s {
            "keep-both" => Ok(Policy::KeepBoth),
            "keep-mine" => Ok(Policy::KeepMine),
            "take-theirs" => Ok(Policy::TakeTheirs),
            "replace" => Ok(Policy::Replace),
            _ => Err(Error::Invalid(format!(
                "unknown conflict policy {s:?} (keep-both, keep-mine, take-theirs, replace)"
            ))),
        }
    }
}

/// What a line would do, counted in notes and files of the other memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// Notes not in the target.
    pub new: usize,
    /// Notes already there (same text), or that the target has retired.
    pub already: usize,
    /// Notes in conflict with a different version in the target.
    pub differing: usize,
    /// Notes already there that would gain tags.
    pub retag: usize,
    /// Files not in the target.
    pub files_new: usize,
    /// Files already in the target.
    pub files_already: usize,
}

impl Preview {
    fn to_json(&self) -> Value {
        json!({
            "new": self.new, "already": self.already, "differing": self.differing,
            "retag": self.retag, "files_new": self.files_new, "files_already": self.files_already,
        })
    }

    fn from_json(v: &Value) -> Preview {
        let n = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
        Preview {
            new: n("new"),
            already: n("already"),
            differing: n("differing"),
            retag: n("retag"),
            files_new: n("files_new"),
            files_already: n("files_already"),
        }
    }
}

/// One line of the correspondence table: a space of the other memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The space in the export.
    pub source: String,
    /// Its kind in the export.
    pub kind: String,
    pub action: Action,
    /// The space it joins (`merge`), or the name of the new space (`copy`).
    pub target: String,
    /// For `copy`: another name for the new space, replacing `target`.
    pub rename: Option<String>,
    pub policy: Policy,
    /// Filled in by [`plan`] and [`preview`]; ignored when applying.
    pub preview: Preview,
    /// Why the default was chosen, for the person editing the plan.
    pub note: String,
    /// What the guide of the other space would do: `none`, `same`, `differs` or `new`.
    pub guide: String,
}

impl Line {
    /// The name of the space the line ends in.
    pub fn destination(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.target)
    }

    fn to_json(&self) -> Value {
        json!({
            "source": self.source, "kind": self.kind, "action": self.action.word(),
            "target": self.target, "rename": self.rename, "policy": self.policy.word(),
            "preview": self.preview.to_json(), "guide": self.guide, "note": self.note,
        })
    }

    fn from_json(v: &Value) -> Result<Line> {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Ok(Line {
            source: s("source"),
            kind: s("kind"),
            action: Action::parse(&s("action"))?,
            target: s("target"),
            rename: v["rename"].as_str().map(String::from),
            policy: Policy::parse(&s("policy"))?,
            preview: Preview::from_json(&v["preview"]),
            note: s("note"),
            guide: s("guide"),
        })
    }
}

/// The plan of a merge.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The export directory the other memory is read from.
    pub export: PathBuf,
    /// The person the merged spaces belong to.
    pub user: String,
    pub created_at: String,
    pub lines: Vec<Line>,
}

impl Plan {
    /// The plan as JSON text, one line of the table per line of text.
    pub fn to_text(&self) -> String {
        let lines: Vec<String> = self
            .lines
            .iter()
            .map(|l| format!("    {}", l.to_json()))
            .collect();
        format!(
            "{{\n  \"format\": \"{PLAN_FORMAT}\",\n  \"version\": {PLAN_VERSION},\n  \"export\": {},\n  \"user\": {},\n  \"created_at\": {},\n  \"lines\": [\n{}\n  ]\n}}\n",
            json!(self.export.to_string_lossy()),
            json!(self.user),
            json!(self.created_at),
            lines.join(",\n")
        )
    }

    pub fn from_text(text: &str) -> Result<Plan> {
        let v: Value = serde_json::from_str(text)
            .map_err(|e| Error::Invalid(format!("the plan is not valid JSON: {e}")))?;
        if v["format"] != PLAN_FORMAT {
            return Err(Error::Invalid("this is not a MemCP merge plan".into()));
        }
        if v["version"].as_u64() != Some(u64::from(PLAN_VERSION)) {
            return Err(Error::Invalid(format!(
                "this plan has format version {}, this program reads version {PLAN_VERSION}",
                v["version"]
            )));
        }
        let lines = v["lines"]
            .as_array()
            .ok_or_else(|| Error::Invalid("the plan has no lines".into()))?
            .iter()
            .map(Line::from_json)
            .collect::<Result<Vec<_>>>()?;
        Ok(Plan {
            export: PathBuf::from(v["export"].as_str().unwrap_or_default()),
            user: v["user"].as_str().unwrap_or_default().to_string(),
            created_at: v["created_at"].as_str().unwrap_or_default().to_string(),
            lines,
        })
    }
}

// --- the other memory ---------------------------------------------------------------------------

/// A space of the export, copied next to the data, brought to the current schema, read only by us.
struct Source {
    conn: Connection,
    path: PathBuf,
}

impl Drop for Source {
    fn drop(&mut self) {
        let p = self.path.display();
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(format!("{p}-wal"));
        let _ = fs::remove_file(format!("{p}-shm"));
    }
}

fn open_source(brain: &Brain, export: &Path, name: &str) -> Result<Source> {
    Space::new(name)?;
    let tmp = brain.dir().join(format!(
        ".merge-{name}-{}-{}.sqlite",
        std::process::id(),
        util::random_base32(6)
    ));
    fs::copy(export.join(format!("vaults/{name}.sqlite")), &tmp)?;
    crate::fsx::set_private_file(&tmp)?;
    let source = Source {
        conn: brain::open_db(&tmp)?,
        path: tmp,
    };
    brain::migrate(&source.conn, SPACE_MIGRATIONS)?;
    Ok(source)
}

/// The guides in force of the export, by space name.
fn export_guides(export: &Path) -> std::collections::BTreeMap<String, String> {
    let list: Vec<Value> = fs::read_to_string(export.join("guides.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    list.iter()
        .filter(|g| g["current"] == true)
        .filter_map(|g| {
            let name = g["scope"].as_str()?.strip_prefix("space:")?;
            Some((name.to_string(), g["body"].as_str()?.to_string()))
        })
        .collect()
}

/// A theirs file row.
struct TheirFile {
    name: String,
    mime: String,
    size: i64,
    sha256: String,
    note: String,
    fact_id: Option<String>,
    author: String,
    created_at: i64,
}

fn their_files(src: &Connection) -> Result<Vec<TheirFile>> {
    let mut st = src.prepare(
        "SELECT id, name, mime, size, sha256, note, fact_id, author, created_at FROM files
         WHERE status = 'active' ORDER BY created_at, id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(TheirFile {
            name: r.get(1)?,
            mime: r.get(2)?,
            size: r.get(3)?,
            sha256: r.get(4)?,
            note: r.get(5)?,
            fact_id: r.get(6)?,
            author: r.get(7)?,
            created_at: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The active notes of the other memory that are not file descriptions.
fn their_facts(src: &Connection) -> Result<Vec<Fact>> {
    let ids: Vec<String> = {
        let mut st = src.prepare(
            "SELECT id FROM facts WHERE status = 'active' AND coalesce(type, '') <> 'file'
             ORDER BY created_at, id",
        )?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut out = vec![];
    for id in ids {
        if let Some(f) = facts::read_fact(src, &id)? {
            out.push(f);
        }
    }
    Ok(out)
}

// --- comparing ------------------------------------------------------------------------------------

/// What one note of the other memory is, next to the target.
enum Item {
    New(Fact),
    /// The same text is active in the target; `add_tags` are the tags it lacks.
    Already {
        mine: String,
        add_tags: Vec<String>,
    },
    /// Another version of the note is active in the target.
    Conflict {
        mine: String,
        theirs: Fact,
        dashboard: bool,
    },
    /// The target retired or replaced it: its decision stands.
    Settled,
}

impl Item {
    /// Whether applying under `policy` writes something.
    fn acts(&self, policy: Policy) -> bool {
        match self {
            Item::New(_) => true,
            Item::Already { add_tags, .. } => !add_tags.is_empty(),
            Item::Conflict {
                dashboard: true, ..
            } => policy == Policy::Replace,
            Item::Conflict { .. } => policy != Policy::KeepMine,
            Item::Settled => false,
        }
    }
}

fn active_dashboard(dst: &Connection) -> Result<Option<String>> {
    Ok(dst
        .query_row(
            "SELECT f.id FROM facts f JOIN fact_tags t ON t.fact_id = f.id
             WHERE f.status = 'active' AND t.tag = 'dashboard' ORDER BY f.created_at, f.id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

fn classify(dst: Option<&Connection>, f: Fact) -> Result<Item> {
    let Some(dst) = dst else {
        return Ok(Item::New(f));
    };
    if let Some(m) = facts::active_by_hash(dst, &text::text_hash(&f.text))? {
        let m = facts::read_fact(dst, &m.id)?.unwrap_or(m);
        let add_tags = f
            .tags
            .iter()
            .filter(|t| !m.tags.contains(t))
            .cloned()
            .collect();
        return Ok(Item::Already {
            mine: m.id,
            add_tags,
        });
    }
    let dashboard = f.tags.iter().any(|t| t == "dashboard");
    if let Some(m) = facts::read_fact(dst, &f.id)? {
        return Ok(if m.status == "active" {
            Item::Conflict {
                mine: m.id,
                theirs: f,
                dashboard,
            }
        } else {
            Item::Settled
        });
    }
    if dashboard && let Some(mine) = active_dashboard(dst)? {
        return Ok(Item::Conflict {
            mine,
            theirs: f,
            dashboard,
        });
    }
    Ok(Item::New(f))
}

fn file_present(dst: &Connection, f: &TheirFile) -> Result<bool> {
    Ok(dst
        .query_row(
            "SELECT 1 FROM files WHERE sha256 = ?1 AND name = ?2 AND status = 'active' LIMIT 1",
            params![f.sha256, f.name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Counts what a line would do, and how many writes that is.
fn survey(src: &Connection, dst: Option<&Connection>, policy: Policy) -> Result<(Preview, usize)> {
    let mut p = Preview::default();
    let mut pending = 0;
    for f in their_facts(src)? {
        let item = classify(dst, f)?;
        if item.acts(policy) {
            pending += 1;
        }
        match &item {
            Item::New(_) => p.new += 1,
            Item::Already { add_tags, .. } => {
                p.already += 1;
                if !add_tags.is_empty() {
                    p.retag += 1;
                }
            }
            Item::Conflict { .. } => p.differing += 1,
            Item::Settled => p.already += 1,
        }
    }
    for f in their_files(src)? {
        if dst
            .map(|d| file_present(d, &f))
            .transpose()?
            .unwrap_or(false)
        {
            p.files_already += 1;
        } else {
            p.files_new += 1;
            pending += 1;
        }
    }
    Ok((p, pending))
}

// --- the plan ---------------------------------------------------------------------------------------

/// The spaces a person can write to, with their kind.
fn writable(brain: &Brain, ctl: &Connection, user: &User) -> Result<Vec<(Space, String)>> {
    let mut st = ctl.prepare(
        "SELECT s.name, s.kind FROM spaces s JOIN space_members m ON m.space = s.name
         WHERE m.user_id = ?1 AND (m.can_write = 1 OR m.owner = 1) ORDER BY s.name",
    )?;
    let rows = st.query_map([&user.id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = vec![];
    for row in rows {
        let (name, kind) = row?;
        if brain.has_space(&Space::new(&name)?)? {
            out.push((Space::new(&name)?, kind));
        }
    }
    Ok(out)
}

fn can_write(brain: &Brain, ctl: &Connection, user: &User, space: &Space) -> Result<bool> {
    if !brain.has_space(space)? {
        return Ok(false);
    }
    if user.admin {
        return Ok(true);
    }
    Ok(writable(brain, ctl, user)?.iter().any(|(s, _)| s == space))
}

fn kind_or_custom(brain: &Brain, kind: &str) -> Result<String> {
    Ok(if brain.catalogue()?.iter().any(|k| k.key == kind) {
        kind.to_string()
    } else {
        crate::catalogue::CUSTOM.to_string()
    })
}

/// Reads an export and proposes, for each of its spaces, what to do. Writes nothing.
///
/// Defaults: same name and same kind, merge; same kind and another name, merge into the person's
/// only space of that kind; otherwise copy as a new space (under another name if the name is
/// taken). The conflict policy is `policy` on every line.
pub fn plan(brain: &Brain, export_dir: &Path, user: &User, policy: Policy) -> Result<Plan> {
    let manifest = export::verify_export(export_dir)?;
    let ctl = brain.open_control()?;
    let mine = writable(brain, &ctl, user)?;
    let vaults = manifest["vaults"]
        .as_array()
        .ok_or_else(|| Error::Invalid("the manifest lists no vault".into()))?;
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut lines = vec![];
    for v in vaults {
        let source = v["name"].as_str().unwrap_or_default().to_string();
        Space::new(&source)?;
        let kind = kind_or_custom(brain, v["kind"].as_str().unwrap_or("custom"))?;
        let same_name = mine.iter().find(|(s, _)| s.as_str() == source);
        let same_kind: Vec<&(Space, String)> = mine
            .iter()
            .filter(|(s, k)| *k == kind && !taken.contains(s.as_str()))
            .collect();
        let (action, target, note) = match same_name {
            Some((s, k)) if *k == kind => (
                Action::Merge,
                s.to_string(),
                "same name and same kind".to_string(),
            ),
            _ if kind != crate::catalogue::CUSTOM && same_kind.len() == 1 => (
                Action::Merge,
                same_kind[0].0.to_string(),
                format!("the only space of kind {kind}: check that this is the one"),
            ),
            _ => {
                let mut name = source.clone();
                let mut n = 1;
                while brain.has_space(&Space::new(&name)?)?
                    || brain.dir().join(Space::new(&name)?.file_name()).exists()
                    || taken.contains(&name)
                {
                    n += 1;
                    name = format!("{source}-{n}");
                }
                let why = if name == source {
                    "no space of this name or kind".to_string()
                } else {
                    format!("the name {source} is taken by another kind")
                };
                (Action::Copy, name, why)
            }
        };
        taken.insert(target.clone());
        lines.push(Line {
            source,
            kind,
            action,
            target,
            rename: None,
            policy,
            preview: Preview::default(),
            note,
            guide: "none".into(),
        });
    }
    let mut plan = Plan {
        export: crate::fsx::canonical(export_dir)?,
        user: user.name.clone(),
        created_at: util::iso(util::now()),
        lines,
    };
    preview(brain, &mut plan)?;
    Ok(plan)
}

/// Fills in the preview and the guide state of every line, from what is there now. Writes nothing.
pub fn preview(brain: &Brain, plan: &mut Plan) -> Result<()> {
    export::verify_export(&plan.export)?;
    let guides = export_guides(&plan.export);
    for line in &mut plan.lines {
        if line.action == Action::Skip {
            line.preview = Preview::default();
            continue;
        }
        let src = open_source(brain, &plan.export, &line.source)?;
        let dest = Space::new(line.destination())?;
        let conn = if brain.has_space(&dest)? {
            Some(brain.open_space(&dest)?)
        } else {
            None
        };
        line.preview = survey(&src.conn, conn.as_ref(), line.policy)?.0;
        line.guide = match (guides.get(&line.source), conn.is_some()) {
            (None, _) => "none".into(),
            (Some(_), false) => "new".into(),
            (Some(theirs), true) => {
                let ctl = brain.open_control()?;
                match guide::current(&ctl, &guide::space_scope(&dest))? {
                    None => "new".into(),
                    Some(g) if g.body == theirs.trim() => "same".into(),
                    Some(_) => "differs".into(),
                }
            }
        };
    }
    Ok(())
}

// --- applying -----------------------------------------------------------------------------------------

/// How to apply.
#[derive(Clone, Debug)]
pub struct Options {
    /// Where the backup is made (outside the data directory).
    pub backup_to: PathBuf,
    /// The time of the backup, in Unix seconds.
    pub at: i64,
    /// For tests: fail, after this many lines have been applied, as an interruption would.
    pub stop_after: Option<usize>,
}

/// What a line did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineReport {
    pub source: String,
    pub destination: String,
    /// The space was created by this merge.
    pub created: bool,
    /// Notes written (new, or the second version of a conflict kept beside the first).
    pub added: usize,
    /// Notes that gained tags.
    pub tagged: usize,
    /// Notes of the target replaced by the other's version (they stay in history).
    pub replaced: usize,
    /// Conflicts left as they were.
    pub left: usize,
    pub files: usize,
    pub guide_replaced: bool,
}

/// What an application did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Where the backup is; `None` when there was nothing to change.
    pub backup: Option<PathBuf>,
    pub lines: Vec<LineReport>,
    /// The guides of the other memory that were not applied: space, text.
    pub guide_proposals: Vec<(String, String)>,
}

impl Report {
    pub fn changed(&self) -> bool {
        self.backup.is_some()
    }
}

/// A line, checked and resolved.
struct Resolved<'a> {
    line: &'a Line,
    dest: Space,
    /// The space is to be created.
    create: bool,
    kind: String,
}

fn resolve<'a>(
    brain: &Brain,
    ctl: &Connection,
    user: &User,
    manifest: &Value,
    plan: &'a Plan,
) -> Result<Vec<Resolved<'a>>> {
    let known: Vec<&str> = manifest["vaults"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    let mut sources = BTreeSet::new();
    let mut created = BTreeSet::new();
    let mut out = vec![];
    for line in &plan.lines {
        if !known.contains(&line.source.as_str()) {
            return Err(Error::Invalid(format!(
                "the plan names {:?}, which is not in the export",
                line.source
            )));
        }
        if !sources.insert(line.source.clone()) {
            return Err(Error::Invalid(format!(
                "{:?} appears on two lines of the plan",
                line.source
            )));
        }
        if line.action == Action::Skip {
            continue;
        }
        if line.action == Action::Merge && line.rename.is_some() {
            return Err(Error::Invalid(format!(
                "{}: a merge cannot rename the space it joins",
                line.source
            )));
        }
        let dest = Space::new(line.destination())?;
        let exists = brain.has_space(&dest)?;
        let create = match line.action {
            Action::Merge => {
                if !exists {
                    return Err(Error::Invalid(format!(
                        "{}: there is no space {dest} to merge into",
                        line.source
                    )));
                }
                false
            }
            _ => !exists,
        };
        if exists && !can_write(brain, ctl, user, &dest)? {
            return Err(Error::Invalid(format!(
                "{} cannot write to {dest}",
                user.name
            )));
        }
        if create {
            if brain.dir().join(dest.file_name()).exists() {
                return Err(Error::Invalid(format!("{dest} is in the way")));
            }
            if !created.insert(dest.to_string()) {
                return Err(Error::Invalid(format!(
                    "{dest} would be created by two lines"
                )));
            }
        }
        out.push(Resolved {
            line,
            dest,
            create,
            kind: kind_or_custom(brain, &line.kind)?,
        });
    }
    Ok(out)
}

/// What to undo if the merge fails before it is committed.
#[derive(Default)]
struct Undo {
    spaces: Vec<Space>,
    files: Vec<PathBuf>,
}

impl Undo {
    fn run(&self, brain: &Brain) {
        for p in self.files.iter().rev() {
            let _ = fs::remove_file(p);
        }
        if let Ok(ctl) = brain.open_control() {
            for s in &self.spaces {
                let _ = ctl.execute("DELETE FROM space_members WHERE space = ?1", [s.as_str()]);
                let _ = ctl.execute(
                    "DELETE FROM guides WHERE scope = ?1",
                    [guide::space_scope(s)],
                );
                let _ = ctl.execute("DELETE FROM spaces WHERE name = ?1", [s.as_str()]);
            }
        }
        for s in &self.spaces {
            let path = brain.dir().join(s.file_name());
            let _ = fs::remove_file(format!("{}-wal", path.display()));
            let _ = fs::remove_file(format!("{}-shm", path.display()));
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir_all(brain.dir().join("files").join(s.as_str()));
        }
    }
}

fn agent_of(f: &Fact) -> String {
    f.source_agent
        .clone()
        .unwrap_or_else(|| "agent:merge".to_string())
}

/// Writes a copy of one of their notes, with its tags, dates and author, in the caller's
/// transaction. `replaces` links it to the note it supersedes.
fn insert_copy(
    dst: &Connection,
    space: &Space,
    f: &Fact,
    tags: &[String],
    replaces: Option<&str>,
) -> Result<String> {
    let mut new = NewFact::agent(&f.text, &agent_of(f));
    new.kind = f.kind.clone();
    new.valid_from = Some(f.valid_from);
    new.valid_to = f.valid_to;
    new.tags = facts::normalize_tags(tags)?;
    let text = facts::check_text(&f.text)?;
    let id = facts::insert(dst, space, &f.author, &new, &text, replaces)?.id;
    dst.execute(
        "UPDATE facts SET created_at = ?2 WHERE id = ?1",
        params![id, f.created_at],
    )?;
    Ok(id)
}

fn merge_event(dst: &Connection, fact_id: &str, action: &str, detail: Value) -> Result<()> {
    dst.execute(
        "INSERT INTO fact_events (fact_id, at, actor, action, detail) VALUES (?1, ?2, 'merge', ?3, ?4)",
        params![fact_id, util::now(), action, detail.to_string()],
    )?;
    Ok(())
}

/// The other's version replaces the target's: the old one is marked replaced, its text stays.
fn replace_note(dst: &Connection, space: &Space, mine: &str, theirs: &Fact) -> Result<()> {
    let old = facts::read_fact(dst, mine)?.ok_or_else(|| Error::UnknownFact(mine.into()))?;
    let mut tags = old.tags.clone();
    tags.extend(
        theirs
            .tags
            .iter()
            .filter(|t| !old.tags.contains(t))
            .cloned(),
    );
    dst.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    let changed = dst.execute(
        "UPDATE facts SET status = 'replaced', replaced_by = ?1, status_at = ?2, status_by = 'merge',
                status_reason = 'replaced by the version of another memory'
         WHERE id = ?1 AND status = 'active'",
        params![mine, util::now()],
    )?;
    if changed != 1 {
        return Err(Error::Invalid(format!("fact {mine} is no longer active")));
    }
    let id = insert_copy(dst, space, theirs, &tags, Some(mine))?;
    dst.execute(
        "UPDATE facts SET replaced_by = ?2 WHERE id = ?1",
        params![mine, id],
    )?;
    merge_event(
        dst,
        mine,
        "supersede",
        json!({ "by": id, "reason": "merge" }),
    )?;
    dst.execute_batch("PRAGMA defer_foreign_keys = OFF")?;
    Ok(())
}

fn add_tags(dst: &Connection, fact_id: &str, tags: &[String]) -> Result<()> {
    for tag in facts::normalize_tags(tags)? {
        dst.execute(
            "INSERT OR IGNORE INTO fact_tags (fact_id, tag) VALUES (?1, ?2)",
            params![fact_id, tag],
        )?;
    }
    Ok(())
}

/// Brings a file: its bytes (kept by fingerprint), its row, and the note that describes it.
fn copy_file(
    brain: &Brain,
    blobs: &Path,
    src: &Connection,
    dst: &Connection,
    space: &Space,
    f: &TheirFile,
    undo: &mut Undo,
) -> Result<()> {
    // A fingerprint is 64 hexadecimal digits and nothing else: it names a file under `blobs`, never a path.
    if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid(format!(
            "{} carries a fingerprint that is not one",
            f.name
        )));
    }
    let from = blobs.join(&f.sha256);
    let bytes = fs::read(&from)
        .map_err(|e| Error::Invalid(format!("the bytes of {} cannot be read: {e}", f.name)))?;
    if hex(&Sha256::digest(&bytes)) != f.sha256 {
        return Err(Error::Schema(format!(
            "the bytes of {} do not match their fingerprint",
            f.name
        )));
    }
    let dir = brain.dir().join("files").join(space.as_str());
    fs::DirBuilder::new()
        .recursive(true)
        .private_dir()
        .create(&dir)?;
    let path = dir.join(&f.sha256);
    if !path.exists() {
        let tmp = dir.join(format!(".{}.tmp", f.sha256));
        fs::write(&tmp, &bytes)?;
        crate::fsx::set_private_file(&tmp)?;
        undo.files.push(path.clone());
        fs::rename(&tmp, &path)?;
    }
    let described = match &f.fact_id {
        Some(id) => facts::read_fact(src, id)?,
        None => None,
    };
    let (mut text, tags, author, created) = match &described {
        Some(d) => (
            d.text.clone(),
            d.tags.clone(),
            d.author.clone(),
            d.created_at,
        ),
        None => (
            if f.note.is_empty() {
                format!("File: {}", f.name)
            } else {
                format!("File: {} — {}", f.name, f.note)
            },
            vec![],
            f.author.clone(),
            f.created_at,
        ),
    };
    // One text is active once: the same description for other bytes is told apart by the fingerprint.
    if facts::active_by_hash(dst, &text::text_hash(&text))?.is_some() {
        text = format!("{text} [{}]", &f.sha256[..8]);
    }
    let id = format!("file_{}", util::random_base32(10));
    let mut new = NewFact::agent(&text, "agent:merge");
    new.kind = "context".into();
    new.tags = facts::normalize_tags(&tags)?;
    let checked = facts::check_text(&text)?;
    let fact = facts::insert(dst, space, &author, &new, &checked, None)?;
    dst.execute(
        "UPDATE facts SET type = 'file', fields = ?2, created_at = ?3 WHERE id = ?1",
        params![fact.id, json!({ "file_id": id }).to_string(), created],
    )?;
    dst.execute(
        "INSERT INTO files (id, name, mime, size, sha256, note, fact_id, author, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            f.name,
            f.mime,
            f.size,
            f.sha256,
            f.note,
            fact.id,
            f.author,
            f.created_at
        ],
    )?;
    Ok(())
}

fn count(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

/// Applies a plan: backup, then every line in one transaction per target space, then the
/// verification of the counts, then the commit. Any failure before the commit changes nothing.
/// A plan with nothing to change makes no backup and writes nothing.
pub fn apply(brain: &Brain, plan: &Plan, opts: &Options) -> Result<Report> {
    let manifest = export::verify_export(&plan.export)?;
    let ctl = brain.open_control()?;
    let user = users::resolve(&ctl, &plan.user)?;
    if user.disabled {
        return Err(Error::Invalid(format!("{} is disabled", user.name)));
    }
    let resolved = resolve(brain, &ctl, &user, &manifest, plan)?;

    // Is there anything to do? Then the backup comes first.
    let mut pending = 0;
    for r in &resolved {
        if r.create {
            pending += 1;
            continue;
        }
        let src = open_source(brain, &plan.export, &r.line.source)?;
        let dst = brain.open_space(&r.dest)?;
        pending += survey(&src.conn, Some(&dst), r.line.policy)?.1;
        if let Some(theirs) = export_guides(&plan.export).get(&r.line.source)
            && r.line.policy == Policy::Replace
            && guide::current(&ctl, &guide::space_scope(&r.dest))?
                .is_some_and(|g| g.body != theirs.trim())
        {
            pending += 1;
        }
    }
    if pending == 0 {
        return Ok(Report {
            guide_proposals: proposals(&plan.export, &resolved, &ctl)?,
            ..Report::default()
        });
    }
    let backup = brain.backup(&opts.backup_to, usize::MAX, opts.at)?;

    let mut undo = Undo::default();
    match run(brain, &ctl, &user, plan, &resolved, opts, &mut undo) {
        Ok(mut report) => {
            report.backup = Some(backup);
            Ok(report)
        }
        Err(e) => {
            undo.run(brain);
            Err(match e {
                Error::Invalid(m) => Error::Invalid(format!(
                    "{m} (nothing was changed; the backup is in {})",
                    backup.display()
                )),
                other => other,
            })
        }
    }
}

fn proposals(
    export_dir: &Path,
    resolved: &[Resolved<'_>],
    ctl: &Connection,
) -> Result<Vec<(String, String)>> {
    let guides = export_guides(export_dir);
    let mut out = vec![];
    for r in resolved {
        if r.create {
            continue;
        }
        if let Some(theirs) = guides.get(&r.line.source) {
            let mine = guide::current(ctl, &guide::space_scope(&r.dest))?;
            if mine.is_none_or(|g| g.body != theirs.trim()) && r.line.policy != Policy::Replace {
                out.push((r.dest.to_string(), theirs.clone()));
            }
        }
    }
    Ok(out)
}

fn run(
    brain: &Brain,
    ctl: &Connection,
    user: &User,
    plan: &Plan,
    resolved: &[Resolved<'_>],
    opts: &Options,
    undo: &mut Undo,
) -> Result<Report> {
    // New spaces first: they are registered, and undone if anything fails.
    for r in resolved.iter().filter(|r| r.create) {
        brain.add_space_for(&r.dest, &user.id)?;
        undo.spaces.push(r.dest.clone());
        brain.set_space_kind(&r.dest, &r.kind)?;
    }
    let mut dests: Vec<Space> = vec![];
    for r in resolved {
        if !dests.contains(&r.dest) {
            dests.push(r.dest.clone());
        }
    }
    let conns: Vec<Connection> = dests
        .iter()
        .map(|d| brain.open_space(d))
        .collect::<Result<_>>()?;
    let txs: Vec<rusqlite::Transaction<'_>> = conns
        .iter()
        .map(|c| {
            rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)
                .map_err(Error::from)
        })
        .collect::<Result<_>>()?;
    let before: Vec<(i64, i64)> = txs
        .iter()
        .map(|t| {
            Ok((
                count(t, "SELECT count(*) FROM facts WHERE status = 'active'")?,
                count(t, "SELECT count(*) FROM files WHERE status = 'active'")?,
            ))
        })
        .collect::<Result<_>>()?;
    let mut delta = vec![(0i64, 0i64); dests.len()];

    let guides = export_guides(&plan.export);
    let mut report = Report::default();
    let mut sources = vec![];
    for (n, r) in resolved.iter().enumerate() {
        let at = dests.iter().position(|d| d == &r.dest).unwrap_or(0);
        let dst: &Connection = &txs[at];
        let src = open_source(brain, &plan.export, &r.line.source)?;
        let mut lr = LineReport {
            source: r.line.source.clone(),
            destination: r.dest.to_string(),
            created: r.create,
            ..LineReport::default()
        };
        let policy = r.line.policy;
        for f in their_facts(&src.conn)? {
            match classify(Some(dst), f)? {
                Item::New(f) => {
                    insert_copy(dst, &r.dest, &f, &f.tags, None)?;
                    lr.added += 1;
                    delta[at].0 += 1;
                }
                Item::Already {
                    mine,
                    add_tags: tags,
                } => {
                    if !tags.is_empty() {
                        add_tags(dst, &mine, &tags)?;
                        lr.tagged += 1;
                    }
                }
                Item::Settled => {}
                Item::Conflict {
                    mine,
                    theirs,
                    dashboard,
                } => {
                    if dashboard {
                        if policy == Policy::Replace {
                            replace_note(dst, &r.dest, &mine, &theirs)?;
                            lr.replaced += 1;
                        } else {
                            lr.left += 1;
                        }
                    } else {
                        match policy {
                            Policy::KeepBoth => {
                                insert_copy(dst, &r.dest, &theirs, &theirs.tags, None)?;
                                lr.added += 1;
                                delta[at].0 += 1;
                            }
                            Policy::KeepMine => lr.left += 1,
                            Policy::TakeTheirs | Policy::Replace => {
                                replace_note(dst, &r.dest, &mine, &theirs)?;
                                lr.replaced += 1;
                            }
                        }
                    }
                }
            }
        }
        for f in their_files(&src.conn)? {
            if !file_present(dst, &f)? {
                copy_file(
                    brain,
                    &plan.export.join("files").join(&r.line.source),
                    &src.conn,
                    dst,
                    &r.dest,
                    &f,
                    undo,
                )?;
                lr.files += 1;
                delta[at].0 += 1;
                delta[at].1 += 1;
            }
        }
        sources.push(src);
        report.lines.push(lr);
        if opts.stop_after == Some(n + 1) {
            return Err(Error::Invalid("interrupted".into()));
        }
    }
    // Verify before committing: counts, links, and that nothing is left to do.
    for (i, tx) in txs.iter().enumerate() {
        let facts_now = count(tx, "SELECT count(*) FROM facts WHERE status = 'active'")?;
        let files_now = count(tx, "SELECT count(*) FROM files WHERE status = 'active'")?;
        if facts_now != before[i].0 + delta[i].0 || files_now != before[i].1 + delta[i].1 {
            return Err(Error::Schema(format!(
                "{}: the counts after merging do not match (notes {facts_now}, files {files_now})",
                dests[i]
            )));
        }
        let broken: i64 = count(tx, "SELECT count(*) FROM pragma_foreign_key_check")?;
        if broken != 0 {
            return Err(Error::Schema(format!("{}: broken links", dests[i])));
        }
    }
    for (n, r) in resolved.iter().enumerate() {
        let at = dests.iter().position(|d| d == &r.dest).unwrap_or(0);
        let left = survey(&sources[n].conn, Some(&*txs[at]), r.line.policy)?.1;
        if left != 0 {
            return Err(Error::Schema(format!(
                "{}: {left} change(s) still pending after merging",
                r.dest
            )));
        }
    }
    // Guides: the target's stay unless `replace`; a new space takes the other's.
    for (n, r) in resolved.iter().enumerate() {
        let Some(theirs) = guides.get(&r.line.source) else {
            continue;
        };
        let scope = guide::space_scope(&r.dest);
        let mine = guide::current(ctl, &scope)?;
        let differs = mine.as_ref().is_none_or(|g| g.body != theirs.trim());
        if differs && (r.create || (r.line.policy == Policy::Replace && mine.is_some())) {
            guide::set(ctl, &user.id, &scope, theirs)?;
            report.lines[n].guide_replaced = true;
        } else if differs {
            report
                .guide_proposals
                .push((r.dest.to_string(), theirs.clone()));
        }
    }
    for tx in txs {
        tx.commit()?;
    }
    Ok(report)
}
