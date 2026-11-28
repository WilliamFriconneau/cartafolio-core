use crate::config::Config;
use crate::error::ServerError;
use crate::oauth::{self, OAuth};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::header;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use pensine_access::{Accounts, Principal};
use pensine_mcp::Mcp;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

pub trait Authenticator: Send + Sync {
    fn authenticate(&self, bearer: &str) -> Option<Principal>;
}

pub struct KeysAuthenticator {
    accounts: Arc<Accounts>,
}

impl KeysAuthenticator {
    pub fn new(accounts: Arc<Accounts>) -> Self {
        Self { accounts }
    }
}

impl Authenticator for KeysAuthenticator {
    fn authenticate(&self, bearer: &str) -> Option<Principal> {
        self.accounts.authenticate(bearer).ok()
    }
}

pub struct Server;

impl Server {
    pub async fn start(config: Config) -> Result<Handle, ServerError> {
        let accounts = Arc::new(Accounts::open(&config.data_dir)?);
        let mcp = Arc::new(Mcp::new(accounts.clone()));
        let authenticators: Vec<Arc<dyn Authenticator>> =
            vec![Arc::new(KeysAuthenticator::new(accounts.clone()))];
        Self::start_inner(config, mcp, authenticators, Some(accounts)).await
    }

    pub async fn start_with(
        config: Config,
        mcp: Arc<Mcp>,
        authenticators: Vec<Arc<dyn Authenticator>>,
    ) -> Result<Handle, ServerError> {
        Self::start_inner(config, mcp, authenticators, None).await
    }

    async fn start_inner(
        config: Config,
        mcp: Arc<Mcp>,
        mut authenticators: Vec<Arc<dyn Authenticator>>,
        accounts: Option<Arc<Accounts>>,
    ) -> Result<Handle, ServerError> {
        check_bindable(&config)?;
        let bound = bind(config.listen).await?;
        let oauth = accounts
            .map(|accounts| {
                let fallback = format!(
                    "http://{}",
                    SocketAddr::new(
                        if !bound.local_addr.ip().is_loopback() {
                            IpAddr::V4(Ipv4Addr::LOCALHOST)
                        } else {
                            bound.local_addr.ip()
                        },
                        bound.local_addr.port(),
                    )
                );
                OAuth::open(
                    &config.data_dir,
                    accounts,
                    config.public_url.as_deref().unwrap_or(&fallback),
                )
                .map(Arc::new)
            })
            .transpose()?;
        if let Some(oauth) = &oauth {
            authenticators.push(oauth.clone());
        }
        let state = AppState {
            config,
            mcp,
            authenticators: Arc::new(authenticators),
        };
        let mut router = router(state);
        if let Some(oauth) = &oauth {
            router = router.merge(oauth::routes(oauth.clone()));
        }
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        for listener in bound.listeners {
            let router = router.clone();
            let mut receiver = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = receiver.changed().await;
                    })
                    .await;
            });
        }
        Ok(Handle {
            local_addr: bound.local_addr,
            shutdown: shutdown_tx,
            oauth,
        })
    }
}

pub struct Handle {
    local_addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    oauth: Option<Arc<OAuth>>,
}

impl Handle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Issues a one-time OAuth code for an account already verified by a trusted caller.
    pub fn oauth_invitation(&self, account: &str) -> Result<String, ServerError> {
        self.oauth
            .as_ref()
            .ok_or_else(|| ServerError::Invalid("OAuth is unavailable".into()))?
            .invitation(account)
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

#[derive(Clone)]
struct AppState {
    config: Config,
    mcp: Arc<Mcp>,
    authenticators: Arc<Vec<Arc<dyn Authenticator>>>,
}

fn router(state: AppState) -> Router {
    let max_body = state.config.max_body;
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/mcp", get(mcp_not_allowed).post(mcp_handler))
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
        return unauthorized(&state.config);
    };
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

fn unauthorized(config: &Config) -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    let challenge = match &config.public_url {
        Some(url) => format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
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
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let duration_ms = started.elapsed().as_millis() as u64;
    tracing::info!(method = %method, path = %path, status = status, duration_ms = duration_ms, "request");
    response
}

fn check_bindable(config: &Config) -> Result<(), ServerError> {
    if !config.listen.ip().is_loopback() && !config.allow_remote {
        return Err(ServerError::Invalid(
            "binding a non-loopback address requires allow_remote = true".into(),
        ));
    }
    Ok(())
}

struct Bound {
    listeners: Vec<tokio::net::TcpListener>,
    local_addr: SocketAddr,
}

async fn bind(listen: SocketAddr) -> Result<Bound, ServerError> {
    if !listen.ip().is_unspecified() {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        let local_addr = listener.local_addr()?;
        return Ok(Bound {
            listeners: vec![listener],
            local_addr,
        });
    }
    let wildcard = tokio::net::TcpListener::bind(listen).await?;
    let local_addr = wildcard.local_addr()?;
    let loopback_ip = if local_addr.is_ipv4() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        IpAddr::V6(Ipv6Addr::LOCALHOST)
    };
    let loopback_addr = SocketAddr::new(loopback_ip, local_addr.port());
    match tokio::net::TcpListener::bind(loopback_addr).await {
        Ok(loopback) => Ok(Bound {
            listeners: vec![wildcard, loopback],
            local_addr: loopback_addr,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if wildcard_covers_loopback() {
                Ok(Bound {
                    listeners: vec![wildcard],
                    local_addr,
                })
            } else {
                Err(ServerError::Invalid(format!(
                    "the loopback address {loopback_addr} is already held by another program"
                )))
            }
        }
        Err(error) => Err(ServerError::Io(error)),
    }
}

fn wildcard_covers_loopback() -> bool {
    cfg!(target_os = "linux")
}
