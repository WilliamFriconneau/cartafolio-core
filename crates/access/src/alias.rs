use crate::{AccessError, Account, Accounts};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

const MAX_LEN: usize = 32;
const RESERVED: &[&str] = &[
    "mcp",
    "admin",
    "authorize",
    "token",
    "register",
    "healthz",
    "oauth",
    "static",
    "assets",
    "api",
    "well-known",
    "www",
];

pub fn validate_alias(alias: &str) -> Result<(), AccessError> {
    if !(2..=MAX_LEN).contains(&alias.len())
        || alias.starts_with('-')
        || alias.ends_with('-')
        || !alias
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(AccessError::Invalid(
            "An alias must be 2 to 32 lowercase ASCII letters, digits, or hyphens, with no leading or trailing hyphen".into(),
        ));
    }
    if RESERVED.contains(&alias) {
        return Err(AccessError::Invalid("This alias is reserved".into()));
    }
    Ok(())
}

impl Accounts {
    pub fn set_alias(&self, account: &str, alias: &str) -> Result<(), AccessError> {
        validate_alias(alias)?;
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let owner = alias_owner(&tx, alias)?;
        if owner.as_deref().is_some_and(|owner| owner != account) {
            return Err(AccessError::Invalid("This alias is already taken".into()));
        }
        if tx.execute(
            "UPDATE account_aliases SET alias = ?2 WHERE account = ?1",
            (account, alias),
        )? == 0
        {
            return Err(AccessError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn alias_of(&self, account: &str) -> Result<String, AccessError> {
        self.db()?
            .query_row(
                "SELECT alias FROM account_aliases WHERE account = ?1",
                [account],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(AccessError::NotFound)
    }

    pub fn account_by_alias(&self, alias: &str) -> Result<Account, AccessError> {
        let id = {
            let db = self.db()?;
            alias_owner(&db, alias)?.ok_or(AccessError::NotFound)?
        };
        self.account(&id)
    }
}

fn alias_owner(db: &Connection, alias: &str) -> Result<Option<String>, AccessError> {
    Ok(db
        .query_row(
            "SELECT account FROM account_aliases WHERE alias = ?1",
            [alias],
            |row| row.get(0),
        )
        .optional()?)
}

fn slug(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for c in name
        .nfkd()
        .filter(|&c| !is_combining_mark(c))
        .flat_map(char::to_lowercase)
    {
        if c.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(c);
            if slug.len() >= MAX_LEN {
                break;
            }
            separator = false;
        } else {
            separator = true;
        }
    }
    slug.truncate(MAX_LEN);
    slug.trim_end_matches('-').to_owned()
}

pub(crate) fn insert_default_alias(
    db: &Connection,
    id: &str,
    name: &str,
) -> Result<(), AccessError> {
    let preferred = slug(name);
    let fallback = if validate_alias(id).is_ok() {
        id.to_owned()
    } else {
        slug(id)
    };
    let base = if validate_alias(&preferred).is_ok() {
        preferred
    } else if validate_alias(&fallback).is_ok() {
        fallback
    } else {
        format!("account-{fallback}")
            .trim_end_matches('-')
            .to_owned()
    };
    let mut candidate = base.clone();
    for number in 2_u64.. {
        if db.execute(
            "INSERT INTO account_aliases (account, alias) VALUES (?1, ?2)
             ON CONFLICT(alias) DO NOTHING",
            (id, &candidate),
        )? > 0
        {
            return Ok(());
        }
        let suffix = format!("-{number}");
        let prefix = &base[..base.len().min(MAX_LEN - suffix.len())];
        candidate = format!("{}{suffix}", prefix.trim_end_matches('-'));
    }
    Err(AccessError::Conflict)
}

pub(crate) fn backfill_aliases(db: &Connection) -> Result<(), AccessError> {
    let mut statement = db.prepare(
        "SELECT a.id, COALESCE(h.name, a.id) FROM accounts a
         LEFT JOIN ha_users h ON h.account_id = a.id ORDER BY a.id",
    )?;
    let accounts = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, name) in accounts {
        insert_default_alias(db, &id, &name)?;
    }
    Ok(())
}
