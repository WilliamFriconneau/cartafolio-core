use crate::admission::{Admission, Admitted};
use crate::oauth::OAuth;
use crate::{Authenticator, Config};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, MatchedPath, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Version};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use pensine_access::{validate_alias, AccessError, AccountOperations, Principal};
use pensine_mcp::Mcp;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{timeout_at, Instant as Deadline};

#[derive(Clone)]
pub(super) struct AppState {
    pub admission: Arc<Admission>,
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

async fn mcp_handler(State(state): State<AppState>, request: Request) -> Response {
    let deadline = Deadline::now() + state.config.request_timeout;
    let Some(principal) = authenticate(&state, request.headers()) else {
        return unauthorized(state.config.public_url().as_deref(), "");
    };
    read_and_dispatch(&state, principal, request, deadline).await
}

async fn alias_handler(
    State(state): State<AppState>,
    Path(alias): Path<String>,
    request: Request,
) -> Response {
    let deadline = Deadline::now() + state.config.request_timeout;
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
    read_and_dispatch(&state, principal, request, deadline).await
}

async fn read_and_dispatch(
    state: &AppState,
    principal: Principal,
    request: Request,
    deadline: Deadline,
) -> Response {
    let Some(permit) = state.admission.enter(&principal.account, deadline).await else {
        return busy();
    };
    let read_deadline = deadline.min(Deadline::now() + state.config.read_timeout);
    let version = request.version();
    let body = match timeout_at(read_deadline, Bytes::from_request(request, state)).await {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => return error.into_response(),
        Err(_) => {
            let mut response = StatusCode::REQUEST_TIMEOUT.into_response();
            if matches!(version, Version::HTTP_10 | Version::HTTP_11) {
                response
                    .headers_mut()
                    .insert(header::CONNECTION, HeaderValue::from_static("close"));
            }
            return response;
        }
    };
    let lock_deadline = deadline.min(Deadline::now() + state.config.account_lock_timeout);
    let operations = match timeout_at(lock_deadline, lock_account(state, &principal)).await {
        Ok(Ok(operations)) if Deadline::now() < lock_deadline => operations,
        Ok(Err(_)) => return internal_error(),
        _ => return busy(),
    };
    dispatch(state, principal, body, operations, permit, deadline).await
}

async fn lock_account(
    state: &AppState,
    principal: &Principal,
) -> Result<AccountOperations, AccessError> {
    let account = state.mcp.accounts().account(&principal.account)?;
    loop {
        if let Some(operations) = account.try_lock_operations()? {
            return Ok(operations);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn dispatch(
    state: &AppState,
    principal: Principal,
    body: Bytes,
    operations: AccountOperations,
    permit: Admitted,
    deadline: Deadline,
) -> Response {
    let mcp = state.mcp.clone();
    // A synchronous operation already underway can finish atomically after a timeout.
    // The worker owns both capacity permits and the account lock until it exits.
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if Deadline::now() >= deadline {
            return Err(StatusCode::GATEWAY_TIMEOUT);
        }
        let request: Value = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
        if Deadline::now() >= deadline {
            return Err(StatusCode::GATEWAY_TIMEOUT);
        }
        Ok(mcp.handle_admitted(&principal, request, &operations))
    });
    let _abort = AbortOnDrop(task.abort_handle());
    let result = match timeout_at(deadline, task).await {
        Ok(Ok(Ok(value))) => value,
        Ok(Ok(Err(StatusCode::BAD_REQUEST))) => return parse_error(),
        Ok(Err(_)) => return internal_error(),
        Ok(Ok(Err(_))) | Err(_) => return StatusCode::GATEWAY_TIMEOUT.into_response(),
    };
    if result.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }
    json_response(StatusCode::OK, &result.to_string())
}

struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
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

fn busy() -> Response {
    let mut response = StatusCode::SERVICE_UNAVAILABLE.into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
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
