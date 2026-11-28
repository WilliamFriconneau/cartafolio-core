use crate::alias::{backfill_aliases, insert_default_alias, validate_alias};
use crate::AccessError;
use pensine_store::Store;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE accounts (id TEXT PRIMARY KEY NOT NULL);
     CREATE TABLE connections (
        id TEXT PRIMARY KEY NOT NULL,
        account TEXT NOT NULL REFERENCES accounts(id),
        name TEXT NOT NULL,
        key_hash BLOB NOT NULL UNIQUE CHECK(length(key_hash) = 32),
        read_only INTEGER NOT NULL
     );
     CREATE INDEX connections_account ON connections(account);
     CREATE TABLE grants (
        connection TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
        space TEXT NOT NULL,
        write INTEGER NOT NULL,
        PRIMARY KEY(connection, space)
     );",
    "CREATE TABLE ha_users (
        ha_id TEXT PRIMARY KEY NOT NULL,
        account_id TEXT NOT NULL UNIQUE REFERENCES accounts(id),
        name TEXT NOT NULL,
        created_at INTEGER NOT NULL
     );",
    "CREATE TABLE account_aliases (
        account TEXT PRIMARY KEY NOT NULL REFERENCES accounts(id),
        alias TEXT NOT NULL UNIQUE
     );",
];

#[derive(Clone)]
pub struct Account {
    pub id: String,
    pub store: Arc<Store>,
    operations: Arc<Mutex<()>>,
}

impl Account {
    /// Serializes operations that resolve permissions and then use the result.
    pub fn lock_operations(&self) -> Result<MutexGuard<'_, ()>, AccessError> {
        self.operations.lock().map_err(|_| AccessError::Conflict)
    }
}

pub struct Accounts {
    root: PathBuf,
    writer: Mutex<Connection>,
    cache: Mutex<HashMap<String, Account>>,
}

impl Accounts {
    pub fn open(root: &Path) -> Result<Self, AccessError> {
        fs::create_dir_all(root.join("accounts"))?;
        let root = root.canonicalize()?;
        let mut db = Connection::open(root.join("access.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "foreign_keys", true)?;
        let journal: String = db.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !journal.eq_ignore_ascii_case("wal") {
            return Err(AccessError::Invalid(
                "Could not enable SQLite WAL mode".into(),
            ));
        }
        migrate(&mut db)?;
        Ok(Self {
            root,
            writer: Mutex::new(db),
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn account_create(&self, id: &str) -> Result<Account, AccessError> {
        self.account_create_with_alias(id, None)
    }

    pub fn account_create_with_alias(
        &self,
        id: &str,
        alias: Option<&str>,
    ) -> Result<Account, AccessError> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(AccessError::Invalid(
                "An account id must contain only lowercase ASCII letters, digits, or hyphens"
                    .into(),
            ));
        }
        let mut cache = self.cache.lock().map_err(|_| AccessError::Conflict)?;
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute(
            "INSERT INTO accounts VALUES (?1) ON CONFLICT DO NOTHING",
            [id],
        )? == 0
        {
            return Err(AccessError::Conflict);
        }
        if let Some(alias) = alias {
            validate_alias(alias)?;
            if tx.execute(
                "INSERT INTO account_aliases (account, alias) VALUES (?1, ?2)
                 ON CONFLICT(alias) DO NOTHING",
                (id, alias),
            )? == 0
            {
                return Err(AccessError::Invalid("This alias is already taken".into()));
            }
        } else {
            insert_default_alias(&tx, id, id)?;
        }
        let account = self.open_account(id)?;
        tx.commit()?;
        cache.insert(id.to_owned(), account.clone());
        Ok(account)
    }

    /// Maps a Home Assistant user to its account, creating the account on the
    /// first sight with a deterministic id derived from the Home Assistant id.
    pub fn account_for_ha_user(&self, ha_id: &str, name: &str) -> Result<Account, AccessError> {
        if ha_id.trim().is_empty() {
            return Err(AccessError::Invalid(
                "A Home Assistant user id is required".into(),
            ));
        }
        let mut cache = self.cache.lock().map_err(|_| AccessError::Conflict)?;
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT account_id FROM ha_users WHERE ha_id = ?1",
                [ha_id],
                |row| row.get(0),
            )
            .optional()?;
        let id = existing.unwrap_or_else(|| account_id_for_ha_user(ha_id));
        if tx.execute(
            "INSERT INTO accounts VALUES (?1) ON CONFLICT DO NOTHING",
            [&id],
        )? > 0
        {
            insert_default_alias(&tx, &id, name)?;
        }
        tx.execute(
            "INSERT INTO ha_users (ha_id, account_id, name, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(ha_id) DO UPDATE SET name = excluded.name",
            (ha_id, &id, name, now()?),
        )?;
        let account = match cache.get(&id) {
            Some(account) => account.clone(),
            None => self.open_account(&id)?,
        };
        tx.commit()?;
        cache.insert(id, account.clone());
        Ok(account)
    }

    /// Lists account ids and their public aliases in id order.
    pub fn account_list(&self) -> Result<Vec<(String, String)>, AccessError> {
        let db = self.db()?;
        let mut statement =
            db.prepare("SELECT account, alias FROM account_aliases ORDER BY account")?;
        let accounts = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(accounts)
    }

    pub fn account(&self, id: &str) -> Result<Account, AccessError> {
        let mut cache = self.cache.lock().map_err(|_| AccessError::Conflict)?;
        if let Some(account) = cache.get(id) {
            return Ok(account.clone());
        }
        let exists: bool = self.db()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AccessError::NotFound);
        }
        let account = self.open_account(id)?;
        cache.insert(id.to_owned(), account.clone());
        Ok(account)
    }

    fn open_account(&self, id: &str) -> Result<Account, AccessError> {
        Ok(Account {
            id: id.into(),
            store: Arc::new(Store::open(&self.root.join("accounts").join(id))?),
            operations: Arc::default(),
        })
    }

    pub(crate) fn db(&self) -> Result<MutexGuard<'_, Connection>, AccessError> {
        self.writer.lock().map_err(|_| AccessError::Conflict)
    }
}

fn account_id_for_ha_user(ha_id: &str) -> String {
    let sanitised: String = ha_id
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(12)
        .collect();
    if sanitised.is_empty() {
        let digest = Sha256::digest(ha_id.as_bytes());
        let hex: String = digest
            .iter()
            .take(6)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("u-{hex}")
    } else {
        format!("u-{sanitised}")
    }
}

fn now() -> Result<i64, AccessError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AccessError::Invalid("System clock is out of range".into()))?
        .as_secs();
    i64::try_from(seconds).map_err(|_| AccessError::Invalid("System clock is out of range".into()))
}

fn migrate(db: &mut Connection) -> Result<(), AccessError> {
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index + 1;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: usize = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current > MIGRATIONS.len() {
            return Err(AccessError::Invalid(
                "Unsupported access schema version".into(),
            ));
        }
        if current < version {
            tx.execute_batch(sql)?;
            if version == 3 {
                backfill_aliases(&tx)?;
            }
            tx.pragma_update(None, "user_version", version)?;
        }
        tx.commit()?;
    }
    Ok(())
}
