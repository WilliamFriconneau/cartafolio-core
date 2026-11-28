use crate::{AccessError, Accounts, Grant, Principal};
use rand::{rngs::OsRng, TryRngCore};
use rusqlite::Connection as Database;
use sha2::{Digest, Sha256};
use std::fmt::{self, Write};
use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub id: String,
    pub account: String,
    pub name: String,
    pub grants: Vec<Grant>,
    pub read_only: bool,
}

pub struct SecretKey(String);

impl SecretKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn generate() -> Result<Self, AccessError> {
        let mut bytes = [0u8; 32];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(std::io::Error::other)?;
        let mut encoded = String::from("pk_");
        for byte in bytes {
            write!(encoded, "{byte:02x}").map_err(std::io::Error::other)?;
        }
        Ok(Self(encoded))
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey([redacted])")
    }
}

impl Accounts {
    pub fn connection_create_owner(
        &self,
        account: &str,
        name: &str,
    ) -> Result<(Connection, SecretKey), AccessError> {
        self.connection_create(
            account,
            name,
            vec![Grant {
                space: "*".into(),
                write: true,
            }],
            false,
        )
    }

    /// Administrative entry point for trusted callers.
    pub fn connection_create(
        &self,
        account: &str,
        name: &str,
        mut grants: Vec<Grant>,
        read_only: bool,
    ) -> Result<(Connection, SecretKey), AccessError> {
        let scope = self.account(account)?;
        let _operations = scope.lock_operations()?;
        for grant in &grants {
            if grant.space != "*" {
                scope.store.space_chain(&grant.space)?;
            }
        }
        grants.sort_unstable_by(|left, right| left.space.cmp(&right.space));
        if grants.windows(2).any(|pair| pair[0].space == pair[1].space) {
            return Err(AccessError::Invalid("Duplicate space grant".into()));
        }

        let key = SecretKey::generate()?;
        let hash = digest(key.as_str());
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let id = tx.query_row(
            "INSERT INTO connections (id, account, name, key_hash, read_only)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4) RETURNING id",
            (account, name, hash.as_slice(), read_only),
            |row| row.get::<_, String>(0),
        )?;
        for grant in &grants {
            tx.execute(
                "INSERT INTO grants VALUES (?1, ?2, ?3)",
                (&id, &grant.space, grant.write),
            )?;
        }
        tx.commit()?;
        Ok((
            Connection {
                id,
                account: account.into(),
                name: name.into(),
                grants,
                read_only,
            },
            key,
        ))
    }

    pub fn connections(&self, account: &str) -> Result<Vec<Connection>, AccessError> {
        self.account(account)?;
        let db = self.db()?;
        let mut stmt = db.prepare("SELECT id, account, name, read_only FROM connections WHERE account = ?1 ORDER BY name, id")?;
        let mut connections = stmt
            .query_map([account], connection_row)?
            .collect::<Result<Vec<_>, _>>()?;
        for connection in &mut connections {
            load_grants(&db, connection)?;
        }
        Ok(connections)
    }

    pub fn connection_revoke(&self, account: &str, id: &str) -> Result<(), AccessError> {
        if self.db()?.execute(
            "DELETE FROM connections WHERE account = ?1 AND id = ?2",
            (account, id),
        )? == 0
        {
            return Err(AccessError::NotFound);
        }
        Ok(())
    }

    pub fn authenticate(&self, bearer: &str) -> Result<Principal, AccessError> {
        let provided = digest(bearer);
        let connection = {
            let db = self.db()?;
            let mut stmt = db.prepare("SELECT id, key_hash FROM connections")?;
            let hashes = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, [u8; 32]>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let index = find_match(
                hashes.iter().map(|(_, hash)| hash),
                &provided,
                |left, right| left.ct_eq(right),
            )
            .ok_or(AccessError::Unauthorized)?;
            let (id, _) = hashes.get(index).ok_or(AccessError::Unauthorized)?;
            let mut connection = db.query_row(
                "SELECT id, account, name, read_only FROM connections WHERE id = ?1",
                [id],
                connection_row,
            )?;
            load_grants(&db, &mut connection)?;
            connection
        };
        let scope = self.account(&connection.account)?;
        // Confirm the connection remains active after opening the account.
        let db = self.db()?;
        let active: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM connections WHERE id = ?1)",
            [&connection.id],
            |row| row.get(0),
        )?;
        if !active {
            return Err(AccessError::Unauthorized);
        }
        Principal::new(scope, connection)
    }
}

fn digest(bearer: &str) -> [u8; 32] {
    Sha256::digest(bearer.as_bytes()).into()
}

pub(crate) fn find_match<'a>(
    hashes: impl IntoIterator<Item = &'a [u8; 32]>,
    provided: &[u8; 32],
    mut compare: impl FnMut(&[u8; 32], &[u8; 32]) -> Choice,
) -> Option<usize> {
    let mut matched = u64::MAX;
    for (index, hash) in hashes.into_iter().enumerate() {
        matched.conditional_assign(&(index as u64), compare(hash, provided));
    }
    (matched != u64::MAX).then_some(matched as usize)
}

fn connection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Connection> {
    Ok(Connection {
        id: row.get(0)?,
        account: row.get(1)?,
        name: row.get(2)?,
        read_only: row.get(3)?,
        grants: Vec::new(),
    })
}

fn load_grants(db: &Database, connection: &mut Connection) -> Result<(), AccessError> {
    let mut stmt =
        db.prepare("SELECT space, write FROM grants WHERE connection = ?1 ORDER BY space")?;
    connection.grants = stmt
        .query_map([&connection.id], |row| {
            Ok(Grant {
                space: row.get(0)?,
                write: row.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(())
}
