use crate::oauth::OAuth;
use crate::{Authenticator, Config};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, MatchedPath, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use pensine_access::{validate_alias, AccessError, Principal};
use pensine_mcp::Mcp;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub(super) struct AppState {
    pub config: Config,
    pub mcp: Arc<Mcp>,
    pub authenticators: Arc<Vec<Arc<dyn Authenticator>>>,
    pub oauth: Option<Arc<OAuth>>,
}

pub(super) fn router(state: AppState) -> Router {
    let max_body = state.config.max_body;
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/mcp", get(mcp_not_allowed).post(mcp_handler))
        .route("/mcp/{alias}", get(mcp_not_allowed).post(alias_handler))
        .layer(DefaultBodyLimit::max(max_body))
        .layer(middleware::from_fn(trace_request))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (StatusCode::OK, "Pensine: a memory for assistants.\n")
}

async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        format!("ok pensine {}\n", env!("CARGO_PKG_VERSION")),
    )
}

async fn mcp_not_allowed() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}

async fn mcp_handler(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(principal) = authenticate(&state, &headers) else {
        return unauthorized(state.config.public_url().as_deref(), "");
    };
    dispatch(&state, principal, body).await
}

async fn alias_handler(
    State(state): State<AppState>,
    Path(alias): Path<String>,
    request: Request,
) -> Response {
    if validate_alias(&alias).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(principal) = authenticate(&state, request.headers()) else {
        let issuer = state.oauth.as_ref().map(|oauth| oauth.issuer());
        return unauthorized(issuer.as_deref(), &format!("/mcp/{alias}"));
    };
    match state.mcp.accounts().alias_of(&principal.account) {
        Ok(current) if current == alias => {}
        Ok(_) | Err(AccessError::NotFound) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return internal_error(),
    }
    match Bytes::from_request(request, &state).await {
        Ok(body) => dispatch(&state, principal, body).await,
        Err(error) => error.into_response(),
    }
}

async fn dispatch(state: &AppState, principal: Principal, body: Bytes) -> Response {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return parse_error(),
    };
    let mcp = state.mcp.clone();
    let timeout = state.config.request_timeout;
    let task = tokio::task::spawn_blocking(move || mcp.handle(&principal, request));
    let result = match tokio::time::timeout(timeout, task).await {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => return internal_error(),
        Err(_) => return StatusCode::GATEWAY_TIMEOUT.into_response(),
    };
    if result.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }
    json_response(StatusCode::OK, &result.to_string())
}

fn authenticate(state: &AppState, headers: &HeaderMap) -> Option<Principal> {
    let bearer = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    state
        .authenticators
        .iter()
        .find_map(|authenticator| authenticator.authenticate(bearer))
}

fn unauthorized(issuer: Option<&str>, resource_path: &str) -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    let challenge = match issuer {
        Some(url) => format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource{resource_path}\"",
            url.trim_end_matches('/')
        ),
        None => "Bearer".to_owned(),
    };
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

fn parse_error() -> Response {
    let body = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}});
    json_response(StatusCode::BAD_REQUEST, &body.to_string())
}

fn internal_error() -> Response {
    let body =
        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Internal error"}});
    json_response(StatusCode::INTERNAL_SERVER_ERROR, &body.to_string())
}

fn json_response(status: StatusCode, body: &str) -> Response {
    let mut response = (status, body.to_owned()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

async fn trace_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let duration_ms = started.elapsed().as_millis() as u64;
    tracing::info!(method = %method, path = ?path, status = status, duration_ms = duration_ms, "request");
    response
}
