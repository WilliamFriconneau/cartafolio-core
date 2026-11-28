use crate::AccessError;
use pensine_store::Store;
use rusqlite::{Connection, TransactionBehavior};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const MIGRATIONS: &[&str] = &["CREATE TABLE accounts (id TEXT PRIMARY KEY NOT NULL);
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
     );"];

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
        let account = self.open_account(id)?;
        tx.commit()?;
        cache.insert(id.to_owned(), account.clone());
        Ok(account)
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
            tx.pragma_update(None, "user_version", version)?;
        }
        tx.commit()?;
    }
    Ok(())
}
