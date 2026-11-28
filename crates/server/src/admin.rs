use crate::admin_html::{csrf_matches, dashboard, Dashboard, Feedback};
use crate::config::Config;
use crate::error::ServerError;
use crate::oauth::{InvitationRights, OAuth};
use crate::texts::{for_request, Texts};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Form, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use pensine_access::{AccessError, Account, Accounts, Grant};
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
            let user = if name.is_empty() {
                account.id.clone()
            } else {
                name.to_owned()
            };
            request.extensions_mut().insert(AdminUser {
                account,
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
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
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
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
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
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
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
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
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
    if !csrf_matches(&headers, &fields) {
        return Ok(StatusCode::FORBIDDEN.into_response());
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
        data_dir: &state.data_dir,
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
