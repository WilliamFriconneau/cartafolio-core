use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::error::{Error, Result};
use crate::fsx::{PrivateDir as _, PrivateFile as _};
use crate::schema::{CONTROL_MIGRATIONS, SPACE_MIGRATIONS};
use crate::space::Space;
use crate::text::Language;
use crate::util::now;

const CONFIG_FILE: &str = "config.toml";
const CONTROL_FILE: &str = "control.sqlite";

/// Configuration written by `init`.
pub fn default_config(language: Language) -> String {
    format!(
        "# memcp configuration (defaults written by `memcp init`).\n\
         listen = \"127.0.0.1:8791\"\n\
         # Search language: \"en\" or \"fr\". Changing it requires a reindex.\n\
         language = \"{}\"\n",
        language.code()
    )
}

/// The data directory: one SQLite file per space, plus the control database.
///
/// A `Brain` keeps no connection open: each call to `open_space` or
/// `open_control` opens the wanted file and only that one, which makes the
/// isolation physical (a token without rights on a space never opens its file).
#[derive(Debug, Clone)]
pub struct Brain {
    dir: PathBuf,
}

impl Brain {
    /// Creates the directory (0700), the control database, the given spaces and
    /// the default configuration (files in 0600). Refuses if the directory already
    /// holds memcp files or is readable by others. On failure, removes
    /// what was just created.
    pub fn init(dir: &Path, spaces: &[Space], language: Language) -> Result<Brain> {
        let created_dir = prepare_dir(dir)?;
        let mut created: Vec<PathBuf> = Vec::new();
        let brain = Brain {
            dir: dir.to_path_buf(),
        };
        let result = (|| {
            init_control(dir, &mut created)?;
            for space in spaces {
                brain.create_space(space, crate::users::OWNER, &mut created)?;
            }
            write_private(
                &dir.join(CONFIG_FILE),
                &default_config(language),
                &mut created,
            )
        })();
        match result {
            Ok(()) => Ok(brain),
            Err(e) => {
                for p in &created {
                    remove_db_files(p);
                }
                if created_dir {
                    let _ = fs::remove_dir(dir);
                }
                Err(e)
            }
        }
    }

    /// Opens an initialised directory (it must hold `control.sqlite`).
    pub fn open(dir: &Path) -> Result<Brain> {
        if !dir.join(CONTROL_FILE).is_file() {
            return Err(Error::NotInitialized(dir.to_path_buf()));
        }
        Ok(Brain {
            dir: dir.to_path_buf(),
        })
    }

    /// The data directory this handle points at (the folder given to `init` or `open`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The search language set in the configuration (English if absent).
    pub fn language(&self) -> Result<Language> {
        let Ok(cfg) = fs::read_to_string(self.dir.join(CONFIG_FILE)) else {
            return Ok(Language::English);
        };
        for line in cfg.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            if k.trim() == "language" {
                let code = v.trim().trim_matches('"');
                return Language::parse(code)
                    .ok_or_else(|| Error::Invalid(format!("unknown language {code:?} in config")));
            }
        }
        Ok(Language::English)
    }

    /// Registered spaces, in name order.
    pub fn spaces(&self) -> Result<Vec<Space>> {
        let ctl = self.open_control()?;
        let mut st = ctl.prepare("SELECT name FROM spaces ORDER BY name")?;
        let names: Vec<String> = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        names.iter().map(|n| Space::new(n)).collect()
    }

    /// Whether `space` is registered in the control database. Read-only; a registered
    /// space whose file is missing still answers `true` (use `doctor` to find that).
    pub fn has_space(&self, space: &Space) -> Result<bool> {
        let ctl = self.open_control()?;
        Ok(ctl
            .query_row(
                "SELECT 1 FROM spaces WHERE name = ?1",
                [space.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The kinds of vault this installation offers: its `catalogue.txt` if it has one, the
    /// default for a person's life otherwise.
    pub fn catalogue(&self) -> Result<Vec<crate::catalogue::Kind>> {
        match fs::read_to_string(self.dir.join("catalogue.txt")) {
            Ok(text) => crate::catalogue::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(crate::catalogue::default_catalogue())
            }
            Err(e) => Err(Error::Invalid(format!("catalogue.txt unreadable: {e}"))),
        }
    }

    /// The kind of a space (`custom` when it was made without one).
    pub fn space_kind(&self, space: &Space) -> Result<String> {
        let ctl = self.open_control()?;
        ctl.query_row(
            "SELECT kind FROM spaces WHERE name = ?1",
            [space.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::UnknownSpace(space.to_string()))
    }

    /// Gives a space a kind of the catalogue (or back to `custom`).
    pub fn set_space_kind(&self, space: &Space, kind: &str) -> Result<()> {
        if kind != crate::catalogue::CUSTOM && !self.catalogue()?.iter().any(|k| k.key == kind) {
            return Err(Error::Invalid(format!(
                "unknown kind {kind:?}; the catalogue has: {}",
                self.catalogue()?
                    .iter()
                    .map(|k| k.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let ctl = self.open_control()?;
        let n = ctl.execute(
            "UPDATE spaces SET kind = ?2 WHERE name = ?1",
            [space.as_str(), kind],
        )?;
        if n == 1 {
            Ok(())
        } else {
            Err(Error::UnknownSpace(space.to_string()))
        }
    }

    /// Creates a person's vault of a kind of the catalogue, named after the person and the
    /// kind. A person has one vault of each kind.
    pub fn add_vault(&self, user_id: &str, person: &str, kind: &str) -> Result<Space> {
        if !self.catalogue()?.iter().any(|k| k.key == kind) {
            return Err(Error::Invalid(format!(
                "unknown kind {kind:?}; the catalogue has: {}",
                self.catalogue()?
                    .iter()
                    .map(|k| k.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        // One of each kind per person, however the vault got its kind.
        let owned: Option<String> = self
            .open_control()?
            .query_row(
                "SELECT s.name FROM spaces s JOIN space_members m ON m.space = s.name AND m.owner = 1
                 WHERE m.user_id = ?1 AND s.kind = ?2 ORDER BY s.name LIMIT 1",
                [user_id, kind],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(name) = owned {
            return Err(Error::Invalid(format!(
                "{person} already has a {kind} vault ({name})"
            )));
        }
        let space = crate::catalogue::vault_name(person, kind)?;
        if self.has_space(&space)? {
            return Err(Error::Invalid(format!(
                "{person} already has a {kind} vault ({space})"
            )));
        }
        self.add_space_for(&space, user_id)?;
        self.set_space_kind(&space, kind)?;
        // A bridge paired before « À trier » existed, for « all my vaults », writes to the inbox of this one too.
        let ctl = self.open_control()?;
        let ids: Vec<String> = ctl
            .prepare(
                "SELECT id FROM connections WHERE user_id = ?1 AND all_inboxes = 1 AND revoked_at IS NULL",
            )?
            .query_map([user_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for id in ids {
            ctl.execute(
                "INSERT OR IGNORE INTO grants (connection_id, space, can_read, can_write) VALUES (?1, ?2, 1, 1)",
                rusqlite::params![id, space.as_str()],
            )?;
            ctl.execute(
                "INSERT OR IGNORE INTO grant_paths (connection_id, space, path, can_write) VALUES (?1, ?2, 'inbox', 1)",
                rusqlite::params![id, space.as_str()],
            )?;
        }
        Ok(space)
    }

    /// Creates a new space owned by the bootstrap administrator.
    pub fn add_space(&self, space: &Space) -> Result<()> {
        self.add_space_for(space, crate::users::OWNER)
    }

    /// Creates a new space (its file and its registration) owned by `user_id`,
    /// who gets read and write access. Refuses an existing one.
    pub fn add_space_for(&self, space: &Space, user_id: &str) -> Result<()> {
        let known_user = self
            .open_control()?
            .query_row(
                "SELECT 1 FROM users WHERE id = ?1 AND disabled_at IS NULL",
                [user_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !known_user {
            return Err(Error::Invalid(format!(
                "unknown or disabled user: {user_id}"
            )));
        }
        if self.has_space(space)? || self.dir.join(space.file_name()).exists() {
            return Err(Error::Invalid(format!("space {space} already exists")));
        }
        // A vault and a connection end an address the same way (`/mcp/<name>`): never the same name.
        let used: Option<i64> = self
            .open_control()?
            .query_row(
                "SELECT 1 FROM connections WHERE agent = ?1",
                [space.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if used.is_some() {
            return Err(Error::Invalid(format!(
                "{space} is the name of a connection: a vault needs another name"
            )));
        }
        let mut created = Vec::new();
        self.create_space(space, user_id, &mut created)
            .inspect_err(|_| {
                for p in &created {
                    remove_db_files(p);
                }
            })
    }

    fn create_space(&self, space: &Space, owner: &str, created: &mut Vec<PathBuf>) -> Result<()> {
        let path = self.dir.join(space.file_name());
        create_private_file(&path, created)?;
        let conn = open_db(&path)?;
        migrate(&conn, SPACE_MIGRATIONS)?;
        write_meta(&conn, &[("space", space.as_str())])?;
        drop(conn);
        let ctl = self.open_control()?;
        let tx =
            rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO spaces (name, created_at) VALUES (?1, ?2)",
            (space.as_str(), now()),
        )?;
        tx.execute(
            "INSERT INTO space_members (space, user_id, can_write, owner) VALUES (?1, ?2, 1, 1)",
            (space.as_str(), owner),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Opens a space file, applies missing migrations and checks that the file
    /// declares this space (a file stored under the wrong name is refused).
    pub fn open_space(&self, space: &Space) -> Result<Connection> {
        if !self.has_space(space)? {
            return Err(Error::UnknownSpace(space.to_string()));
        }
        let conn = self.open_existing(&space.file_name())?;
        migrate(&conn, SPACE_MIGRATIONS)?;
        crate::sync::ensure_log(&conn)?;
        let declared = meta(&conn, "space")?;
        if declared.as_deref() != Some(space.as_str()) {
            return Err(Error::Schema(format!(
                "{} declares space {:?} instead of {:?}",
                space.file_name(),
                declared.unwrap_or_default(),
                space.as_str()
            )));
        }
        Ok(conn)
    }

    /// Opens the control database (tokens, rights, spaces, authentication audit).
    pub fn open_control(&self) -> Result<Connection> {
        let conn = self.open_existing(CONTROL_FILE)?;
        migrate(&conn, CONTROL_MIGRATIONS)?;
        if meta(&conn, "kind")?.as_deref() != Some("control") {
            return Err(Error::Schema(format!(
                "{CONTROL_FILE} is not a control database"
            )));
        }
        Ok(conn)
    }

    /// Consistent copy of every database (`VACUUM INTO`, safe while the server
    /// writes) and of the configuration into `root/backup-YYYY-MM-DDTHHMMSS/`, then
    /// keeps only the `keep` most recent ones. `root` must be outside the data
    /// directory. Every copy is checked (`integrity_check`) before it is kept.
    pub fn backup(&self, root: &Path, keep: usize, at: i64) -> Result<PathBuf> {
        let data = crate::fsx::canonical(&self.dir)?;
        if resolves_inside(root, &data)? {
            return Err(Error::Invalid(format!(
                "the backup must be outside the data directory ({})",
                data.display()
            )));
        }
        DirBuilder::new()
            .recursive(true)
            .private_dir()
            .create(root)?;
        let root = crate::fsx::canonical(root)?;
        crate::fsx::set_private_dir(&root)?;
        let dest = root.join(format!("backup-{}", crate::util::compact(at)));
        DirBuilder::new().private_dir().create(&dest)?;

        let files = self
            .spaces()?
            .iter()
            .map(Space::file_name)
            .chain([CONTROL_FILE.to_string()])
            .collect::<Vec<_>>();
        for file in files {
            let target = dest.join(&file);
            let conn = self.open_existing(&file)?;
            conn.execute("VACUUM INTO ?1", [target.to_string_lossy()])?;
            crate::fsx::set_private_file(&target)?;
            let copy = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let ok: String = copy.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            if ok != "ok" {
                return Err(Error::Schema(format!("copy of {file} is damaged: {ok}")));
            }
        }
        copy_tree(&self.dir.join("files"), &dest.join("files"))?;
        let cfg = self.dir.join(CONFIG_FILE);
        if cfg.is_file() {
            fs::copy(&cfg, dest.join(CONFIG_FILE))?;
            crate::fsx::set_private_file(&dest.join(CONFIG_FILE))?;
        }

        let mut old: Vec<PathBuf> = fs::read_dir(&root)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("backup-"))
            })
            .collect();
        old.sort();
        let excess = old.len().saturating_sub(keep.max(1));
        for p in old.into_iter().take(excess) {
            fs::remove_dir_all(p)?;
        }
        Ok(dest)
    }

    fn open_existing(&self, file: &str) -> Result<Connection> {
        let path = self.dir.join(file);
        if !path.is_file() {
            return Err(Error::NotInitialized(self.dir.clone()));
        }
        open_db(&path)
    }
}

/// Copies a directory of files (the stored photos and documents), private, if it exists.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    DirBuilder::new().recursive(true).private_dir().create(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let meta = fs::symlink_metadata(&src)?;
        if meta.is_dir() {
            copy_tree(&src, &dst)?;
        } else if meta.is_file() {
            fs::copy(&src, &dst)?;
            crate::fsx::set_private_file(&dst)?;
        }
    }
    Ok(())
}

/// Whether `p` (which may not exist yet) lies inside `dir` once resolved:
/// nearest existing ancestor canonicalised (symlinks included), then the rest.
pub(crate) fn resolves_inside(p: &Path, dir: &Path) -> Result<bool> {
    let mut base = std::path::absolute(p)?;
    let mut rest = Vec::new();
    while !base.exists() {
        match (base.file_name().map(ToOwned::to_owned), base.parent()) {
            (Some(n), Some(parent)) => {
                rest.push(n);
                base = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut full = crate::fsx::canonical(&base)?;
    full.extend(rest.into_iter().rev());
    Ok(full.starts_with(dir))
}

/// Checks the data directory, creates it if needed. Returns `true` if created here.
fn prepare_dir(dir: &Path) -> Result<bool> {
    if dir.exists() {
        let meta = fs::metadata(dir)?;
        if !meta.is_dir() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not a directory", dir.display()),
            )));
        }
        for f in [CONTROL_FILE, CONFIG_FILE] {
            if dir.join(f).exists() {
                return Err(Error::AlreadyInitialized(dir.to_path_buf()));
            }
        }
        if let Some(mode) = crate::fsx::mode_bits(&meta).map(|m| m & 0o777)
            && mode & 0o077 != 0
        {
            return Err(Error::DirectoryTooOpen {
                path: dir.to_path_buf(),
                mode,
            });
        }
        Ok(false)
    } else {
        DirBuilder::new()
            .recursive(true)
            .private_dir()
            .create(dir)?;
        Ok(true)
    }
}

fn init_control(dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let path = dir.join(CONTROL_FILE);
    create_private_file(&path, created)?;
    let conn = open_db(&path)?;
    migrate(&conn, CONTROL_MIGRATIONS)?;
    write_meta(&conn, &[("kind", "control")])
}

fn write_private(path: &Path, content: &str, created: &mut Vec<PathBuf>) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .private_file()
        .open(path)?;
    created.push(path.to_path_buf());
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// Creates an empty 0600 file before SQLite touches it (SQLite copies the main
/// file's permissions onto `-wal` and `-shm`).
fn create_private_file(path: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .private_file()
        .open(path)?;
    created.push(path.to_path_buf());
    Ok(())
}

fn remove_db_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut s = path.as_os_str().to_os_string();
        s.push(suffix);
        let _ = fs::remove_file(PathBuf::from(s));
    }
}

/// Opens an existing SQLite file with the project settings: WAL,
/// `secure_delete`, foreign keys, small cache, no `mmap`.
/// Every write transaction of the core is opened IMMEDIATE (`Transaction::new_unchecked(.., Immediate)`): it takes the
/// write lock at once and waits its turn under the busy timeout. A deferred one that reads first and writes after cannot
/// wait: SQLite refuses its upgrade at once (« database is locked ») when another writer committed in between, which
/// lost notes when many devices wrote at the same time (see crates/client/tests/e2e.rs).
pub(crate) fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA secure_delete = ON;
         PRAGMA foreign_keys = ON;
         PRAGMA cache_size = -4096;
         PRAGMA mmap_size = 0;
         PRAGMA trusted_schema = OFF;",
    )?;
    Ok(conn)
}

/// Applies missing migrations, each in its own transaction. The version is
/// `user_version`; a database newer than the program is refused.
pub(crate) fn migrate(conn: &Connection, migrations: &[&str]) -> Result<()> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let latest = u32::try_from(migrations.len()).unwrap_or(u32::MAX);
    if current > latest {
        return Err(Error::Schema(format!(
            "version {current} is newer than the program's ({latest})"
        )));
    }
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        if sql.contains("writable_schema") {
            // A table's text edited in place: a new schema version makes every connection read it again.
            let v: i64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
            tx.pragma_update(None, "schema_version", v + 1)?;
        }
        let version = i64::try_from(i + 1).unwrap_or(i64::MAX);
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

fn write_meta(conn: &Connection, pairs: &[(&str, &str)]) -> Result<()> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    for (k, v) in pairs {
        tx.execute("INSERT INTO meta (k, v) VALUES (?1, ?2)", (k, v))?;
    }
    tx.execute(
        "INSERT INTO meta (k, v) VALUES ('schema', ?1), ('created_at', ?2)",
        (version.to_string(), now().to_string()),
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT v FROM meta WHERE k = ?1", [key], |r| r.get(0))
        .optional()?)
}
