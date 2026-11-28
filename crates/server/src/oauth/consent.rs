use super::{
    field, hash, invite_selector, random_secret, resource_matches, Fields, OAuth, OAuthError,
    Result, Scope, CODE_TTL, INVITE_ATTEMPT_LIMIT, INVITE_TTL,
};
use crate::html::escape_html;
use crate::ServerError;
use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::sync::Arc;
use url::Url;

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

pub(super) async fn authorize(
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

pub(super) async fn consent(
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
