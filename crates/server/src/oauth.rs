use crate::html::escape_html;
use crate::{Authenticator, ServerError};
use axum::extract::{DefaultBodyLimit, Form, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use pensine_access::{Accounts, Connection, Grant, Principal};
use rand::{rngs::OsRng, TryRngCore};
use rusqlite::{Connection as Database, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::{Host, Url};

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
    issuer: String,
    origin: String,
    resource: String,
}

impl OAuth {
    pub(crate) fn open(
        root: &Path,
        accounts: Arc<Accounts>,
        issuer: &str,
    ) -> std::result::Result<Self, ServerError> {
        let issuer = issuer.trim_end_matches('/');
        let url = parse_web_url(issuer)
            .filter(|url| url.path() == "/" && url.query().is_none())
            .ok_or_else(|| {
                ServerError::Invalid("public_url must be an HTTPS or loopback origin".into())
            })?;
        let mut db = Database::open(root.join("oauth.db")).map_err(db_error)?;
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
            issuer: issuer.into(),
            origin: url.origin().ascii_serialization(),
            resource: format!("{issuer}/mcp"),
        })
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

pub(crate) fn routes(oauth: Arc<OAuth>) -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/authorize", get(authorize).post(consent))
        .route("/token", post(token))
        .route_layer(middleware::from_fn(oauth_no_store))
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(oauth)
}

async fn protected_resource(State(oauth): State<Arc<OAuth>>) -> Json<Value> {
    Json(json!({
        "resource": oauth.resource,
        "authorization_servers": [oauth.issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["memory"],
    }))
}

async fn authorization_server(State(oauth): State<Arc<OAuth>>) -> Json<Value> {
    Json(json!({
        "issuer": oauth.issuer,
        "authorization_endpoint": format!("{}/authorize", oauth.issuer),
        "token_endpoint": format!("{}/token", oauth.issuer),
        "registration_endpoint": format!("{}/register", oauth.issuer),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
        "scopes_supported": ["memory"],
    }))
}

#[derive(Deserialize)]
struct Registration {
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    token_endpoint_auth_method: Option<String>,
}

async fn register(
    State(oauth): State<Arc<OAuth>>,
    Json(registration): Json<Registration>,
) -> Result<Response> {
    let redirects = registration.redirect_uris;
    if redirects.is_empty()
        || redirects.iter().any(|uri| {
            parse_web_url(uri).is_none_or(|url| {
                url.query_pairs()
                    .any(|(key, _)| matches!(key.as_ref(), "code" | "error" | "state" | "iss"))
            })
        })
    {
        return Err(OAuthError::BadRequest("invalid_redirect_uri"));
    }
    if registration
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err(OAuthError::BadRequest("invalid_client_metadata"));
    }
    let name = registration
        .client_name
        .unwrap_or_else(|| "OAuth client".into());
    let id = random_secret()?;
    oauth.db()?.execute(
        "INSERT INTO oauth_clients VALUES (?1, ?2, ?3)",
        (&id, &name, serde_json::to_string(&redirects)?),
    )?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "client_id": id,
            "client_name": name,
            "redirect_uris": redirects,
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        })),
    )
        .into_response())
}

#[derive(Serialize, Deserialize)]
struct Authorization {
    client: String,
    name: String,
    redirect: String,
    challenge: String,
    state: Option<String>,
}

impl Authorization {
    fn validate(oauth: &OAuth, fields: &Fields) -> Result<Self> {
        let client = field(fields, "client_id");
        let redirect = field(fields, "redirect_uri");
        let challenge = field(fields, "code_challenge");
        let row: Option<(String, String)> = oauth
            .db()?
            .query_row(
                "SELECT name, redirects FROM oauth_clients WHERE id = ?1",
                [client],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (name, redirects) = row.ok_or(OAuthError::BadRequest("invalid_client"))?;
        let redirects: Vec<String> = serde_json::from_str(&redirects)?;
        if !redirects.iter().any(|registered| registered == redirect) {
            return Err(OAuthError::BadRequest("invalid_redirect_uri"));
        }
        resource_matches(oauth, fields)?;
        if field(fields, "response_type") != "code"
            || field(fields, "code_challenge_method") != "S256"
            || challenge.len() != 43
            || !URL_SAFE_NO_PAD
                .decode(challenge)
                .is_ok_and(|bytes| bytes.len() == 32)
        {
            return Err(OAuthError::BadRequest("invalid_request"));
        }
        if fields
            .get("scope")
            .is_some_and(|scope| scope != "memory" && !scope.is_empty())
        {
            return Err(OAuthError::BadRequest("invalid_scope"));
        }
        Ok(Self {
            client: client.into(),
            name,
            redirect: redirect.into(),
            challenge: challenge.into(),
            state: fields.get("state").cloned(),
        })
    }

    fn callback(&self, oauth: &OAuth, key: &str, value: &str) -> Result<Response> {
        let mut url = Url::parse(&self.redirect)
            .map_err(|_| OAuthError::BadRequest("invalid_redirect_uri"))?;
        let mut query = url.query_pairs_mut();
        query
            .append_pair(key, value)
            .append_pair("iss", &oauth.issuer);
        if let Some(state) = &self.state {
            query.append_pair("state", state);
        }
        drop(query);
        let location = HeaderValue::from_str(url.as_str())
            .map_err(|_| OAuthError::BadRequest("invalid_redirect_uri"))?;
        Ok((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
    }
}

async fn authorize(
    State(oauth): State<Arc<OAuth>>,
    Query(fields): Query<Fields>,
) -> Result<Response> {
    let request = Authorization::validate(&oauth, &fields)?;
    let mut page = format!(
        "<h1>Connect to {}</h1><form method=\"post\" action=\"/authorize\">",
        escape_html(&request.name)
    );
    for key in [
        "client_id",
        "redirect_uri",
        "state",
        "code_challenge",
        "code_challenge_method",
        "response_type",
        "resource",
        "scope",
    ] {
        if let Some(value) = fields.get(key) {
            hidden(&mut page, key, value)?;
        }
    }
    page.push_str("<label>One-time code <input name=\"invite\" required></label><button>Continue</button></form>");
    Ok(consent_page(page))
}

async fn consent(
    State(oauth): State<Arc<OAuth>>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response> {
    if headers
        .get("sec-fetch-site")
        .is_some_and(|site| site == "cross-site")
        || headers
            .get(header::ORIGIN)
            .is_some_and(|origin| origin.as_bytes() != oauth.origin.as_bytes())
    {
        return Err(OAuthError::Forbidden);
    }
    if fields.contains_key("nonce") {
        decide(&oauth, &fields)
    } else {
        choose_spaces(&oauth, &fields)
    }
}

fn choose_spaces(oauth: &OAuth, fields: &Fields) -> Result<Response> {
    let request = Authorization::validate(oauth, fields)?;
    let mut db = oauth.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let invite = field(fields, "invite");
    let invite_hash = hash(invite);
    let account = tx
        .query_row(
            "DELETE FROM oauth_invites WHERE hash = ?1 AND expires_at > unixepoch()
             AND failed_attempts < ?2 RETURNING account",
            (invite_hash, INVITE_ATTEMPT_LIMIT),
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(account) = account else {
        tx.execute(
            "UPDATE oauth_invites SET failed_attempts = failed_attempts + 1
             WHERE selector = ?1 AND hash != ?2 AND expires_at > unixepoch()
             AND failed_attempts < ?3",
            (invite_selector(invite), invite_hash, INVITE_ATTEMPT_LIMIT),
        )?;
        tx.commit()?;
        return Err(OAuthError::Forbidden);
    };
    let spaces = oauth
        .accounts
        .account(&account)
        .map_err(ServerError::from)?
        .store
        .spaces()
        .map_err(std::io::Error::other)?;
    let nonce = random_secret()?;
    let mut page = format!(
        "<h1>Choose spaces for {}</h1><form method=\"post\" action=\"/authorize\">",
        escape_html(&request.name)
    );
    hidden(&mut page, "nonce", &nonce)?;
    hidden(&mut page, "client_id", &request.client)?;
    hidden(&mut page, "redirect_uri", &request.redirect)?;
    if let Some(state) = &request.state {
        hidden(&mut page, "state", state)?;
    }
    for space in spaces {
        write!(
            page,
            "<label><input type=\"checkbox\" name=\"v_{}\" value=\"1\"> {}</label><br>",
            escape_html(&space.name),
            escape_html(&space.label)
        )?;
    }
    page.push_str("<label><input type=\"checkbox\" name=\"readonly\" value=\"1\"> Read only</label><br>        <button name=\"decision\" value=\"allow\">Allow</button>         <button name=\"decision\" value=\"deny\">Deny</button></form>");
    tx.execute(
        "DELETE FROM oauth_consent WHERE expires_at <= unixepoch()",
        [],
    )?;
    tx.execute(
        "INSERT INTO oauth_consent VALUES (?1, ?2, ?3, unixepoch() + ?4)",
        (
            hash(&nonce),
            account,
            serde_json::to_string(&request)?,
            INVITE_TTL,
        ),
    )?;
    tx.commit()?;
    Ok(consent_page(page))
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

fn decide(oauth: &OAuth, fields: &Fields) -> Result<Response> {
    let mut db = oauth.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (account, request): (String, String) = tx.query_row(
        "DELETE FROM oauth_consent WHERE hash = ?1 AND expires_at > unixepoch() RETURNING account, request",
        [hash(field(fields, "nonce"))],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?.ok_or(OAuthError::Forbidden)?;
    let request: Authorization = serde_json::from_str(&request)?;
    let response = match field(fields, "decision") {
        "deny" => request.callback(oauth, "error", "access_denied")?,
        "allow" => {
            let spaces: Vec<_> = oauth
                .accounts
                .account(&account)
                .map_err(ServerError::from)?
                .store
                .spaces()
                .map_err(std::io::Error::other)?
                .into_iter()
                .filter(|space| field(fields, &format!("v_{}", space.name)) == "1")
                .map(|space| space.name)
                .collect();
            if spaces.is_empty() {
                return Err(OAuthError::BadRequest("select_at_least_one_space"));
            }
            let scope = Scope {
                id: random_secret()?,
                account,
                name: request.name.clone(),
                spaces,
                read_only: field(fields, "readonly") == "1",
            };
            let code = random_secret()?;
            tx.execute(
                "DELETE FROM oauth_codes WHERE expires_at <= unixepoch()",
                [],
            )?;
            tx.execute(
                "INSERT INTO oauth_codes VALUES (?1, ?2, ?3, ?4, ?5, unixepoch() + ?6)",
                (
                    hash(&code),
                    &request.client,
                    &request.redirect,
                    &request.challenge,
                    serde_json::to_string(&scope)?,
                    CODE_TTL,
                ),
            )?;
            request.callback(oauth, "code", &code)?
        }
        _ => return Err(OAuthError::BadRequest("invalid_request")),
    };
    tx.commit()?;
    Ok(response)
}

async fn token(State(oauth): State<Arc<OAuth>>, Form(fields): Form<Fields>) -> Result<Response> {
    resource_matches(&oauth, &fields)?;
    let client = field(&fields, "client_id");
    let mut db = oauth.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let scope: String = match field(&fields, "grant_type") {
        "authorization_code" => {
            let code = hash(field(&fields, "code"));
            let (challenge, scope): (String, String) = tx
                .query_row(
                    "DELETE FROM oauth_codes WHERE hash = ?1 AND client = ?2 AND redirect = ?3
                    AND expires_at > unixepoch() RETURNING challenge, scope",
                    (code, client, field(&fields, "redirect_uri")),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or(OAuthError::BadRequest("invalid_grant"))?;
            let verifier = field(&fields, "code_verifier");
            if !(43..=128).contains(&verifier.len())
                || !verifier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                || !bool::from(
                    challenge
                        .as_bytes()
                        .ct_eq(URL_SAFE_NO_PAD.encode(hash(verifier)).as_bytes()),
                )
            {
                return Err(OAuthError::BadRequest("invalid_grant"));
            }
            scope
        }
        "refresh_token" => {
            let refresh_hash = hash(field(&fields, "refresh_token"));
            let Some(scope) = rotate_refresh(&tx, client, refresh_hash)? else {
                tx.commit()?;
                return Err(OAuthError::BadRequest("invalid_grant"));
            };
            scope
        }
        _ => return Err(OAuthError::BadRequest("unsupported_grant_type")),
    };
    let access = random_secret()?;
    let refresh = random_secret()?;
    let expired = tx.execute(
        "DELETE FROM oauth_sessions WHERE refresh_until <= unixepoch()",
        [],
    )?;
    tx.execute(
        "INSERT INTO oauth_sessions VALUES (?1, ?2, ?3, ?4, unixepoch() + ?5, unixepoch() + ?6)",
        (
            hash(&access),
            hash(&refresh),
            client,
            scope,
            ACCESS_TTL,
            REFRESH_TTL,
        ),
    )?;
    if expired > 0 {
        prune_rotated_refreshes(&tx)?;
    }
    tx.commit()?;
    Ok(Json(json!({
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL,
        "scope": "memory",
    }))
    .into_response())
}

fn rotate_refresh(
    tx: &Transaction<'_>,
    client: &str,
    refresh_hash: [u8; 32],
) -> Result<Option<String>> {
    let scope: Option<String> = tx
        .query_row(
            "DELETE FROM oauth_sessions WHERE refresh_hash = ?1 AND client = ?2
             AND refresh_until > unixepoch() RETURNING scope",
            (refresh_hash, client),
            |row| row.get(0),
        )
        .optional()?;
    if let Some(scope) = &scope {
        tx.execute(
            "INSERT INTO oauth_rotated_refresh (hash, client, scope) VALUES (?1, ?2, ?3)",
            (refresh_hash, client, scope),
        )?;
    } else if let Some(reused_scope) = tx
        .query_row(
            "SELECT scope FROM oauth_rotated_refresh WHERE hash = ?1 AND client = ?2",
            (refresh_hash, client),
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        // The scope contains a unique ID minted when consent is granted.
        tx.execute(
            "DELETE FROM oauth_sessions WHERE client = ?1 AND scope = ?2",
            (client, &reused_scope),
        )?;
        tx.execute(
            "DELETE FROM oauth_rotated_refresh WHERE client = ?1 AND scope = ?2",
            (client, &reused_scope),
        )?;
    }
    Ok(scope)
}

fn prune_rotated_refreshes(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "DELETE FROM oauth_rotated_refresh AS rotated WHERE NOT EXISTS (
            SELECT 1 FROM oauth_sessions AS active
            WHERE active.client = rotated.client AND active.scope = rotated.scope
         )",
        [],
    )?;
    Ok(())
}

fn resource_matches(oauth: &OAuth, fields: &Fields) -> Result<()> {
    if fields
        .get("resource")
        .is_some_and(|resource| resource != &oauth.resource)
    {
        return Err(OAuthError::BadRequest("invalid_target"));
    }
    Ok(())
}

fn field<'a>(fields: &'a Fields, key: &str) -> &'a str {
    fields.get(key).map_or("", String::as_str)
}

fn parse_web_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let loopback = match url.host()? {
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
        Host::Domain(domain) => domain == "localhost",
    };
    ((url.scheme() == "https" || url.scheme() == "http" && loopback)
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
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

fn hidden(page: &mut String, name: &str, value: &str) -> std::fmt::Result {
    write!(
        page,
        "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
        escape_html(value)
    )
}

fn consent_page(body: String) -> Response {
    (
        [
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; frame-ancestors 'none'; base-uri 'none'"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html(format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Pensine consent</title>{body}</html>")),
    )
        .into_response()
}

async fn oauth_no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
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
