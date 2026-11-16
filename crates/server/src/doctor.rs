//! `doctor`: reads an installation and says, in plain words, what is fine and what deserves a
//! look. It writes nothing and starts no migration: every database is opened read-only, so it is
//! safe to run on a live installation, before an update included.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use memcp_core::access;
use memcp_core::users;
use memcp_core::util::{iso, now};
use memcp_core::{CONTROL_SCHEMA_VERSION, SPACE_SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};

use crate::cli::Failure;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

/// What `doctor` found, line by line.
#[derive(Default)]
pub struct Report {
    lines: Vec<(Level, String)>,
}

impl Report {
    fn add(&mut self, level: Level, text: impl Into<String>) {
        self.lines.push((level, text.into()));
    }

    pub fn problems(&self) -> usize {
        self.lines.iter().filter(|(l, _)| *l == Level::Fail).count()
    }

    pub fn warnings(&self) -> usize {
        self.lines.iter().filter(|(l, _)| *l == Level::Warn).count()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for (level, text) in &self.lines {
            let tag = match level {
                Level::Ok => "ok  ",
                Level::Info => "info",
                Level::Warn => "warn",
                Level::Fail => "FAIL",
            };
            out.push_str(&format!("{tag}  {text}\n"));
        }
        out
    }

    /// The lines that are not plain "ok", for a test.
    pub fn lines(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.lines.iter().map(|(l, t)| {
            (
                match l {
                    Level::Ok => "ok",
                    Level::Info => "info",
                    Level::Warn => "warn",
                    Level::Fail => "fail",
                },
                t.as_str(),
            )
        })
    }
}

fn read_only(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

fn version(c: &Connection) -> rusqlite::Result<u32> {
    c.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn quick(c: &Connection) -> String {
    c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
        .unwrap_or_else(|e| e.to_string())
}

/// What a directory is, which decides what is expected of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// An installation of its own: an administrator, vaults.
    Engine,
    /// The registry of a manager: the people, no vault.
    Registry,
    /// One person's directory under a manager: vaults, and no administrator (the manager has them).
    Person,
}

/// Reads the installation in `data`, and, when given, the public address and the local listener.
pub fn inspect(data: &Path, public_url: Option<&str>, listen: Option<&str>) -> Report {
    inspect_as(data, Role::Engine, public_url, listen)
}

/// [`inspect`] for a directory that has a role under a manager.
pub fn inspect_as(
    data: &Path,
    role: Role,
    public_url: Option<&str>,
    listen: Option<&str>,
) -> Report {
    let mut r = Report::default();
    storage(&mut r, data);
    if let Ok(ctl) = read_only(&data.join("control.sqlite")) {
        control(&mut r, data, &ctl, role);
    }
    if let Some(url) = public_url {
        address(&mut r, url);
    }
    if let Some(addr) = listen {
        listener(&mut r, addr);
    }
    r
}

fn storage(r: &mut Report, data: &Path) {
    let Ok(meta) = std::fs::metadata(data) else {
        r.add(
            Level::Fail,
            format!("the data directory {} does not exist", data.display()),
        );
        return;
    };
    if !memcp_core::fsx::loose(&meta) {
        r.add(Level::Ok, "the data directory is private to its owner");
    } else {
        r.add(
            Level::Warn,
            format!(
                "the data directory is readable by others (mode {:o}). Fix: chmod 700 {}",
                memcp_core::fsx::mode_bits(&meta).unwrap_or(0) & 0o777,
                data.display()
            ),
        );
    }
    let mut loose = vec![];
    if let Ok(rd) = std::fs::read_dir(data) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "sqlite")
                && e.metadata().is_ok_and(|m| memcp_core::fsx::loose(&m))
            {
                loose.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    if !loose.is_empty() {
        r.add(
            Level::Warn,
            format!(
                "these files are readable by others: {}. Fix: chmod 600 on them",
                loose.join(", ")
            ),
        );
    }
}

fn control(r: &mut Report, data: &Path, ctl: &Connection, role: Role) {
    match version(ctl) {
        Ok(v) if v == CONTROL_SCHEMA_VERSION => {
            r.add(Level::Ok, format!("control database: version {v}, the current one"));
        }
        Ok(v) if v < CONTROL_SCHEMA_VERSION => r.add(
            Level::Warn,
            format!("control database is at version {v}, this program wants {CONTROL_SCHEMA_VERSION}: it migrates at the next start (take a backup first)"),
        ),
        Ok(v) => r.add(
            Level::Fail,
            format!("control database is at version {v}, newer than this program ({CONTROL_SCHEMA_VERSION}): do not start it with this version"),
        ),
        Err(e) => r.add(Level::Fail, format!("control database unreadable: {e}")),
    }
    let q = quick(ctl);
    if q != "ok" {
        r.add(
            Level::Fail,
            format!("control database failed its integrity check: {q}"),
        );
    }
    vaults(r, data, ctl, role);
    people(r, ctl, role);
    connections(r, ctl);
}

fn vaults(r: &mut Report, data: &Path, ctl: &Connection, role: Role) {
    let names: Vec<String> = ctl
        .prepare("SELECT name FROM spaces ORDER BY name")
        .and_then(|mut st| st.query_map([], |row| row.get(0))?.collect())
        .unwrap_or_default();
    if names.is_empty() && role != Role::Registry {
        r.add(
            Level::Warn,
            "no vault yet: create one with `vault add KIND`, or on the page",
        );
    }
    for name in names {
        let path = data.join(format!("{name}.sqlite"));
        if !path.exists() {
            r.add(Level::Fail, format!("vault {name}: its file is missing"));
            continue;
        }
        let c = match read_only(&path) {
            Ok(c) => c,
            Err(e) => {
                r.add(Level::Fail, format!("vault {name}: cannot be opened ({e})"));
                continue;
            }
        };
        let v = version(&c).unwrap_or(0);
        let q = quick(&c);
        let facts: i64 = c
            .query_row(
                "SELECT count(*) FROM facts WHERE status = 'active'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if q != "ok" {
            r.add(
                Level::Fail,
                format!("vault {name}: failed its integrity check: {q}"),
            );
        } else if v > SPACE_SCHEMA_VERSION {
            r.add(
                Level::Fail,
                format!(
                    "vault {name}: version {v} is newer than this program ({SPACE_SCHEMA_VERSION})"
                ),
            );
        } else if v < SPACE_SCHEMA_VERSION {
            r.add(Level::Warn, format!("vault {name}: version {v}, migrates to {SPACE_SCHEMA_VERSION} at the next start ({facts} facts; take a backup first)"));
        } else {
            r.add(
                Level::Ok,
                format!("vault {name}: version {v}, {facts} facts, integrity ok"),
            );
        }
        let inbox: i64 = c
            .query_row(
                "SELECT count(DISTINCT f.id) FROM facts f JOIN fact_tags t ON t.fact_id = f.id
                 WHERE f.status = 'active' AND (t.tag = 'inbox' OR substr(t.tag, 1, 6) = 'inbox/')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if inbox > 0 {
            r.add(
                Level::Info,
                format!("vault {name}: {inbox} quick note(s) wait in the inbox to be sorted"),
            );
        }
    }
}

fn people(r: &mut Report, ctl: &Connection, role: Role) {
    let Ok(all) = users::list_users(ctl) else {
        return;
    };
    let live: Vec<_> = all.iter().filter(|u| !u.disabled).collect();
    let admins = live.iter().filter(|u| u.admin).count();
    if role == Role::Person {
        r.add(Level::Ok, "one person, who answers to the registry");
    } else if admins == 0 {
        r.add(
            Level::Fail,
            "no administrator: nobody can open the admin page",
        );
    } else {
        r.add(
            Level::Ok,
            format!("people: {} active, {admins} administrator(s)", live.len()),
        );
    }
}

fn connections(r: &mut Report, ctl: &Connection) {
    let Ok(list) = access::list(ctl) else {
        return;
    };
    let t = now();
    let live: Vec<_> = list.iter().filter(|c| c.revoked_at.is_none()).collect();
    r.add(
        Level::Ok,
        format!(
            "connections: {} active, {} revoked",
            live.len(),
            list.len() - live.len()
        ),
    );
    for c in live {
        if c.expires_at.is_some_and(|e| e < t) {
            r.add(
                Level::Info,
                format!("connection {} has expired: revoke it to tidy up", c.agent),
            );
        }
        if c.rights == "all" {
            r.add(
                Level::Warn,
                format!("connection {} reaches every vault of its person. Prefer one limited to the vaults it needs (`regrant {} --grant vault:rw`)", c.agent, c.agent),
            );
        }
        let idle = c.last_used_at.map_or(t - c.created_at, |u| t - u);
        if idle > 30 * 86_400 {
            r.add(
                Level::Info,
                format!(
                    "connection {} {} for {} days: revoke it if it is no longer used",
                    c.agent,
                    c.last_used_at
                        .map_or("was never used, made".to_string(), |u| format!(
                            "last used {},",
                            iso(u)
                        )),
                    idle / 86_400
                ),
            );
        }
    }
}

fn address(r: &mut Report, url: &str) {
    let lower = url.to_lowercase();
    if url.ends_with('/') {
        r.add(
            Level::Warn,
            "the public address ends with a slash: remove it",
        );
    }
    if !lower.starts_with("https://") {
        r.add(
            Level::Warn,
            "the public address is not HTTPS: Claude on the web and most apps need HTTPS",
        );
    } else {
        r.add(Level::Ok, "the public address is HTTPS");
    }
    if [
        "localhost",
        "127.",
        "192.168.",
        "10.",
        "homeassistant.local",
    ]
    .iter()
    .any(|h| lower.contains(h))
    {
        r.add(Level::Warn, "the public address only works on your own network: Claude on the web calls your server from the internet");
    }
    if lower.contains(".ts.net") {
        r.add(
            Level::Info,
            "a Tailscale name is reachable from the internet only if Funnel is on for it",
        );
    }
    r.add(
        Level::Info,
        format!("from outside your network, check it answers: curl -sS {url}/health  (expect {{\"ok\":true}}), and curl -sS {url}/.well-known/oauth-authorization-server"),
    );
}

fn listener(r: &mut Report, addr: &str) {
    let Some(sock) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        r.add(
            Level::Warn,
            format!("cannot read the listener address {addr}"),
        );
        return;
    };
    match TcpStream::connect_timeout(&sock, Duration::from_secs(2)) {
        Err(e) => r.add(
            Level::Warn,
            format!("nothing answers on {addr} ({e}): the server is not running there"),
        ),
        Ok(mut s) => {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = s.write_all(
                format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
            let mut body = String::new();
            let _ = s.read_to_string(&mut body);
            if body.starts_with("HTTP/1.1 200") || body.starts_with("HTTP/1.0 200") {
                r.add(Level::Ok, format!("the server answers on {addr}"));
            } else {
                r.add(
                    Level::Warn,
                    format!("{addr} answers, but not like this server's /health"),
                );
            }
        }
    }
}

/// The command: prints the report, and fails when something is wrong.
pub fn run(data: &Path, public_url: Option<&str>, listen: Option<&str>) -> Result<(), Failure> {
    let r = inspect(data, public_url, listen);
    print!("{}", r.render());
    let (p, w) = (r.problems(), r.warnings());
    println!("\n{p} problem(s), {w} warning(s).");
    if p > 0 {
        return Err((1, "problems found".into()));
    }
    Ok(())
}
