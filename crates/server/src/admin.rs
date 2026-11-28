use crate::admin_html::{csrf_matches, dashboard, Dashboard, Feedback};
use crate::config::Config;
use crate::error::ServerError;
use crate::oauth::{InvitationRights, OAuth};
use crate::texts::{for_request, Texts};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Form, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use hmac::{Hmac, Mac};
use pensine_access::{AccessError, Account, Accounts, Grant};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AdminState {
    pub accounts: Arc<Accounts>,
    pub oauth: Arc<OAuth>,
    pub trusted_proxy: IpAddr,
    pub data_dir: PathBuf,
    pub config: Config,
    pub csrf_secret: [u8; 32],
    pub version: &'static str,
}

pub(crate) fn router(state: AdminState) -> Router {
    Router::new()
        .route("/", get(admin_page))
        .route("/connections", get(admin_page).post(connections_create))
        .route("/alias", post(alias_change))
        .route("/connections/revoke", post(connection_revoke))
        .route("/oauth", get(admin_page).post(oauth_invite))
        .route("/oauth/revoke", post(oauth_revoke))
        .route("/spaces", get(admin_page))
        .route("/status", get(admin_page))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), admin_guard))
        .layer(middleware::from_fn(admin_headers))
        .with_state(state)
}

async fn admin_guard(
    State(state): State<AdminState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    if addr.ip() != state.trusted_proxy {
        return StatusCode::FORBIDDEN.into_response();
    }
    let headers = request.headers().clone();
    let ha_id = headers
        .get("x-remote-user-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(ha_id) = ha_id else {
        return refused();
    };
    if request.method() == Method::POST && cross_site(&headers) {
        return refused();
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&state.csrf_secret) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    mac.update(ha_id.as_bytes());
    let csrf = format!("{:x}", mac.finalize().into_bytes());
    let name = headers
        .get("x-remote-user-name")
        .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
        .map(str::trim)
        .unwrap_or("");
    match state.accounts.account_for_ha_user(ha_id, name) {
        Ok(account) => {
            let user = if name.is_empty() {
                account.id.clone()
            } else {
                name.to_owned()
            };
            request.extensions_mut().insert(AdminUser {
                account,
                csrf,
                secure_cookie: forwarded_https(&headers),
                user,
                texts: for_request(&headers),
            });
            next.run(request).await
        }
        Err(error) => {
            tracing::error!(error = %error, "could not resolve the admin account");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Clone)]
struct AdminUser {
    account: Account,
    csrf: String,
    secure_cookie: bool,
    user: String,
    texts: &'static Texts,
}

type Fields = HashMap<String, String>;

async fn admin_page(
    State(state): State<AdminState>,
    Extension(user): Extension<AdminUser>,
) -> Result<Response, ServerError> {
    render(&state, &user, None, Feedback::None)
}

async fn connections_create(
    State(state): State<AdminState>,
    Extension(user): Extension<AdminUser>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields, &user.csrf) {
        return Ok(refused());
    }
    let name = fields.get("name").map_or("", |name| name.trim());
    if name.is_empty() {
        return render(
            &state,
            &user,
            Some(&fields),
            Feedback::Error(user.texts.name_required),
        );
    }
    let (grants, read_only) = if fields.contains_key("scope") {
        let Some(rights) = invitation_rights(&user.account, &fields)? else {
            return render(
                &state,
                &user,
                Some(&fields),
                Feedback::Error(user.texts.select_space),
            );
        };
        let grants = rights
            .spaces
            .into_iter()
            .map(|space| Grant {
                space,
                write: !rights.read_only,
            })
            .collect();
        (grants, rights.read_only)
    } else {
        legacy_key_grants(&user.account, &fields)?
    };
    let (_, key) = state
        .accounts
        .connection_create(&user.account.id, name, grants, read_only)?;
    render(&state, &user, Some(&fields), Feedback::Key(key.as_str()))
}

async fn connection_revoke(
    State(state): State<AdminState>,
    Extension(user): Extension<AdminUser>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields, &user.csrf) {
        return Ok(refused());
    }
    let id = fields.get("id").map_or("", String::as_str);
    match state.accounts.connection_revoke(&user.account.id, id) {
        Ok(()) | Err(AccessError::NotFound) => Ok(Redirect::to("../").into_response()),
        Err(error) => Err(error.into()),
    }
}

async fn oauth_invite(
    State(state): State<AdminState>,
    Extension(user): Extension<AdminUser>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields, &user.csrf) {
        return Ok(refused());
    }
    let Some(rights) = invitation_rights(&user.account, &fields)? else {
        return render(
            &state,
            &user,
            Some(&fields),
            Feedback::Error(user.texts.select_space),
        );
    };
    let code = state
        .oauth
        .scoped_invitation(&user.account.id, Some(&rights))?;
    render(&state, &user, Some(&fields), Feedback::Code(&code))
}

async fn oauth_revoke(
    State(state): State<AdminState>,
    Extension(user): Extension<AdminUser>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields, &user.csrf) {
        return Ok(refused());
    }
    let id = fields.get("id").map_or("", String::as_str);
    match state.oauth.revoke_session(&user.account.id, id) {
        Ok(()) | Err(ServerError::Invalid(_)) => Ok(Redirect::to("../").into_response()),
        Err(error) => Err(error),
    }
}

async fn alias_change(
    State(state): State<AdminState>,
    Extension(user): Extension<AdminUser>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields, &user.csrf) {
        return Ok(refused());
    }
    let alias = fields.get("alias").map_or("", String::as_str);
    match state.accounts.set_alias(&user.account.id, alias) {
        Ok(()) => render(&state, &user, None, Feedback::None),
        Err(AccessError::Invalid(_) | AccessError::Conflict) => render(
            &state,
            &user,
            None,
            Feedback::Error(user.texts.invalid_alias),
        ),
        Err(error) => Err(error.into()),
    }
}

fn render(
    state: &AdminState,
    user: &AdminUser,
    fields: Option<&Fields>,
    feedback: Feedback<'_>,
) -> Result<Response, ServerError> {
    let id = &user.account.id;
    dashboard(Dashboard {
        user: &user.user,
        texts: user.texts,
        public_url: state.config.public_url().as_deref(),
        alias: &state.accounts.alias_of(id)?,
        connections: &state.accounts.connections(id)?,
        sessions: &state.oauth.sessions(id)?,
        spaces: &user.account.store.spaces()?,
        version: state.version,
        data_dir: &state.data_dir.join("accounts").join(id).join("blobs"),
        csrf: &user.csrf,
        secure_cookie: user.secure_cookie,
        fields,
        feedback,
    })
}

fn invitation_rights(
    account: &Account,
    fields: &Fields,
) -> Result<Option<InvitationRights>, ServerError> {
    let spaces = match fields.get("scope").map(String::as_str) {
        Some("all") => vec!["*".into()],
        Some("selected") => account
            .store
            .spaces()?
            .into_iter()
            .filter(|space| checked(fields, &format!("s_{}", space.name)))
            .map(|space| space.name)
            .collect(),
        _ => return Ok(None),
    };
    Ok((!spaces.is_empty()).then(|| InvitationRights {
        spaces,
        read_only: checked(fields, "read_only"),
    }))
}

fn legacy_key_grants(
    account: &Account,
    fields: &Fields,
) -> Result<(Vec<Grant>, bool), ServerError> {
    let read_only = checked(fields, "read_only");
    let mut grants: Vec<_> = account
        .store
        .spaces()?
        .into_iter()
        .filter(|space| checked(fields, &format!("s_{}", space.name)))
        .map(|space| Grant {
            write: !read_only && checked(fields, &format!("w_{}", space.name)),
            space: space.name,
        })
        .collect();
    if grants.is_empty() {
        grants.push(Grant {
            space: "*".into(),
            write: !read_only,
        });
    }
    Ok((grants, read_only))
}

fn checked(fields: &Fields, name: &str) -> bool {
    fields.get(name).is_some_and(|value| value == "1")
}

async fn admin_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(HeaderValue::from_static("frame-ancestors 'self'"));
    response
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .is_some_and(|value| value == "https")
}

/// Browsers say how a form post relates to the page in `Sec-Fetch-Site`. Behind the Home Assistant ingress the `Origin`
/// is Home Assistant's own address, which this server cannot know, so it is not compared (comparing it refused every
/// legitimate post); a request the browser marks `cross-site` is refused, and every post also needs the CSRF token bound
/// to the signed-in user and the strict cookie.
fn cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
}

/// A refusal a person can read: an empty body would be downloaded as a file by the browser.
fn refused() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "Refused: the form has expired or came from another site. Reload the page and try again.\n",
    )
        .into_response()
}
