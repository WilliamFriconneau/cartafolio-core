mod consent;
mod routes;
mod token;

pub(crate) use routes::routes;

use crate::config::parse_public_origin;
use crate::{Authenticator, Config, ServerError};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use pensine_access::validate_alias;
use pensine_access::{Accounts, Connection, Grant, Principal};
use rand::{rngs::OsRng, TryRngCore};
use rusqlite::{Connection as Database, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const INVITE_SELECTOR_LEN: usize = 12;
const INVITE_ATTEMPT_LIMIT: u32 = 5;
const INVITE_TTL: i64 = 600;
const CODE_TTL: i64 = 300;
const ACCESS_TTL: i64 = 3600;
const REFRESH_TTL: i64 = 90 * 86400;

type Fields = HashMap<String, String>;
type Result<T> = std::result::Result<T, OAuthError>;

pub(crate) struct OAuth {
    accounts: Arc<Accounts>,
    db: Mutex<Database>,
    config: Config,
    fallback_issuer: String,
}

impl OAuth {
    pub(crate) fn open(
        config: Config,
        accounts: Arc<Accounts>,
        fallback_issuer: String,
    ) -> std::result::Result<Self, ServerError> {
        if config
            .public_url
            .as_deref()
            .is_some_and(|url| parse_public_origin(url).is_none())
        {
            return Err(ServerError::Invalid(
                "public_url must be an HTTPS or loopback origin".into(),
            ));
        }
        let mut db = Database::open(config.data_dir.join("oauth.db")).map_err(db_error)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(db_error)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS oauth_clients (
                id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, redirects TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS oauth_invites (
                hash BLOB PRIMARY KEY NOT NULL, account TEXT NOT NULL, expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS oauth_consent (
                hash BLOB PRIMARY KEY NOT NULL, account TEXT NOT NULL, request TEXT NOT NULL,
                expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS oauth_codes (
                hash BLOB PRIMARY KEY NOT NULL, client TEXT NOT NULL, redirect TEXT NOT NULL,
                challenge TEXT NOT NULL, scope TEXT NOT NULL, expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS oauth_sessions (
                access_hash BLOB PRIMARY KEY NOT NULL, refresh_hash BLOB UNIQUE NOT NULL,
                client TEXT NOT NULL, scope TEXT NOT NULL, access_until INTEGER NOT NULL,
                refresh_until INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS oauth_session_family ON oauth_sessions (client, scope);
             CREATE TABLE IF NOT EXISTS oauth_rotated_refresh (
                hash BLOB PRIMARY KEY NOT NULL, client TEXT NOT NULL, scope TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS oauth_rotated_family ON oauth_rotated_refresh (client, scope);",
        )
        .map_err(db_error)?;
        let has_invite_selector: bool = tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info('oauth_invites') WHERE name = 'selector')",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !has_invite_selector {
            tx.execute_batch(
                "ALTER TABLE oauth_invites ADD COLUMN selector BLOB;
                 ALTER TABLE oauth_invites ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0;
                 CREATE UNIQUE INDEX oauth_invite_selector ON oauth_invites (selector);",
            )
            .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(Self {
            accounts,
            db: Mutex::new(db),
            config,
            fallback_issuer,
        })
    }

    pub(crate) fn issuer(&self) -> String {
        self.config.public_url().map_or_else(
            || self.fallback_issuer.clone(),
            |url| url.trim_end_matches('/').to_owned(),
        )
    }

    fn db(&self) -> std::result::Result<MutexGuard<'_, Database>, ServerError> {
        self.db
            .lock()
            .map_err(|_| ServerError::Invalid("OAuth storage unavailable".into()))
    }

    pub(crate) fn invitation(&self, account: &str) -> std::result::Result<String, ServerError> {
        self.accounts.account(account)?;
        let invite = random_secret()?;
        let db = self.db()?;
        db.execute(
            "DELETE FROM oauth_invites WHERE expires_at <= unixepoch()",
            [],
        )
        .map_err(db_error)?;
        db.execute(
            "INSERT INTO oauth_invites (hash, account, expires_at, selector)
             VALUES (?1, ?2, unixepoch() + ?3, ?4)",
            (hash(&invite), account, INVITE_TTL, invite_selector(&invite)),
        )
        .map_err(db_error)?;
        Ok(invite)
    }

    fn principal(&self, bearer: &str) -> Result<Option<Principal>> {
        let scope = self
            .db()?
            .query_row(
                "SELECT scope FROM oauth_sessions WHERE access_hash = ?1 AND access_until > unixepoch()",
                [hash(bearer)],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(scope) = scope else {
            return Ok(None);
        };
        let scope: Scope = serde_json::from_str(&scope)?;
        let account = self
            .accounts
            .account(&scope.account)
            .map_err(ServerError::from)?;
        let connection = Connection {
            id: scope.id,
            account: scope.account,
            name: scope.name,
            grants: scope
                .spaces
                .into_iter()
                .map(|space| Grant {
                    space,
                    write: !scope.read_only,
                })
                .collect(),
            read_only: scope.read_only,
        };
        Ok(Some(
            Principal::new(account, connection).map_err(ServerError::from)?,
        ))
    }

    pub(crate) fn sessions(
        &self,
        account: &str,
    ) -> std::result::Result<Vec<SessionInfo>, ServerError> {
        let db = self.db()?;
        let mut stmt = db
            .prepare("SELECT client, scope, refresh_until FROM oauth_sessions")
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        drop(stmt);
        let mut sessions = Vec::new();
        for (client_id, scope_json, refresh_until) in rows {
            let scope: Scope = match serde_json::from_str(&scope_json) {
                Ok(scope) => scope,
                Err(_) => continue,
            };
            if scope.account != account {
                continue;
            }
            let client = db
                .query_row(
                    "SELECT name FROM oauth_clients WHERE id = ?1",
                    [&client_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(db_error)
                .unwrap_or(client_id);
            sessions.push(SessionInfo {
                id: scope.id,
                name: scope.name,
                client,
                spaces: scope.spaces,
                read_only: scope.read_only,
                refresh_until,
            });
        }
        Ok(sessions)
    }

    pub(crate) fn revoke_session(
        &self,
        account: &str,
        id: &str,
    ) -> std::result::Result<(), ServerError> {
        let db = self.db()?;
        let mut stmt = db
            .prepare("SELECT access_hash, refresh_hash, scope FROM oauth_sessions")
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        drop(stmt);
        for (access_hash, refresh_hash, scope_json) in rows {
            let scope: Scope = match serde_json::from_str(&scope_json) {
                Ok(scope) => scope,
                Err(_) => continue,
            };
            if scope.account == account && scope.id == id {
                db.execute(
                    "DELETE FROM oauth_sessions WHERE access_hash = ?1 AND refresh_hash = ?2",
                    (access_hash, refresh_hash),
                )
                .map_err(db_error)?;
                return Ok(());
            }
        }
        Err(ServerError::Invalid("OAuth session not found".into()))
    }
}

impl Authenticator for OAuth {
    fn authenticate(&self, bearer: &str) -> Option<Principal> {
        self.principal(bearer).ok().flatten()
    }
}

#[derive(Serialize, Deserialize)]
struct Scope {
    id: String,
    account: String,
    name: String,
    spaces: Vec<String>,
    read_only: bool,
}

pub(crate) struct SessionInfo {
    pub id: String,
    pub name: String,
    pub client: String,
    pub spaces: Vec<String>,
    pub read_only: bool,
    pub refresh_until: i64,
}

fn resource_matches(oauth: &OAuth, fields: &Fields) -> Result<()> {
    if fields.get("resource").is_some_and(|resource| {
        let base = format!("{}/mcp", oauth.issuer());
        resource != &base
            && !resource
                .strip_prefix(&base)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .is_some_and(|alias| validate_alias(alias).is_ok())
    }) {
        return Err(OAuthError::BadRequest("invalid_target"));
    }
    Ok(())
}

fn field<'a>(fields: &'a Fields, key: &str) -> &'a str {
    fields.get(key).map_or("", String::as_str)
}

fn random_secret() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(std::io::Error::other)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn invite_selector(value: &str) -> Option<[u8; 32]> {
    // The prefix identifies the invitation without counting failures against unrelated codes.
    value.get(..INVITE_SELECTOR_LEN).map(hash)
}

enum OAuthError {
    BadRequest(&'static str),
    Forbidden,
    Internal,
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::BadRequest(error) => (StatusCode::BAD_REQUEST, error),
            Self::Forbidden => (StatusCode::FORBIDDEN, "access_denied"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        };
        (status, Json(json!({"error": error}))).into_response()
    }
}

macro_rules! internal_error_from {
    ($($ty:ty),+ $(,)?) => {
        $(impl From<$ty> for OAuthError {
            fn from(_: $ty) -> Self {
                Self::Internal
            }
        })+
    };
}

internal_error_from!(
    ServerError,
    rusqlite::Error,
    serde_json::Error,
    std::io::Error,
    std::fmt::Error
);

fn db_error(error: rusqlite::Error) -> ServerError {
    ServerError::Io(std::io::Error::other(error))
}
