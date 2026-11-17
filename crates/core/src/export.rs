//! Export and adopt: a person's memory leaves as a directory and can be taken in anywhere.
//!
//! An export holds, for each space the person **owns**: a consistent copy of its database,
//! its files, and a readable Markdown version of its facts with their history. It holds the
//! person's instructions (every version), and a manifest with the fingerprint of every file.
//! It never holds a key, a connection, a right or a sign-in link: those belong to an
//! installation, not to a person's memory. A space shared with the person by someone else
//! is not theirs to take.
//!
//! Adopting an export checks every fingerprint first, refuses before it changes anything if
//! a name is taken, checks the counts of each space after copying it, and undoes everything
//! if any step fails.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
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

pub const FORMAT: &str = "memcp-export";
/// What the format was called before the project was renamed: exports already made are still read.
const FORMER_FORMAT: &str = "humanmemcp-export";
pub const VERSION: u32 = 1;

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

/// Statuses of the facts of a space, as a JSON object (`{"active": 3, "replaced": 1}`).
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

/// The spaces a person owns: name and kind.
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

/// The facts of a space as a document a person can read without any program: what is in
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
    // The health record, when the space has one.
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

/// Every version of the person's instructions and of those of their spaces, as JSON.
fn guides_of(ctl: &Connection, who: &User, spaces: &[(Space, String)]) -> Result<Value> {
    let mut scopes = vec![guide::person_scope(&who.id)];
    scopes.extend(spaces.iter().map(|(s, _)| guide::space_scope(s)));
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
    let spaces = owned(&ctl, &who.id)?;
    if spaces.is_empty() {
        return Err(Error::Invalid(format!(
            "{} owns no space to export",
            who.name
        )));
    }
    private_dir(to)?;
    let root =
        crate::fsx::canonical(to)?.join(format!("memcp-export-{}-{}", who.name, util::compact(at)));
    fs::DirBuilder::new()
        .private_dir()
        .create(&root)
        .map_err(|e| Error::Invalid(format!("{}: {e}", root.display())))?;

    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    let mut entries = vec![];
    for (space, kind) in &spaces {
        let conn = brain.open_space(space)?;
        let rel = format!("vaults/{space}.sqlite");
        let target = root.join(&rel);
        private_dir(&root.join("vaults"))?;
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

        entries.push(json!({
            "name": space.as_str(), "kind": kind,
            "schema": conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
            "facts": fact_counts(&conn)?,
            "files": count(&conn, "SELECT count(*) FROM files")?,
            "blobs": blobs,
        }));
    }

    let g = serde_json::to_string_pretty(&guides_of(&ctl, who, &spaces)?).unwrap_or_default();
    write_private(&root.join("guides.json"), g.as_bytes())?;
    hashes.insert("guides.json".into(), sha_of(g.as_bytes()));

    let readme = "This is the memory of one person, taken out of MemCP.\n\n\
        readable/   each space as a Markdown document: the facts in force, then the history.\n\
        vaults/     each space as an SQLite database, complete, to be adopted by another MemCP.\n\
        files/      the photos and documents of each space, named by their fingerprint.\n\
        guides.json the person's standing instructions, every version.\n\
        manifest.json the fingerprint of every file here, and what each space holds.\n\n\
        There is no key, connection or right in it: those belong to an installation.\n\
        To check it: memcp verify-export <this directory>. To take it in: memcp --data <dir> adopt <this directory> --user <person>.\n";
    write_private(&root.join("README.txt"), readme.as_bytes())?;
    hashes.insert("README.txt".into(), sha_of(readme.as_bytes()));

    let manifest = json!({
        "format": FORMAT, "version": VERSION, "program": env!("CARGO_PKG_VERSION"),
        "created_at": util::iso(at),
        "person": { "name": who.name, "display": who.display },
        "vaults": entries, "hashes": hashes,
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
    pub name: String,
    pub kind: String,
    pub facts: i64,
    pub files: i64,
}

/// Takes the spaces of an export into this installation as the property of `owner`. With a
/// `prefix`, each space is named `<prefix>-<name>`. Nothing is changed unless the whole export
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
    let spaces = manifest["vaults"]
        .as_array()
        .ok_or_else(|| Error::Invalid("the manifest lists no space".into()))?;
    let ctl = brain.open_control()?;
    let catalogue = brain.catalogue()?;

    struct Plan<'a> {
        orig: String,
        target: Space,
        kind: String,
        entry: &'a Value,
    }
    let mut plan = vec![];
    let mut taken = vec![];
    for v in spaces {
        let orig = v["name"].as_str().unwrap_or_default().to_string();
        Space::new(&orig)?;
        let target = Space::new(&match prefix {
            Some(p) => format!("{p}-{orig}"),
            None => orig.clone(),
        })?;
        if brain.has_space(&target)? || brain.dir().join(target.file_name()).exists() {
            taken.push(target.to_string());
        }
        let kind = v["kind"].as_str().unwrap_or("custom");
        let kind = if catalogue.iter().any(|k| k.key == kind) {
            kind
        } else {
            crate::catalogue::CUSTOM
        };
        plan.push(Plan {
            orig,
            target,
            kind: kind.to_string(),
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

    let mut made: Vec<PathBuf> = vec![];
    let mut registered: Vec<Space> = vec![];
    let mut done: Vec<Adopted> = vec![];
    let run = (|| -> Result<()> {
        for p in &plan {
            let tmp = brain.dir().join(format!(".adopt-{}.sqlite", p.target));
            fs::copy(dir.join(format!("vaults/{}.sqlite", p.orig)), &tmp)?;
            crate::fsx::set_private_file(&tmp)?;
            made.push(tmp.clone());
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
            let final_path = brain.dir().join(p.target.file_name());
            fs::rename(&tmp, &final_path)?;
            made.retain(|m| m != &tmp);
            made.push(final_path);

            let blobs = dir.join("files").join(&p.orig);
            if blobs.is_dir() {
                let to = brain.dir().join("files").join(p.target.as_str());
                private_dir(&to)?;
                made.push(to.clone());
                for entry in fs::read_dir(&blobs)? {
                    let path = entry?.path();
                    if let Some(name) = path.file_name() {
                        fs::copy(&path, to.join(name))?;
                        crate::fsx::set_private_file(&to.join(name))?;
                    }
                }
            }
            ctl.execute(
                "INSERT INTO spaces (name, created_at, kind) VALUES (?1, ?2, ?3)",
                rusqlite::params![p.target.as_str(), util::now(), p.kind],
            )?;
            ctl.execute(
                "INSERT INTO space_members (space, user_id, can_write, owner) VALUES (?1, ?2, 1, 1)",
                rusqlite::params![p.target.as_str(), owner.id],
            )?;
            registered.push(p.target.clone());
            done.push(Adopted {
                name: p.target.to_string(),
                kind: p.kind.clone(),
                facts,
                files,
            });
        }
        // The instructions in force at the time of the export come back, for each space, and
        // for the person if they have none yet.
        for g in guides.iter().filter(|g| g["current"] == true) {
            let scope = g["scope"].as_str().unwrap_or_default();
            let body = g["body"].as_str().unwrap_or_default();
            let target = if scope == "person" {
                if guide::current(&ctl, &guide::person_scope(&owner.id))?.is_some() {
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
            guide::set(&ctl, &owner.id, &target, body)?;
        }
        Ok(())
    })();
    if let Err(e) = run {
        // Undo: nothing of a half-taken export stays.
        for s in &registered {
            let _ = ctl.execute("DELETE FROM space_members WHERE space = ?1", [s.as_str()]);
            let _ = ctl.execute(
                "DELETE FROM guides WHERE scope = ?1",
                [guide::space_scope(s)],
            );
            let _ = ctl.execute("DELETE FROM spaces WHERE name = ?1", [s.as_str()]);
        }
        for path in made.iter().rev() {
            if path.is_dir() {
                let _ = fs::remove_dir_all(path);
            } else {
                let _ = fs::remove_file(path);
                let _ = fs::remove_file(format!("{}-wal", path.display()));
                let _ = fs::remove_file(format!("{}-shm", path.display()));
            }
        }
        return Err(e);
    }
    Ok(done)
}
