use crate::error::ServerError;
use crate::html::escape_html;
use crate::oauth::SessionInfo;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use pensine_access::{Connection, Grant};
use pensine_store::SpaceInfo;
use rand::{rngs::OsRng, TryRngCore};
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

pub(super) const CSRF_COOKIE: &str = "pensine_csrf";
const STYLE: &str = "body{font-family:system-ui,sans-serif;max-width:48rem;margin:2rem auto;padding:0 1rem}nav a{margin-right:1rem}code{background:#0001;padding:0.1rem 0.3rem}fieldset{border:1px solid #888;margin-bottom:1rem}@media(prefers-color-scheme:dark){body{background:#111;color:#ddd}code{background:#fff2}}";

pub(super) fn render_connections(
    account_id: &str,
    alias_form: &str,
    connections: &[Connection],
    spaces: &[SpaceInfo],
    csrf: &str,
) -> String {
    let mut body = format!("<h1>Connections for {}</h1>", escape_html(account_id));
    body.push_str(alias_form);
    if connections.is_empty() {
        body.push_str("<p>No connections yet.</p>");
    } else {
        body.push_str("<ul>");
        for connection in connections {
            let grants: Vec<String> = connection
                .grants
                .iter()
                .map(|grant| {
                    format!(
                        "{} ({})",
                        escape_html(&grant.space),
                        if grant.write { "write" } else { "read" }
                    )
                })
                .collect();
            body.push_str(&format!(
                "<li><strong>{}</strong> — {}{}<form method=\"post\" action=\"connections/revoke\">{}{}</form></li>",
                escape_html(&connection.name),
                grants.join(", "),
                if connection.read_only { " — read only" } else { "" },
                csrf_field(csrf),
                hidden_field("id", &connection.id),
            ));
        }
        body.push_str("</ul>");
    }
    body.push_str("<h2>Create a connection</h2><form method=\"post\" action=\"connections\">");
    body.push_str(&csrf_field(csrf));
    body.push_str("<label>Name <input name=\"name\" required></label><br>");
    body.push_str(
        "<label><input type=\"checkbox\" name=\"read_only\" value=\"1\"> Read only</label><br>",
    );
    body.push_str("<fieldset><legend>Spaces (leave empty to grant all)</legend>");
    for space in spaces {
        body.push_str(&format!(
            "<label><input type=\"checkbox\" name=\"s_{}\" value=\"1\"> {}</label>\
             <label><input type=\"checkbox\" name=\"w_{}\" value=\"1\"> write</label><br>",
            escape_html(&space.name),
            escape_html(&space.label),
            escape_html(&space.name),
        ));
    }
    body.push_str("</fieldset><button>Create</button></form>");
    body
}

pub(super) fn render_alias(
    public_url: Option<&str>,
    alias: &str,
    error: Option<&str>,
    csrf: &str,
) -> String {
    let mut body = match public_url {
        Some(url) => format!(
            "<p><label>MCP address <input size=\"48\" readonly value=\"{}/mcp/{}\"></label></p>",
            escape_html(url.trim_end_matches('/')),
            escape_html(alias),
        ),
        None => "<p>Set the public address (public_url) to show your MCP address.</p>".into(),
    };
    if let Some(error) = error {
        body.push_str(&format!("<p role=\"alert\">{}</p>", escape_html(error)));
    }
    body.push_str(&format!(
        "<form method=\"post\" action=\"alias\">{}<label>Alias <input name=\"alias\" \
         value=\"{}\" required maxlength=\"32\"></label> <button>Save alias</button></form>",
        csrf_field(csrf),
        escape_html(alias),
    ));
    body
}

pub(super) fn render_oauth(sessions: &[SessionInfo], csrf: &str) -> String {
    let mut body = String::from(
        "<h1>OAuth</h1><p>Connections for Claude on the web (claude.ai).</p>\
         <form method=\"post\" action=\"oauth\">",
    );
    body.push_str(&csrf_field(csrf));
    body.push_str("<button>Create a code for Claude on the web</button></form>");
    if sessions.is_empty() {
        body.push_str("<p>No active OAuth sessions.</p>");
    } else {
        body.push_str("<ul>");
        for session in sessions {
            body.push_str(&format!(
                "<li><strong>{}</strong> ({}) — {}, spaces: {}, expires in about {} day(s)<form method=\"post\" action=\"oauth/revoke\">{}{}</form></li>",
                escape_html(&session.name),
                escape_html(&session.client),
                if session.read_only { "read only" } else { "read/write" },
                escape_html(&session.spaces.join(", ")),
                days_until(session.refresh_until),
                csrf_field(csrf),
                hidden_field("id", &session.id),
            ));
        }
        body.push_str("</ul>");
    }
    body
}

pub(super) fn grants_from_form(
    fields: &HashMap<String, String>,
    spaces: &[SpaceInfo],
) -> (Vec<Grant>, bool) {
    let read_only = checked(fields, "read_only");
    let mut grants: Vec<Grant> = Vec::new();
    for space in spaces {
        if checked(fields, &format!("s_{}", space.name)) {
            grants.push(Grant {
                space: space.name.clone(),
                write: !read_only && checked(fields, &format!("w_{}", space.name)),
            });
        }
    }
    if grants.is_empty() {
        grants.push(Grant {
            space: "*".into(),
            write: !read_only,
        });
    }
    (grants, read_only)
}

pub(super) fn checked(fields: &HashMap<String, String>, name: &str) -> bool {
    fields.get(name).map(String::as_str) == Some("1")
}

pub(super) fn new_csrf() -> Result<String, ServerError> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(std::io::Error::other)?;
    let mut token = String::with_capacity(64);
    for byte in bytes {
        write!(token, "{byte:02x}").map_err(std::io::Error::other)?;
    }
    Ok(token)
}

pub(super) fn csrf_matches(headers: &HeaderMap, fields: &HashMap<String, String>) -> bool {
    match (
        cookie_value(headers, CSRF_COOKIE),
        fields.get("csrf").map(String::as_str),
    ) {
        (Some(cookie), Some(field)) => !cookie.is_empty() && cookie == field,
        _ => false,
    }
}

pub(super) fn page(title: &str, body: &str, csrf: &str) -> Result<Response, ServerError> {
    let cookie = format!("{CSRF_COOKIE}={csrf}; Path=/; HttpOnly; SameSite=Strict");
    let cookie = HeaderValue::from_str(&cookie)
        .map_err(|error| ServerError::Invalid(format!("invalid CSRF cookie: {error}")))?;
    let html = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{}</style>\
         <body><nav><a href=\"connections\">Connections</a><a href=\"oauth\">OAuth</a>\
         <a href=\"spaces\">Spaces</a><a href=\"status\">Status</a></nav>{}</body></html>",
        escape_html(title),
        STYLE,
        body,
    );
    let mut response = Html(html).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}

pub(super) fn disk_used(dir: &Path) -> u64 {
    fn sum(path: &Path, total: &mut u64) {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    sum(&entry_path, total);
                } else if let Ok(metadata) = entry.metadata() {
                    *total += metadata.len();
                }
            }
        }
    }
    let mut total = 0;
    sum(dir, &mut total);
    total
}

fn days_until(timestamp: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    (timestamp.saturating_sub(now)).div_euclid(86_400).max(0)
}

pub(super) fn space_depth(space: &SpaceInfo, all: &[SpaceInfo]) -> usize {
    let mut depth = 0;
    let mut current = space.parent.clone();
    while let Some(parent) = current {
        depth += 1;
        current = all
            .iter()
            .find(|candidate| candidate.name == parent)
            .and_then(|candidate| candidate.parent.clone());
    }
    depth
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.trim().to_owned())
    })
}

fn csrf_field(csrf: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"csrf\" value=\"{}\">",
        escape_html(csrf)
    )
}

fn hidden_field(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        escape_html(name),
        escape_html(value)
    )
}
