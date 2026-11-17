//! The node's half of the remote command line (docs/CLI.md): `POST /admin/api/cli` runs a command of the command line for
//! the person behind the key and answers what it printed, and `POST /admin/api/upload` takes the files a command names.
//!
//! The command is the very function the local command line runs (`commands::dispatch`), on the node's own data, with
//! the rights of the key's person: a person acts for themselves, an administrator for anyone, and a command that touches
//! every person or the node's machine needs an administrator's key. Files a command reads come only from what the caller
//! sent here first (never a path of this machine, except for an administrator's commands that name one on purpose).

use std::io::Write;
use std::path::{Path, PathBuf};

use memcp_core::users::User;
use serde_json::{Value, json};

use crate::app::App;
use crate::cli_table::{self, Remote, Up};
use crate::commands;
use crate::config;
use crate::http::{Request, Response};
use crate::out;

const MAX_WORDS: usize = 256;
/// What one upload may hold in all.
const MAX_UPLOAD: u64 = 8 * 1024 * 1024 * 1024;
const MAX_UPLOAD_FILES: usize = 200_000;
/// How long a folder that was sent and never used is kept (seconds).
const STALE: u64 = 12 * 3600;

fn err(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

fn clean_id(id: &str) -> bool {
    (8..=32).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A path inside an upload: plain names only, never absolute, never leaving the folder.
fn clean_rel(rel: &str) -> bool {
    !rel.is_empty()
        && rel.len() <= 500
        && rel
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.contains(['\\', ':', '\0']))
}

fn root(app: &App, me: &User) -> PathBuf {
    app.brain.dir().join("uploads").join(&me.id)
}

/// Where an `@upload:ID[/path]` word points, if it is one and is clean.
fn staged(app: &App, me: &User, word: &str) -> Option<Result<PathBuf, Response>> {
    let rest = word.strip_prefix("@upload:")?;
    let (id, rel) = rest
        .split_once('/')
        .map_or((rest, None), |(i, r)| (i, Some(r)));
    if !clean_id(id) || rel.is_some_and(|r| !clean_rel(r)) {
        return Some(Err(err(400, "a file sent for this command is named badly")));
    }
    let dir = root(app, me).join(id);
    if !dir.is_dir() {
        return Some(Err(err(400, "the files of this command were not sent")));
    }
    Some(Ok(rel.map_or(dir.clone(), |r| dir.join(r))))
}

/// Runs a command for the person behind the key. `Ok` carries `{code, stdout, stderr}`.
pub fn exec(app: &App, me: &User, body: &Value, ip: Option<&str>) -> Result<Value, Response> {
    let words: Vec<String> = body
        .get("argv")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .map(|v| v.as_str().map(String::from))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|a| a.len() <= MAX_WORDS && !a.is_empty())
        .ok_or_else(|| err(400, "argv: the words of the command, as a list of texts"))?;
    let parsed =
        commands::parse(&words).map_err(|(_, m)| err(400, m.lines().next().unwrap_or("")))?;
    let mut a = commands::normalize(&parsed);
    let entry = cli_table::find(&a.pos).ok_or_else(|| err(404, "unknown command"))?;
    match entry.remote {
        Remote::Exec | Remote::Upload(_) => {}
        Remote::Local(why) => {
            return Err(err(
                400,
                &format!("`{}` works on the machine itself: {why}", entry.words),
            ));
        }
        Remote::Download | Remote::Direct => {
            return Err(err(
                400,
                &format!("`{}` has a route of its own, not this one", entry.words),
            ));
        }
    }
    if entry.admin && !me.admin {
        return Err(err(
            403,
            &format!(
                "`{}` needs an administrator's key; this key is {}'s, a member",
                entry.words, me.name
            ),
        ));
    }
    // The machine's own options are never a caller's to set.
    for f in ["--data", "--config"] {
        a.flags.remove(f);
    }
    match a.flags.get("--user") {
        Some(u) if me.admin || u == &me.name || u == &me.id => {}
        Some(_) => {
            return Err(err(
                403,
                "this key acts for its own person only (an administrator's key acts for anyone)",
            ));
        }
        None => {
            a.flags.insert("--user".into(), me.name.clone());
        }
    }
    a.flags
        .entry("--public-url".into())
        .or_insert_with(|| app.public_url.clone());
    // The files the command names were sent first: their words become the folders they were kept in.
    let mut used: Vec<PathBuf> = vec![];
    let mut resolve = |word: &str| -> Result<String, Response> {
        match staged(app, me, word) {
            Some(Ok(p)) => {
                if let Some(id_dir) = p
                    .ancestors()
                    .find(|x| x.parent() == Some(root(app, me).as_path()))
                {
                    used.push(id_dir.to_path_buf());
                }
                Ok(p.to_string_lossy().to_string())
            }
            Some(Err(r)) => Err(r),
            None => Ok(word.to_string()),
        }
    };
    let mut places: Vec<usize> = vec![];
    let mut flag_names: Vec<&str> = vec![];
    if let Remote::Upload(ups) = entry.remote {
        for u in ups {
            match u {
                Up::Pos(i) => places.push(*i),
                Up::PosFrom(i) => places.extend(*i..a.pos.len()),
                Up::Flag(f) => flag_names.push(f),
            }
        }
    }
    for i in places {
        if let Some(w) = a.pos.get(i).cloned() {
            if !w.starts_with("@upload:") {
                return Err(err(
                    400,
                    "the files of this command are sent first (use the remote command line)",
                ));
            }
            a.pos[i] = resolve(&w)?;
        }
    }
    for f in flag_names {
        if let Some(w) = a.flags.get(f).cloned() {
            if !w.starts_with("@upload:") {
                return Err(err(
                    400,
                    "the files of this command are sent first (use the remote command line)",
                ));
            }
            a.flags.insert(f.to_string(), resolve(&w)?);
        }
    }
    let (result, stdout, mut stderr) = out::capture(|| {
        commands::dispatch(app.brain.dir(), &a, &config::Settings::default(), None)
    });
    for dir in used {
        let _ = std::fs::remove_dir_all(dir);
    }
    let code = match result {
        Ok(()) => 0,
        Err((code, message)) => {
            stderr.push_str(&message);
            stderr.push('\n');
            i64::from(code)
        }
    };
    if let Ok(ctl) = app.brain.open_control() {
        memcp_core::access::auth_event(
            &ctl,
            None,
            ip,
            "remote_command",
            &format!("{}: {}", me.name, entry.words),
        );
    }
    Ok(json!({ "code": code, "stdout": stdout, "stderr": stderr }))
}

fn size_of(dir: &Path, files: &mut usize) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += size_of(&p, files);
            } else {
                *files += 1;
                total += e.metadata().map_or(0, |m| m.len());
            }
        }
    }
    total
}

/// Folders sent a long time ago and never used.
fn sweep(uploads: &Path) {
    let Ok(people) = std::fs::read_dir(uploads) else {
        return;
    };
    for person in people.flatten() {
        let Ok(rd) = std::fs::read_dir(person.path()) else {
            continue;
        };
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|d| d.as_secs() > STALE);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// `POST /upload?id=&path=&offset=`: one chunk of one file of an upload, appended where the file stands (the offset must be
/// the file's size now: a chunk is never written twice or out of order). The body is the bytes, never held beyond the chunk.
pub fn upload(app: &App, me: &User, req: &Request) -> Response {
    let q = |k: &str| req.query.get(k).map_or("", String::as_str);
    let (id, rel) = (q("id"), q("path"));
    let Ok(offset) = q("offset").parse::<u64>() else {
        return err(400, "offset: the size of the file so far");
    };
    if !clean_id(id) || !clean_rel(rel) {
        return err(400, "id and path: plain names");
    }
    let base = root(app, me);
    let dir = base.join(id);
    let target = dir.join(rel);
    let have = std::fs::metadata(&target).map_or(0, |m| m.len());
    if have != offset {
        return err(
            409,
            &format!("the file is {have} bytes long here, not {offset}"),
        );
    }
    if offset == 0 {
        if !dir.is_dir() {
            sweep(&app.brain.dir().join("uploads"));
        }
        let mut files = 0;
        if size_of(&dir, &mut files) > MAX_UPLOAD || files >= MAX_UPLOAD_FILES {
            return err(413, "this upload is too big");
        }
    }
    let done = (|| -> std::io::Result<()> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target)?;
        f.write_all(&req.body)
    })();
    match done {
        Ok(()) => Response::json(200, &json!({ "size": offset + req.body.len() as u64 })),
        Err(e) => err(500, &format!("the file could not be written: {e}")),
    }
}

/// `POST /upload/discard` {id}: forgets a folder that was sent (a command that did not go through).
pub fn discard(app: &App, me: &User, body: &Value) -> Result<Value, Response> {
    let id = body.get("id").and_then(Value::as_str).unwrap_or("");
    if !clean_id(id) {
        return Err(err(400, "id: the upload to forget"));
    }
    let _ = std::fs::remove_dir_all(root(app, me).join(id));
    Ok(json!({ "discarded": id }))
}
