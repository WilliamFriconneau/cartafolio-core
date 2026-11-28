use crate::admin_html::{
    csrf_matches, disk_used, grants_from_form, new_csrf, page, render_alias, render_connections,
    render_oauth, space_depth,
};
use crate::error::ServerError;
use crate::html::escape_html;
use crate::oauth::OAuth;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Form, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use pensine_access::{AccessError, Account, Accounts};
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
    pub public_url: Option<String>,
    pub version: &'static str,
}

pub(crate) fn router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/connections",
            get(connections_page).post(connections_create),
        )
        .route("/alias", post(alias_change))
        .route("/connections/revoke", post(connection_revoke))
        .route("/oauth", get(oauth_page).post(oauth_invite))
        .route("/oauth/revoke", post(oauth_revoke))
        .route("/spaces", get(spaces_page))
        .route("/status", get(status_page))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), admin_guard))
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
        return StatusCode::FORBIDDEN.into_response();
    };
    let name = headers
        .get("x-remote-user-name")
        .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
        .map(str::trim)
        .unwrap_or("");
    match state.accounts.account_for_ha_user(ha_id, name) {
        Ok(account) => {
            request.extensions_mut().insert(account);
            next.run(request).await
        }
        Err(error) => {
            tracing::error!(error = %error, "could not resolve the admin account");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn connections_page(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
) -> Result<Response, ServerError> {
    render_connections_page(&state, &account, None)
}

async fn connections_create(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let name = fields
        .get("name")
        .map(String::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err(ServerError::Invalid("A connection name is required".into()));
    }
    let spaces = account.store.spaces()?;
    let (grants, read_only) = grants_from_form(&fields, &spaces);
    let (_connection, key) =
        state
            .accounts
            .connection_create(&account.id, &name, grants, read_only)?;
    let csrf = new_csrf()?;
    let body = format!(
        "<h1>Connection created</h1><p>Name: {}</p>\
         <p>This key is shown once. Copy it now; it cannot be shown again.</p>\
         <p><code>{}</code></p><p><a href=\"connections\">Back to connections</a></p>",
        escape_html(&name),
        escape_html(key.as_str()),
    );
    page("Connection created", &body, &csrf)
}

async fn connection_revoke(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let id = fields.get("id").map(String::as_str).unwrap_or("");
    match state.accounts.connection_revoke(&account.id, id) {
        Ok(()) | Err(AccessError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    render_connections_page(&state, &account, None)
}

async fn oauth_page(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
) -> Result<Response, ServerError> {
    render_oauth_page(&state, &account).await
}

async fn oauth_invite(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let code = state.oauth.invitation(&account.id)?;
    let csrf = new_csrf()?;
    let body = format!(
        "<h1>Code created</h1><p>This one-time code is valid for 10 minutes.</p>\
         <p><code>{}</code></p><p><a href=\"oauth\">Back to OAuth</a></p>",
        escape_html(&code),
    );
    page("Code created", &body, &csrf)
}

async fn oauth_revoke(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let id = fields.get("id").map(String::as_str).unwrap_or("");
    match state.oauth.revoke_session(&account.id, id) {
        Ok(()) | Err(ServerError::Invalid(_)) => {}
        Err(error) => return Err(error),
    }
    render_oauth_page(&state, &account).await
}

async fn spaces_page(
    State(_state): State<AdminState>,
    Extension(account): Extension<Account>,
) -> Result<Response, ServerError> {
    let csrf = new_csrf()?;
    let spaces = account.store.spaces()?;
    let mut body = String::from("<h1>Spaces</h1><ul>");
    for space in &spaces {
        let depth = space_depth(space, &spaces);
        body.push_str(&format!(
            "<li>{}{} — {} notes, {} files</li>",
            "&nbsp;&nbsp;".repeat(depth),
            escape_html(&space.label),
            space.notes,
            space.files,
        ));
    }
    body.push_str("</ul>");
    page("Spaces", &body, &csrf)
}

async fn status_page(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
) -> Result<Response, ServerError> {
    let csrf = new_csrf()?;
    let spaces = account.store.spaces()?;
    let notes: u64 = spaces.iter().map(|space| space.notes).sum();
    let files: u64 = spaces.iter().map(|space| space.files).sum();
    let body = format!(
        "<h1>Status</h1><dl>\
         <dt>Version</dt><dd>{}</dd>\
         <dt>Notes</dt><dd>{}</dd>\
         <dt>Files</dt><dd>{}</dd>\
         <dt>Disk used</dt><dd>{} bytes</dd></dl>",
        escape_html(state.version),
        notes,
        files,
        disk_used(&state.data_dir),
    );
    page("Status", &body, &csrf)
}

async fn alias_change(
    State(state): State<AdminState>,
    Extension(account): Extension<Account>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Result<Response, ServerError> {
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let alias = fields.get("alias").map_or("", String::as_str);
    match state.accounts.set_alias(&account.id, alias) {
        Ok(()) => render_connections_page(&state, &account, None),
        Err(AccessError::Invalid(message)) => {
            let mut response = render_connections_page(&state, &account, Some(&message))?;
            *response.status_mut() = StatusCode::BAD_REQUEST;
            Ok(response)
        }
        Err(error) => Err(error.into()),
    }
}

fn render_connections_page(
    state: &AdminState,
    account: &Account,
    alias_error: Option<&str>,
) -> Result<Response, ServerError> {
    let csrf = new_csrf()?;
    let connections = state.accounts.connections(&account.id)?;
    let spaces = account.store.spaces()?;
    let alias = state.accounts.alias_of(&account.id)?;
    let alias_form = render_alias(state.public_url.as_deref(), &alias, alias_error, &csrf);
    page(
        "Connections",
        &render_connections(&account.id, &alias_form, &connections, &spaces, &csrf),
        &csrf,
    )
}

async fn render_oauth_page(state: &AdminState, account: &Account) -> Result<Response, ServerError> {
    let csrf = new_csrf()?;
    let sessions = state.oauth.sessions(&account.id)?;
    page("OAuth", &render_oauth(&sessions, &csrf), &csrf)
}
