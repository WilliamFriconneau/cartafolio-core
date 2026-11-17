//! HTTP bytes: a request and a response without a socket, so everything is testable without a network.

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Lowercase names.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub ip: Option<String>,
    /// Arrived on the ingress listener from the trusted proxy (Home Assistant):
    /// only then are the `X-Remote-User-*` headers believed.
    pub via_ingress: bool,
    /// Arrived on the identity listener (behind Tailscale serve, not Funnel): the
    /// tailnet login of the visitor, from `Tailscale-User-Login`.
    pub tailnet_login: Option<String>,
}

impl Request {
    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("", String::as_str)
    }

    /// The key of a connection: `Authorization: Bearer …`, or `?key=…` in the address, so that one address
    /// carries everything (the person pastes it once, in the assistant).
    pub fn key(&self) -> Option<String> {
        bearer_of(self.header("authorization")).or_else(|| {
            self.query
                .get("key")
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        })
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// A big answer kept in a file, sent from it block by block instead of held in memory (the export). The file is
    /// removed once it is sent. When set, `body` is empty.
    pub file: Option<std::path::PathBuf>,
}

impl Response {
    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: v.to_string().into_bytes(),
            file: None,
        }
    }

    pub fn empty(status: u16) -> Response {
        Response {
            status,
            headers: vec![],
            body: vec![],
            file: None,
        }
    }

    pub fn with(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The whole answer: the body, or the file it is kept in (which is removed, as sending it would). For tests and
    /// small answers; a big one is sent from its file, never held.
    pub fn bytes(&self) -> Vec<u8> {
        match &self.file {
            Some(p) => {
                let b = std::fs::read(p).unwrap_or_default();
                let _ = std::fs::remove_file(p);
                b
            }
            None => self.body.clone(),
        }
    }

    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// Splits `a=1&b=2` (no percent-decoding: our parameters are identifiers).
/// The query string, percent-decoded (`redirect_uri=https%3A%2F%2F…` arrives encoded).
pub fn parse_query(q: &str) -> HashMap<String, String> {
    crate::oauth::form(q.as_bytes())
}

/// `Bearer xxx`, tolerating an extra "Bearer" or "Authorization:" pasted into the field.
pub fn bearer_of(header: &str) -> Option<String> {
    let mut h = header.trim();
    if !h.to_ascii_lowercase().starts_with("bearer ") {
        return None;
    }
    loop {
        let lower = h.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            h = h[7..].trim_start();
        } else if lower.starts_with("authorization:") {
            h = h[14..].trim_start();
        } else {
            break;
        }
    }
    (!h.is_empty()).then(|| h.to_string())
}
