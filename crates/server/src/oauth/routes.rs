use super::consent::{authorize, consent};
use super::token::token;
use super::{random_secret, OAuth, OAuthError, Result};
use crate::config::parse_web_url;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pensine_access::validate_alias;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

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
            "/.well-known/oauth-protected-resource/mcp/{alias}",
            get(alias_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(oauth)
}

async fn protected_resource(State(oauth): State<Arc<OAuth>>) -> Json<Value> {
    resource_document(&oauth.issuer(), "/mcp")
}

async fn alias_resource(State(oauth): State<Arc<OAuth>>, Path(alias): Path<String>) -> Response {
    if validate_alias(&alias).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    resource_document(&oauth.issuer(), &format!("/mcp/{alias}")).into_response()
}

fn resource_document(issuer: &str, path: &str) -> Json<Value> {
    Json(json!({
        "resource": format!("{issuer}{path}"),
        "authorization_servers": [issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["memory"],
    }))
}

async fn authorization_server(State(oauth): State<Arc<OAuth>>) -> Json<Value> {
    let issuer = oauth.issuer();
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "registration_endpoint": format!("{issuer}/register"),
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
