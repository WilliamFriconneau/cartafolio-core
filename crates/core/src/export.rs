//! Export and adopt: a person's memory leaves as a directory and can be taken in anywhere.
//!
//! An export holds, for each vault the person **owns**: a consistent copy of its database,
//! its files, and a readable Markdown version of its facts with their history. It holds the
//! person's instructions (every version), and a manifest with the fingerprint of every file.
//! It never holds a key, a connection, a right or a sign-in link: those belong to an
//! installation, not to a person's memory. A vault shared with the person by someone else
//! is not theirs to take.
//!
//! Adopting an export checks every fingerprint first, refuses before it changes anything if
//! a name is taken, checks the counts of each vault after copying it, and undoes everything
//! if any step fails.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::{self, Brain};
use crate::error::{Error, Result};
use crate::fsx::PrivateDir as _;
use crate::guide;
use crate::schema::SPACE_MIGRATIONS;
use crate::space::Space;
use crate::text::hex;
use crate::users::User;
use crate::util;

/// Value of the `format` field of an export manifest.
pub const FORMAT: &str = "memcp-export";
/// What the format was called before the project was renamed: exports already made are still read.
const FORMER_FORMAT: &str = "humanmemcp-export";
/// Version of the export format this program writes; a newer manifest is refused.
pub const VERSION: u32 = 1;

/// The folder of an export that holds one database per space (an export made before the rename calls it `vaults`).
const SPACES_DIR: &str = "spaces";

/// The spaces an export lists: the manifest's `spaces`, or `vaults` in an export made by an older program.
pub fn manifest_spaces(manifest: &Value) -> Option<&Vec<Value>> {
    manifest["spaces"]
        .as_array()
        .or_else(|| manifest["vaults"].as_array())
}

/// Where an export keeps the database of `name`: `spaces/`, or `vaults/` in an export made by an older program.
pub fn space_file(dir: &Path, name: &str) -> PathBuf {
    let new = dir.join(format!("{SPACES_DIR}/{name}.sqlite"));
    if new.is_file() {
        new
    } else {
        dir.join(format!("vaults/{name}.sqlite"))
    }
}

fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .private_dir()
        .create(path)?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    fs::write(path, bytes)?;
    crate::fsx::set_private_file(path)?;
    Ok(())
}

fn sha_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Statuses of the facts of a vault, as a JSON object (`{"active": 3, "replaced": 1}`).
fn fact_counts(conn: &Connection) -> Result<Value> {
    let mut st =
        conn.prepare("SELECT status, count(*) FROM facts GROUP BY status ORDER BY status")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut m = serde_json::Map::new();
    for row in rows {
        let (s, n) = row?;
        m.insert(s, json!(n));
    }
    Ok(Value::Object(m))
}

fn count(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

/// The vaults a person owns: name and kind.
fn owned(ctl: &Connection, user_id: &str) -> Result<Vec<(Space, String)>> {
    let mut st = ctl.prepare(
        "SELECT s.name, s.kind FROM spaces s JOIN space_members m ON m.space = s.name
         WHERE m.user_id = ?1 AND m.owner = 1 ORDER BY s.name",
    )?;
    let rows = st.query_map([user_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = vec![];
    for row in rows {
        let (name, kind) = row?;
        out.push((Space::new(&name)?, kind));
    }
    Ok(out)
}

/// A date, `2026-11-01`, from Unix seconds.
fn day(t: i64) -> String {
    util::iso(t).chars().take(10).collect()
}

/// The facts of a vault as a document a person can read without any program: what is in
/// force, then what was replaced or retired, with the reason.
fn readable(conn: &Connection, space: &Space, kind: &str, who: &User, at: i64) -> Result<String> {
    let mut out = format!(
        "# {space} ({kind})\n\nThe memory of {} in this space, exported on {}. A fact is never edited: a correction replaces the earlier text, which stays below.\n",
        if who.display.is_empty() {
            &who.name
        } else {
            &who.display
        },
        day(at)
    );
    let mut st = conn.prepare(
        "SELECT f.id, f.text, f.status, f.created_at, f.valid_from, f.valid_to, f.source_kind,
                coalesce(f.source_path, ''), coalesce(f.source_agent, ''), coalesce(f.replaced_by, ''),
                coalesce(f.status_reason, ''), f.status_at,
                coalesce((SELECT group_concat(tag, ', ') FROM (SELECT tag FROM fact_tags t WHERE t.fact_id = f.id ORDER BY tag)), '')
         FROM facts f ORDER BY f.created_at, f.id",
    )?;
    struct Row {
        id: String,
        text: String,
        status: String,
        created: i64,
        valid_to: Option<i64>,
        source: String,
        replaced_by: String,
        reason: String,
        status_at: Option<i64>,
        tags: String,
    }
    let rows = st.query_map([], |r| {
        let kind: String = r.get(6)?;
        let path: String = r.get(7)?;
        let agent: String = r.get(8)?;
        Ok(Row {
            id: r.get(0)?,
            text: r.get(1)?,
            status: r.get(2)?,
            created: r.get(3)?,
            valid_to: r.get(5)?,
            source: if kind == "note" { path } else { agent },
            replaced_by: r.get(9)?,
            reason: r.get(10)?,
            status_at: r.get(11)?,
            tags: r.get(12)?,
        })
    })?;
    let all: Vec<Row> = rows.collect::<rusqlite::Result<_>>()?;
    let entry = |f: &Row| -> String {
        let mut head = format!("- {}", day(f.created));
        if !f.tags.is_empty() {
            head.push_str(&format!(" · {}", f.tags));
        }
        if !f.source.is_empty() {
            head.push_str(&format!(" · {}", f.source));
        }
        head.push_str(&format!(" · {}", f.id));
        if let Some(t) = f.valid_to {
            head.push_str(&format!(" · valid until {}", day(t)));
        }
        let body = f.text.lines().collect::<Vec<_>>().join("\n  ");
        format!("{head}\n  {body}\n")
    };
    let live: Vec<&Row> = all
        .iter()
        .filter(|f| f.status == "active" || f.status == "proposed")
        .collect();
    out.push_str(&format!("\n## In force ({})\n\n", live.len()));
    for f in &live {
        out.push_str(&entry(f));
    }
    let past: Vec<&Row> = all
        .iter()
        .filter(|f| f.status == "replaced" || f.status == "retired")
        .collect();
    out.push_str(&format!("\n## Replaced or retired ({})\n\n", past.len()));
    for f in &past {
        let mut line = entry(f);
        let when = f.status_at.map(day).unwrap_or_default();
        let how = if f.status == "replaced" {
            format!("  Replaced on {when} by {}", f.replaced_by)
        } else {
            format!("  Retired on {when}")
        };
        line.push_str(&how);
        if !f.reason.is_empty() {
            line.push_str(&format!(": {}", f.reason));
        }
        line.push('\n');
        out.push_str(&line);
    }
    // The health record, when the vault has one.
    let has_record: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'record'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if has_record && count(conn, "SELECT count(*) FROM record")? > 0 {
        out.push_str("\n## Record entries\n\n");
        let mut st = conn.prepare(
            "SELECT kind, label, detail, coalesce(start, ''), coalesce(\"end\", ''), status, verification FROM record ORDER BY created_at, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(format!(
                "- {}: {} ({}, {}{}{}){}\n",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                {
                    let s: String = r.get(3)?;
                    if s.is_empty() {
                        String::new()
                    } else {
                        format!(", from {s}")
                    }
                },
                {
                    let e: String = r.get(4)?;
                    if e.is_empty() {
                        String::new()
                    } else {
                        format!(" to {e}")
                    }
                },
                {
                    let d: String = r.get(2)?;
                    if d.is_empty() {
                        String::new()
                    } else {
                        format!("\n  {}", d.replace('\n', "\n  "))
                    }
                },
            ))
        })?;
        for row in rows {
            out.push_str(&row?);
        }
    }
    Ok(out)
}

/// Every version of the person's instructions and of those of their vaults, as JSON.
fn guides_of(ctl: &Connection, who: &User, vaults: &[(Space, String)]) -> Result<Value> {
    let mut scopes = vec![guide::person_scope(&who.id)];
    scopes.extend(vaults.iter().map(|(s, _)| guide::space_scope(s)));
    let mut out = vec![];
    for scope in scopes {
        for g in guide::history(ctl, &scope)? {
            let current = guide::current(ctl, &scope)?
                .is_some_and(|c| c.created_at == g.created_at && c.body == g.body);
            let scope_name = if scope.starts_with("person:") {
                "person".to_string()
            } else {
                scope.clone()
            };
            out.push(json!({
                "scope": scope_name, "body": g.body, "written_at": util::iso(g.created_at), "current": current,
            }));
        }
    }
    Ok(Value::Array(out))
}

/// Writes the export of a person into `<to>/memcp-export-<person>-<time>/` and returns
/// that directory. `to` must be outside the data directory.
pub fn export_person(brain: &Brain, who: &User, to: &Path, at: i64) -> Result<PathBuf> {
    let data = crate::fsx::canonical(brain.dir())?;
    if brain::resolves_inside(to, &data)? {
        return Err(Error::Invalid(format!(
            "the export must be outside the data directory ({})",
            data.display()
        )));
    }
    let ctl = brain.open_control()?;
    let vaults = owned(&ctl, &who.id)?;
    if vaults.is_empty() {
        return Err(Error::Invalid(format!(
            "{} owns no space to export",
            who.name
        )));
    }
    export_vaults(brain, who, &vaults, to, "", at)
}

/// A person's whole memory, to a folder the caller chose (inside the data directory is fine). `None` when they own
/// no vault.
pub(crate) fn export_all(brain: &Brain, who: &User, to: &Path, at: i64) -> Result<Option<PathBuf>> {
    let vaults = owned(&brain.open_control()?, &who.id)?;
    if vaults.is_empty() {
        return Ok(None);
    }
    export_vaults(brain, who, &vaults, to, "", at).map(Some)
}

/// The export of one space of a person (what an archive keeps). `to` may be inside the data directory: the caller
/// chose it (the archive folder).
pub(crate) fn export_space(
    brain: &Brain,
    who: &User,
    space: &Space,
    kind: &str,
    to: &Path,
    at: i64,
) -> Result<PathBuf> {
    export_vaults(
        brain,
        who,
        &[(space.clone(), kind.to_string())],
        to,
        &format!("-{space}"),
        at,
    )
}

fn export_vaults(
    brain: &Brain,
    who: &User,
    vaults: &[(Space, String)],
    to: &Path,
    tag: &str,
    at: i64,
) -> Result<PathBuf> {
    let ctl = brain.open_control()?;
    private_dir(to)?;
    let root = crate::fsx::canonical(to)?.join(format!(
        "memcp-export-{}{tag}-{}",
        who.name,
        util::compact(at)
    ));
    fs::DirBuilder::new()
        .private_dir()
        .create(&root)
        .map_err(|e| Error::Invalid(format!("{}: {e}", root.display())))?;

    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    let mut entries = vec![];
    for (space, kind) in vaults {
        let conn = brain.open_space(space)?;
        let rel = format!("{SPACES_DIR}/{space}.sqlite");
        let target = root.join(&rel);
        private_dir(&root.join(SPACES_DIR))?;
        conn.execute("VACUUM INTO ?1", [target.to_string_lossy()])?;
        crate::fsx::set_private_file(&target)?;
        {
            let copy =
                Connection::open_with_flags(&target, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let ok: String = copy.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            if ok != "ok" {
                return Err(Error::Schema(format!(
                    "the copy of {space} is damaged: {ok}"
                )));
            }
        }
        hashes.insert(rel, sha_of(&fs::read(&target)?));

        let mut blobs = 0;
        let blob_dir = brain.dir().join("files").join(space.as_str());
        if blob_dir.is_dir() {
            for entry in fs::read_dir(&blob_dir)? {
                let path = entry?.path();
                if !path.is_file() {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                let bytes = fs::read(&path)?;
                let sha = sha_of(&bytes);
                if sha != name {
                    return Err(Error::Schema(format!(
                        "a file of {space} does not match its name ({name})"
                    )));
                }
                let rel = format!("files/{space}/{name}");
                write_private(&root.join(&rel), &bytes)?;
                hashes.insert(rel, sha);
                blobs += 1;
            }
        }

        let md = readable(&conn, space, kind, who, at)?;
        let rel = format!("readable/{space}.md");
        write_private(&root.join(&rel), md.as_bytes())?;
        hashes.insert(rel, sha_of(md.as_bytes()));

        let stored_label: Option<String> = ctl
            .query_row(
                "SELECT label FROM spaces WHERE name = ?1",
                [space.as_str()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        entries.push(json!({
            "name": space.as_str(), "kind": kind, "label": stored_label,
            "schema": conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
            "facts": fact_counts(&conn)?,
            "files": count(&conn, "SELECT count(*) FROM files")?,
            "blobs": blobs,
        }));
    }

    let g = serde_json::to_string_pretty(&guides_of(&ctl, who, vaults)?).unwrap_or_default();
    write_private(&root.join("guides.json"), g.as_bytes())?;
    hashes.insert("guides.json".into(), sha_of(g.as_bytes()));

    let readme = "This is the memory of one person, taken out of MemCP.\n\n\
        readable/   each vault as a Markdown document: the facts in force, then the history.\n\
        spaces/     each space as an SQLite database, complete, to be adopted by another MemCP.\n\
        files/      the photos and documents of each vault, named by their fingerprint.\n\
        guides.json the person's standing instructions, every version.\n\
        manifest.json the fingerprint of every file here, and what each vault holds.\n\n\
        There is no key, connection or right in it: those belong to an installation.\n\
        To check it: memcp verify-export <this directory>. To take it in: memcp --data <dir> adopt <this directory> --user <person>.\n";
    write_private(&root.join("README.txt"), readme.as_bytes())?;
    hashes.insert("README.txt".into(), sha_of(readme.as_bytes()));

    let manifest = json!({
        "format": FORMAT, "version": VERSION, "program": env!("CARGO_PKG_VERSION"),
        "created_at": util::iso(at),
        "person": { "name": who.name, "display": who.display },
        "spaces": entries, "hashes": hashes,
    });
    write_private(
        &root.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)
            .unwrap_or_default()
            .as_bytes(),
    )?;
    Ok(root)
}

/// Every file under `dir`, as paths relative to it.
fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(Error::Invalid(format!(
                "{} is a link: an export holds only plain files",
                path.display()
            )));
        }
        if path.is_dir() {
            walk(&path, base, out)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Checks an export: the manifest, the fingerprint of every file, and that nothing else is
/// there. Returns the manifest.
pub fn verify_export(dir: &Path) -> Result<Value> {
    let text = fs::read_to_string(dir.join("manifest.json"))
        .map_err(|e| Error::Invalid(format!("no manifest.json in {}: {e}", dir.display())))?;
    let manifest: Value = serde_json::from_str(&text)
        .map_err(|e| Error::Invalid(format!("manifest.json is not valid: {e}")))?;
    if manifest["format"] != FORMAT && manifest["format"] != FORMER_FORMAT {
        return Err(Error::Invalid("this is not a MemCP export".into()));
    }
    if manifest["version"].as_u64() != Some(u64::from(VERSION)) {
        return Err(Error::Invalid(format!(
            "this export has format version {}, this program reads version {VERSION}",
            manifest["version"]
        )));
    }
    let hashes = manifest["hashes"]
        .as_object()
        .ok_or_else(|| Error::Invalid("the manifest has no fingerprints".into()))?;
    for (rel, want) in hashes {
        if rel.split('/').any(|p| p == ".." || p.is_empty()) || rel.starts_with('/') {
            return Err(Error::Invalid(format!(
                "the manifest names a path outside the export: {rel}"
            )));
        }
        let kind = fs::symlink_metadata(dir.join(rel))
            .map_err(|e| Error::Invalid(format!("{rel} cannot be read: {e}")))?;
        if !kind.file_type().is_file() {
            return Err(Error::Invalid(format!("{rel} is not a plain file")));
        }
        let bytes = fs::read(dir.join(rel))
            .map_err(|e| Error::Invalid(format!("{rel} cannot be read: {e}")))?;
        if Some(sha_of(&bytes).as_str()) != want.as_str() {
            return Err(Error::Invalid(format!(
                "{rel} has been altered: its fingerprint differs"
            )));
        }
    }
    let mut present = vec![];
    walk(dir, dir, &mut present)?;
    for rel in present {
        if rel != "manifest.json" && !hashes.contains_key(&rel) {
            return Err(Error::Invalid(format!(
                "{rel} is in the export but not in its manifest"
            )));
        }
    }
    Ok(manifest)
}

/// What an adoption did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adopted {
    /// Name given to the vault in this installation (with the prefix, if one was asked).
    pub name: String,
    /// Kind of the vault.
    pub kind: String,
    /// Facts taken in (all versions, retired ones included).
    pub facts: i64,
    /// Files taken in.
    pub files: i64,
}

/// Takes the vaults of an export into this installation as the property of `owner`. With a
/// `prefix`, each vault is named `<prefix>-<name>`. Nothing is changed unless the whole export
/// is taken in; a name already in use is refused up front.
pub fn adopt(
    brain: &Brain,
    dir: &Path,
    owner: &User,
    prefix: Option<&str>,
) -> Result<Vec<Adopted>> {
    let manifest = verify_export(dir)?;
    if owner.disabled {
        return Err(Error::Invalid(format!("{} is disabled", owner.name)));
    }
    let vaults = manifest_spaces(&manifest)
        .ok_or_else(|| Error::Invalid("the manifest lists no space".into()))?;
    let ctl = brain.open_control()?;
    let catalogue = brain.catalogue()?;

    struct Plan<'a> {
        orig: String,
        target: Space,
        kind: String,
        label: Option<String>,
        entry: &'a Value,
    }
    let mut plan = vec![];
    let mut taken = vec![];
    for v in vaults {
        let orig = v["name"].as_str().unwrap_or_default().to_string();
        Space::new(&orig)?;
        let target = Space::new(&match prefix {
            Some(p) => format!("{p}-{orig}"),
            None => orig.clone(),
        })?;
        if brain.has_space(&target)?
            || crate::recover::in_flight(brain, &target)?
            || (brain.dir().join(target.file_name()).exists()
                && !crate::recover::clear_orphan(brain, &target)?)
        {
            taken.push(target.to_string());
        }
        let kind = v["kind"].as_str().unwrap_or("custom");
        let kind = if kind == crate::catalogue::FOLDER || catalogue.iter().any(|k| k.key == kind) {
            kind
        } else {
            crate::catalogue::CUSTOM
        };
        let label = v["label"]
            .as_str()
            .map(str::to_string)
            .filter(|l| crate::layout::clean_label(l).is_ok())
            // A prefix keeps the copies apart by name; their label follows, so two vaults of one kind can coexist.
            .or_else(|| prefix.map(|_| crate::layout::pretty_name(target.as_str())));
        plan.push(Plan {
            orig,
            target,
            kind: kind.to_string(),
            label,
            entry: v,
        });
    }
    if !taken.is_empty() {
        return Err(Error::Invalid(format!(
            "already in use here: {}. Adopt with a prefix to give the spaces other names.",
            taken.join(", ")
        )));
    }

    let guides: Vec<Value> = fs::read_to_string(dir.join("guides.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();

    // The journal first: the names being adopted, in the control database. A crash before the registration below
    // leaves files no one registered; the start-up pass removes them (`settle_adoptions`). The sweep waits meanwhile.
    let _busy = crate::recover::busy(brain)?;
    let _writing = crate::recover::writing(brain)?;
    let key = format!("{ADOPTING}{}", util::random_base32(10));
    let names: Vec<&str> = plan.iter().map(|p| p.target.as_str()).collect();
    ctl.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2)",
        rusqlite::params![key, json!(names).to_string()],
    )?;
    let mut made: Vec<PathBuf> = vec![];
    let mut done: Vec<Adopted> = vec![];
    let run = (|| -> Result<()> {
        for p in &plan {
            let tmp = brain.dir().join(format!(
                "{ADOPT_PREFIX}{}-{}.sqlite",
                p.target,
                util::random_base32(8)
            ));
            made.push(tmp.clone());
            fs::copy(space_file(dir, &p.orig), &tmp)?;
            crate::fsx::set_private_file(&tmp)?;
            let (facts, files) = {
                let conn = brain::open_db(&tmp)?;
                if p.orig != p.target.as_str() {
                    conn.execute(
                        "UPDATE meta SET v = ?1 WHERE k = 'space'",
                        [p.target.as_str()],
                    )?;
                }
                brain::migrate(&conn, SPACE_MIGRATIONS)?;
                let declared = brain::meta(&conn, "space")?;
                if declared.as_deref() != Some(p.target.as_str()) {
                    return Err(Error::Schema(format!("{} declares the wrong name", p.orig)));
                }
                let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
                if ok != "ok" {
                    return Err(Error::Schema(format!("{} is damaged: {ok}", p.orig)));
                }
                let facts = fact_counts(&conn)?;
                let files = count(&conn, "SELECT count(*) FROM files")?;
                if facts != p.entry["facts"] || files != p.entry["files"].as_i64().unwrap_or(-1) {
                    return Err(Error::Schema(format!(
                        "{} does not hold what the manifest says (facts {facts}, files {files})",
                        p.orig
                    )));
                }
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
                (
                    facts
                        .as_object()
                        .map(|m| m.values().filter_map(Value::as_i64).sum())
                        .unwrap_or(0),
                    files,
                )
            };
            // Closed: the file is the whole database. Linked in under its name, never over another file.
            fs::File::options().write(true).open(&tmp)?.sync_all()?;
            let final_path = brain.dir().join(p.target.file_name());
            fs::hard_link(&tmp, &final_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Invalid(format!("{} is already in use here", p.target))
                } else {
                    e.into()
                }
            })?;
            made.push(final_path);
            brain::remove_db_files(&tmp);
            crate::fsx::sync_dir(brain.dir())?;

            let blobs = dir.join("files").join(&p.orig);
            if blobs.is_dir() {
                let to = brain.dir().join("files").join(p.target.as_str());
                private_dir(&to)?;
                made.push(to.clone());
                for entry in fs::read_dir(&blobs)? {
                    let path = entry?.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        // Whole or absent under its final name: a torn copy would pass for the file.
                        let part = to.join(format!(".{name}.tmp"));
                        fs::copy(&path, &part)?;
                        crate::fsx::set_private_file(&part)?;
                        fs::File::options().write(true).open(&part)?.sync_all()?;
                        fs::rename(&part, to.join(name))?;
                    }
                }
                crate::fsx::sync_dir(&to)?;
            }
            done.push(Adopted {
                name: p.target.to_string(),
                kind: p.kind.clone(),
                facts,
                files,
            });
        }
        crate::failpoint::hit(crate::failpoint::at::ADOPT_FILES)?;
        // One transaction registers every space, their guides, and closes the journal.
        let tx =
            rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
        for p in &plan {
            crate::layout::check_unique_label(
                &tx,
                &p.target,
                &p.kind,
                p.label.as_deref(),
                &owner.id,
            )?;
            tx.execute(
                "INSERT INTO spaces (name, created_at, kind, label) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![p.target.as_str(), util::now(), p.kind, p.label],
            )?;
            tx.execute(
                "INSERT INTO space_members (space, user_id, can_write, owner) VALUES (?1, ?2, 1, 1)",
                rusqlite::params![p.target.as_str(), owner.id],
            )?;
        }
        // The instructions in force at the time of the export come back, for each vault, and
        // for the person if they have none yet.
        for g in guides.iter().filter(|g| g["current"] == true) {
            let scope = g["scope"].as_str().unwrap_or_default();
            let body = g["body"].as_str().unwrap_or_default();
            let target = if scope == "person" {
                if guide::current(&tx, &guide::person_scope(&owner.id))?.is_some() {
                    continue;
                }
                guide::person_scope(&owner.id)
            } else if let Some(orig) = scope.strip_prefix("space:") {
                match plan.iter().find(|p| p.orig == orig) {
                    Some(p) => guide::space_scope(&p.target),
                    None => continue,
                }
            } else {
                continue;
            };
            guide::set(&tx, &owner.id, &target, body)?;
        }
        tx.execute("DELETE FROM meta WHERE k = ?1", [&key])?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = run {
        if e.to_string().contains("(failpoint)") {
            return Err(e);
        }
        // Undo: nothing of a half-taken export stays.
        for path in made.iter().rev() {
            if path.is_dir() {
                let _ = fs::remove_dir_all(path);
            } else {
                brain::remove_db_files(path);
            }
        }
        let _ = ctl.execute("DELETE FROM meta WHERE k = ?1", [&key]);
        return Err(e);
    }
    Ok(done)
}

/// The prefix of the file a database being adopted is checked under, before it is linked in (swept at start).
pub(crate) const ADOPT_PREFIX: &str = ".adopt-";
/// The control database's journal of an adoption under way: the names it is making.
const ADOPTING: &str = "adopt_pending:";

fn adoptions(ctl: &Connection) -> Result<Vec<(String, Vec<String>)>> {
    let rows: Vec<(String, String)> = ctl
        .prepare("SELECT k, v FROM meta WHERE k LIKE 'adopt_pending:%'")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .map(|(k, v)| {
            let names = serde_json::from_str::<Vec<String>>(&v).unwrap_or_default();
            (k, names)
        })
        .collect())
}

/// Whether an adoption under way is making `space`.
pub(crate) fn adopting(brain: &Brain, space: &Space) -> Result<bool> {
    Ok(adoptions(&brain.open_control()?)?
        .iter()
        .any(|(_, names)| names.iter().any(|n| n == space.as_str())))
}

/// The adoptions a crash cut off are undone (run by [`Brain::recover`]): the files of the names they were making and
/// that no one registered are removed, and the journal goes. Answers how many.
pub(crate) fn settle_adoptions(brain: &Brain) -> Result<usize> {
    let ctl = brain.open_control()?;
    let pending = adoptions(&ctl)?;
    for (key, names) in &pending {
        for n in names {
            let Ok(space) = Space::new(n) else {
                continue;
            };
            if brain.has_space(&space)? {
                continue;
            }
            brain::remove_db_files(&brain.dir().join(space.file_name()));
            let _ = fs::remove_dir_all(brain.dir().join("files").join(space.as_str()));
        }
        crate::fsx::sync_dir(brain.dir())?;
        ctl.execute("DELETE FROM meta WHERE k = ?1", [key])?;
    }
    Ok(pending.len())
}
