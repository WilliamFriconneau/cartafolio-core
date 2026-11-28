use super::{
    field, hash, invite_selector, random_secret, resource_matches, Fields, InvitationRights, OAuth,
    OAuthError, Result, Scope, CODE_TTL, INVITE_ATTEMPT_LIMIT, INVITE_TTL,
};
use crate::config::parse_public_origin;
use crate::html::escape_html;
use crate::texts::{for_request, Texts};
use crate::ServerError;
use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use pensine_store::SpaceInfo;
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
    #[serde(default)]
    rights: Option<InvitationRights>,
}

impl Authorization {
    fn validate(oauth: &OAuth, fields: &Fields) -> Result<Self> {
        let client = field(fields, "client_id");
        let redirect = field(fields, "redirect_uri");
        let challenge = field(fields, "code_challenge");
        let row: Option<(String, String)> = oauth
            .db()?
            .query_row(
                "SELECT name, redirects FROM oauth_clients WHERE id = ?1 AND last_used > unixepoch() - 30 * 86400",
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
            rights: None,
        })
    }

    fn callback(&self, oauth: &OAuth, key: &str, value: &str) -> Result<Response> {
        let mut url = Url::parse(&self.redirect)
            .map_err(|_| OAuthError::BadRequest("invalid_redirect_uri"))?;
        let mut query = url.query_pairs_mut();
        query
            .append_pair(key, value)
            .append_pair("iss", &oauth.issuer());
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
    headers: HeaderMap,
    Query(fields): Query<Fields>,
) -> Result<Response> {
    let request = Authorization::validate(&oauth, &fields)?;
    let t = for_request(&headers);
    let mut page = format!(
        "<h1>{} {}</h1><form method=\"post\" action=\"/authorize\">",
        t.connect_to,
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
    write!(
        page,
        "<label>{} <input name=\"invite\" required></label><button>{}</button></form>",
        t.one_time_code, t.next
    )?;
    Ok(consent_page(page, t))
}

pub(super) async fn consent(
    State(oauth): State<Arc<OAuth>>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response> {
    if headers
        .get("sec-fetch-site")
        .is_some_and(|site| site == "cross-site")
        || headers.get(header::ORIGIN).is_some_and(|origin| {
            parse_public_origin(&oauth.issuer()).is_none_or(|url| {
                origin.as_bytes() != url.origin().ascii_serialization().as_bytes()
            })
        })
    {
        return Err(OAuthError::Forbidden);
    }
    if fields.contains_key("nonce") {
        decide(&oauth, &fields)
    } else {
        present_rights(&oauth, &fields, for_request(&headers))
    }
}

fn present_rights(oauth: &OAuth, fields: &Fields, t: &Texts) -> Result<Response> {
    let mut request = Authorization::validate(oauth, fields)?;
    let mut db = oauth.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let invite = field(fields, "invite");
    let invite_hash = hash(invite);
    let invitation = tx
        .query_row(
            "DELETE FROM oauth_invites WHERE hash = ?1 AND expires_at > unixepoch()
             AND failed_attempts < ?2 RETURNING account, rights",
            (invite_hash, INVITE_ATTEMPT_LIMIT),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let Some((account, rights)) = invitation else {
        tx.execute(
            "UPDATE oauth_invites SET failed_attempts = failed_attempts + 1
             WHERE selector = ?1 AND hash != ?2 AND expires_at > unixepoch()
             AND failed_attempts < ?3",
            (invite_selector(invite), invite_hash, INVITE_ATTEMPT_LIMIT),
        )?;
        tx.commit()?;
        return Err(OAuthError::Forbidden);
    };
    request.rights = rights.as_deref().map(serde_json::from_str).transpose()?;
    let spaces = account_spaces(oauth, &account)?;
    let nonce = random_secret()?;
    let page = render_rights(&request, &spaces, &nonce, t)?;
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
    Ok(consent_page(page, t))
}

fn decide(oauth: &OAuth, fields: &Fields) -> Result<Response> {
    let mut db = oauth.db()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (account, request): (String, String) = tx.query_row(
        "DELETE FROM oauth_consent WHERE hash = ?1 AND expires_at > unixepoch() RETURNING account, request",
        [hash(field(fields, "nonce"))],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?.ok_or(OAuthError::Forbidden)?;
    let mut request: Authorization = serde_json::from_str(&request)?;
    let response = match field(fields, "decision") {
        "deny" => request.callback(oauth, "error", "access_denied")?,
        "allow" => {
            let rights = match request.rights.take() {
                None => legacy_rights(oauth, &account, fields)?,
                Some(rights) => rights,
            };
            let scope = Scope {
                id: random_secret()?,
                account,
                name: request.name.clone(),
                spaces: rights.spaces,
                read_only: rights.read_only,
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

fn account_spaces(oauth: &OAuth, account: &str) -> Result<Vec<SpaceInfo>> {
    Ok(oauth
        .accounts
        .account(account)
        .map_err(ServerError::from)?
        .store
        .spaces()
        .map_err(std::io::Error::other)?)
}

fn legacy_rights(oauth: &OAuth, account: &str, fields: &Fields) -> Result<InvitationRights> {
    let spaces: Vec<_> = account_spaces(oauth, account)?
        .into_iter()
        .filter(|space| field(fields, &format!("v_{}", space.name)) == "1")
        .map(|space| space.name)
        .collect();
    if spaces.is_empty() {
        return Err(OAuthError::BadRequest("select_at_least_one_space"));
    }
    Ok(InvitationRights {
        spaces,
        read_only: field(fields, "readonly") == "1",
    })
}

fn render_rights(
    request: &Authorization,
    spaces: &[SpaceInfo],
    nonce: &str,
    t: &Texts,
) -> Result<String> {
    let heading = if request.rights.is_some() {
        t.rights_for
    } else {
        t.choose_for
    };
    let mut page = format!(
        "<h1>{heading} {}</h1><form method=\"post\" action=\"/authorize\">",
        escape_html(&request.name),
    );
    hidden(&mut page, "nonce", nonce)?;
    hidden(&mut page, "client_id", &request.client)?;
    hidden(&mut page, "redirect_uri", &request.redirect)?;
    if let Some(state) = &request.state {
        hidden(&mut page, "state", state)?;
    }
    if let Some(rights) = &request.rights {
        summarize_rights(&mut page, rights, spaces, t)?;
    } else {
        for space in spaces {
            write!(
                page,
                "<label><input type=\"checkbox\" name=\"v_{}\" value=\"1\"> {}</label><br>",
                escape_html(&space.name),
                escape_html(&space.label)
            )?;
        }
        write!(
            page,
            "<label><input type=\"checkbox\" name=\"readonly\" value=\"1\"> {}</label><br>",
            t.read_only
        )?;
    }
    write!(
        page,
        "<button name=\"decision\" value=\"allow\">{}</button> \
        <button name=\"decision\" value=\"deny\">{}</button></form>",
        t.allow, t.deny
    )?;
    Ok(page)
}

fn summarize_rights(
    page: &mut String,
    rights: &InvitationRights,
    spaces: &[SpaceInfo],
    t: &Texts,
) -> std::fmt::Result {
    let mode = if rights.read_only {
        t.read_only
    } else {
        t.read_write
    };
    write!(page, "<p>{mode}</p><ul>")?;
    for name in &rights.spaces {
        let label = if name == "*" {
            t.all
        } else {
            spaces
                .iter()
                .find(|space| space.name == *name)
                .map_or(name.as_str(), |space| space.label.as_str())
        };
        write!(page, "<li>{}</li>", escape_html(label))?;
    }
    page.push_str("</ul>");
    if !rights.spaces.iter().any(|name| name == "*") {
        write!(page, "<p>{}</p>", t.children)?;
    }
    Ok(())
}

fn hidden(page: &mut String, name: &str, value: &str) -> std::fmt::Result {
    write!(
        page,
        "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
        escape_html(value)
    )
}

fn consent_page(body: String, t: &Texts) -> Response {
    (
        [
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html(format!(
            "<!doctype html><html lang=\"{}\"><meta charset=\"utf-8\">\
            <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
            <title>{}</title>{body}</html>",
            t.lang, t.consent_title
        )),
    )
        .into_response()
}
