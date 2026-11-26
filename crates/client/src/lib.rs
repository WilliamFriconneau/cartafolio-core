#![warn(missing_docs)]
//! A program's view of a connection of the memory: its address (`https://<server>/mcp/<name>`) and its
//! key, over MCP on HTTP. It sends what it is given and keeps nothing but the session the memory opens.
//!
//! What it adds to a bare request:
//! - the session: `initialize` once, `Mcp-Session-Id` sent back, started again if the memory forgot it;
//! - answers as JSON or as a stream of events;
//! - retries: a memory that cannot be reached, a 5xx, or an address blocked for a while (HTTP 429, which
//!   says how long to wait) is tried again after a wait; a refused key (401) and the memory's own refusals never are;
//! - `deposit`, which cuts a long list of records into the batches the memory accepts.

use std::io::{BufReader, Read};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

#[cfg(feature = "sync")]
pub mod peer_sync;
mod sse;
mod url;

pub use url::allowed_url;

/// An error of the network in words that never name the memory's address: an error text can reach an assistant, a log,
/// or a screen, and the address is not theirs to know.
fn redact(message: &str, url: &str) -> String {
    let mut out = message.to_string();
    let host = url
        .split("//")
        .nth(1)
        .and_then(|r| r.split(['/', '?']).next())
        .unwrap_or("");
    for secret in [
        url.trim_end_matches('/'),
        host,
        host.rsplit_once(':').map_or(host, |(h, _)| h),
    ] {
        if !secret.is_empty() {
            out = out.replace(secret, "<the memory>");
        }
    }
    out
}

/// What a device gets when it pairs: its connection's address and key, and the spaces whose inbox it writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// Name of the new connection (`peer-<device>-<4 characters>`).
    pub agent: String,
    /// The address of the new connection (`https://<server>/mcp/<agent>`).
    pub url: String,
    /// The key, shown once: the caller must keep it; the memory stores only its hash.
    pub key: String,
    /// The one box the key writes to (the person's inbox).
    pub spaces: Vec<String>,
}

/// Pairs a device with a memory: `server` is the address the person's page shows, `code` the one-time code it
/// gave, `device` what the device calls itself (it names the connection). Done once; the key is not kept here.
pub fn pair(server: &str, code: &str, device: &str) -> Result<Paired, Error> {
    pair_allowing(server, code, device, false)
}

/// [`pair`], and with `allow_clear` an address that is not HTTPS nor of the own network is taken anyway (the caller
/// warns the person: the key will travel in clear).
pub fn pair_allowing(
    server: &str,
    code: &str,
    device: &str,
    allow_clear: bool,
) -> Result<Paired, Error> {
    let server = server.trim().trim_end_matches('/');
    if !allow_clear && !allowed_url(server) {
        return Err(Error::Tool(
            "this address is refused: HTTPS, or in clear only on this machine or the own network"
                .into(),
        ));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let body = json!({ "code": code, "device": device }).to_string();
    let mut res = agent
        .post(&format!("{server}/pair"))
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| Error::Unreachable(redact(&e.to_string(), server)))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .map_err(|e| Error::Unreadable(redact(&e.to_string(), server)))?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let said = || v["error"].as_str().unwrap_or("").to_string();
    match status {
        200 => {}
        429 => return Err(Error::RateLimited(60)),
        400..=499 => return Err(Error::Tool(said())),
        _ => return Err(Error::Unreachable(format!("HTTP {status} {}", said()))),
    }
    let field = |k: &str| {
        v[k].as_str()
            .map(String::from)
            .ok_or_else(|| Error::Unreadable(format!("no {k} in the answer")))
    };
    let paired = Paired {
        agent: field("agent")?,
        url: field("mcp_url")?,
        key: field("key")?,
        // `spaces`; a node older than the rename says `vaults`.
        spaces: v["spaces"]
            .as_array()
            .or_else(|| v["vaults"].as_array())
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
    };
    // The address the memory gives back must be as safe as the one typed.
    if !allow_clear && !allowed_url(&paired.url) {
        return Err(Error::Unreadable(
            "the memory gave an address that is refused".into(),
        ));
    }
    Ok(paired)
}

/// Most records the memory takes in one `deposit` call.
pub const BATCH: usize = 500;
/// The biggest answer read, in bytes.
const ANSWER_MAX: u64 = 40 * 1024 * 1024;
const PROTOCOL: &str = "2025-06-18";

/// What went wrong, and whether trying again could help.
#[derive(Debug)]
pub enum Error {
    /// The memory cannot be reached, or answered a server error: worth another try.
    Unreachable(String),
    /// The key is refused or revoked (HTTP 401). Never retried.
    Refused,
    /// The memory asks to wait (HTTP 429: the flood ceiling, or an address blocked after refused keys): the seconds it asks.
    RateLimited(u64),
    /// The memory answered with an error of the protocol.
    Rpc {
        /// The JSON-RPC error code (`-32602` for an unknown tool).
        code: i64,
        /// The memory's text.
        message: String,
    },
    /// The memory refused this call in its own words (a right, a size, a rule).
    Tool(String),
    /// An answer that is not what was expected.
    Unreadable(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable(m) => write!(f, "the memory cannot be reached: {m}"),
            Error::Refused => write!(f, "the key is refused or revoked"),
            Error::RateLimited(s) => write!(f, "the memory asks to wait {s} s"),
            Error::Rpc { code, message } => write!(f, "protocol error {code}: {message}"),
            Error::Tool(m) => write!(f, "refused by the memory: {m}"),
            Error::Unreadable(m) => write!(f, "unreadable answer: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// How long to wait before trying again, and how many tries.
#[derive(Clone, Debug)]
pub struct Retry {
    /// Attempts in all (the first one included).
    pub tries: u32,
    /// The first wait; doubled each time, for a failure without a delay of its own.
    pub first: Duration,
    /// The longest wait taken, whatever the memory asks.
    pub longest: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Retry {
            tries: 5,
            first: Duration::from_secs(2),
            longest: Duration::from_secs(120),
        }
    }
}

/// The records of a `deposit` and what the memory said of each.
#[derive(Debug)]
pub struct Deposited {
    /// One entry per record sent, in order: `{index, result, id}` or `{index, result: "error", error}`.
    pub results: Vec<Value>,
    /// Records the memory refused or that could not be sent.
    pub failed: usize,
}

/// A connection to one memory: its address, its key and the MCP session the memory opened. Not shared between
/// threads (calls take `&mut self`).
pub struct Client {
    url: String,
    token: String,
    agent: ureq::Agent,
    session: Option<String>,
    next_id: u64,
    /// The retry policy applied by every call.
    pub retry: Retry,
    /// Take an address that is not HTTPS nor of the own network (off by default). Whoever turns it on warns the person:
    /// the key travels in clear.
    pub allow_clear: bool,
}

impl Client {
    /// `url` is the address of a connection; `token` its key.
    pub fn new(url: &str, token: &str) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // A redirect would resend the body (a health record, a recording) where it points.
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(300)))
            .build()
            .into();
        Client {
            url: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            agent,
            session: None,
            next_id: 0,
            retry: Retry::default(),
            allow_clear: false,
        }
    }

    /// One call of a tool of the memory: what it answered, as JSON (a text answer comes back as
    /// `{"text": ...}`). Tried again as `retry` says; a refusal of the memory is `Error::Tool`.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, Error> {
        self.retrying(|c| c.call_once(tool, &arguments))
    }

    /// Any request of the protocol (`tools/list`, `tools/call`, ...): the `result` of the answer, untouched,
    /// so a caller that only passes things on loses nothing. Tried again as `retry` says; an error of the
    /// protocol is `Error::Rpc`.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.retrying(|c| c.request_once(method, &params))
    }

    /// The answer of the memory to `initialize`: its version, its capabilities and the instructions it
    /// gives an assistant that connects (the guides of the person and of the vaults the key reaches).
    pub fn initialize(&mut self) -> Result<Value, Error> {
        self.retrying(Client::start)
    }

    fn retrying<T>(
        &mut self,
        mut attempt: impl FnMut(&mut Client) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut wait = self.retry.first;
        let mut tries = 0;
        loop {
            tries += 1;
            match attempt(self) {
                Err(Error::Unreachable(_)) if tries < self.retry.tries => {
                    std::thread::sleep(wait.min(self.retry.longest));
                    wait = wait.saturating_mul(2);
                }
                Err(Error::RateLimited(s)) if tries < self.retry.tries => {
                    std::thread::sleep(Duration::from_secs(s).min(self.retry.longest));
                }
                other => return other,
            }
        }
    }

    /// Deposits `records` (each a `remember` with its `origin`) in `space`, in batches of [`BATCH`].
    /// A record the memory refuses is reported, not fatal; a memory that cannot be reached, after the
    /// retries, is an error and the records not yet sent are not.
    pub fn deposit(&mut self, space: &str, records: &[Value]) -> Result<Deposited, Error> {
        let mut results = Vec::with_capacity(records.len());
        let mut failed = 0;
        for (n, chunk) in records.chunks(BATCH).enumerate() {
            let answer = self.call("deposit", json!({ "space": space, "records": chunk }))?;
            let list = answer["results"]
                .as_array()
                .ok_or_else(|| Error::Unreadable("deposit answered no results".into()))?;
            for item in list {
                let mut item = item.clone();
                if let Some(i) = item["index"].as_u64() {
                    item["index"] = json!(n * BATCH + usize::try_from(i).unwrap_or(0));
                }
                if item["result"] == "error" {
                    failed += 1;
                }
                results.push(item);
            }
        }
        Ok(Deposited { results, failed })
    }

    /// Keeps a file (a recording, a document) in `space`, filed under `tags`.
    pub fn file_put(
        &mut self,
        space: &str,
        name: &str,
        note: &str,
        tags: &[&str],
        bytes: &[u8],
    ) -> Result<Value, Error> {
        self.call(
            "file_put",
            json!({
                "space": space, "name": name, "note": note, "tags": tags,
                "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
    }

    /// A file kept in the memory: what the memory says of it, and its bytes. A recording of 24 MB comes as
    /// 32 MB of base64: the answer limit allows it.
    pub fn file_get(&mut self, id: &str) -> Result<(Value, Vec<u8>), Error> {
        let result = self.request(
            "tools/call",
            json!({"name": "file_get", "arguments": {"id": id}}),
        )?;
        let blocks = result["content"].as_array().cloned().unwrap_or_default();
        let text = blocks
            .first()
            .and_then(|b| b["text"].as_str())
            .unwrap_or("");
        if result["isError"].as_bool() == Some(true) {
            return Err(Error::Tool(text.to_string()));
        }
        let meta: Value = serde_json::from_str(text)
            .map_err(|_| Error::Unreadable("file_get: no description of the file".into()))?;
        // The bytes come in the block after the description: an embedded resource, or an image; a text file, as text.
        let blob = blocks.iter().find_map(|b| {
            b["resource"]["blob"]
                .as_str()
                .or_else(|| b["data"].as_str())
        });
        let bytes = match blob {
            Some(data) => base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| Error::Unreadable("file_get: the bytes are not base64".into()))?,
            None => blocks
                .get(1)
                .and_then(|b| b["text"].as_str())
                .map(|t| t.as_bytes().to_vec())
                .ok_or_else(|| Error::Unreadable("file_get: no bytes in the answer".into()))?,
        };
        Ok((meta["file"].clone(), bytes))
    }

    fn call_once(&mut self, tool: &str, arguments: &Value) -> Result<Value, Error> {
        let result =
            self.request_once("tools/call", &json!({"name": tool, "arguments": arguments}))?;
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        if result["isError"].as_bool() == Some(true) {
            return Err(Error::Tool(text.to_string()));
        }
        Ok(serde_json::from_str(text).unwrap_or_else(|_| json!({ "text": text })))
    }

    fn request_once(&mut self, method: &str, params: &Value) -> Result<Value, Error> {
        if self.session.is_none() {
            self.start()?;
        }
        let answer = match self.send_request(method, params) {
            // The memory lost the session: start again once.
            Err(Error::Unreadable(m)) if m == "session lost" => {
                self.start()?;
                self.send_request(method, params)?
            }
            other => other?,
        };
        if let Some(e) = answer.get("error") {
            return Err(Error::Rpc {
                code: e["code"].as_i64().unwrap_or(0),
                message: e["message"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok(answer["result"].clone())
    }

    fn send_request(&mut self, method: &str, params: &Value) -> Result<Value, Error> {
        let id = self.take_id();
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.post(&request, Some(id))
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// `initialize`, then the notification that says it is done: the answer to `initialize`. The memory may
    /// hand a session.
    fn start(&mut self) -> Result<Value, Error> {
        self.session = None;
        let id = self.take_id();
        let init = json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
            "protocolVersion": PROTOCOL, "capabilities": {},
            "clientInfo": {"name": "pensine-client", "version": env!("CARGO_PKG_VERSION")}}});
        let answer = self.post(&init, Some(id))?;
        if let Some(e) = answer.get("error") {
            return Err(Error::Rpc {
                code: e["code"].as_i64().unwrap_or(0),
                message: e["message"].as_str().unwrap_or("").to_string(),
            });
        }
        let done = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        // A memory with no session still answers: the session only exists once it said so.
        let _ = self.post(&done, None);
        // "" marks a started connection with no session id to send.
        self.session.get_or_insert_with(String::new);
        Ok(answer["result"].clone())
    }

    /// Posts one message. `id` is the request waiting for an answer, `None` for a notification.
    fn post(&mut self, message: &Value, id: Option<u64>) -> Result<Value, Error> {
        // Checked on every request, so that no program using this client can send the key in clear by mistake.
        if !self.allow_clear && !allowed_url(&self.url) {
            return Err(Error::Tool(
                "the memory's address is refused: the key goes over HTTPS, or in clear only on this machine or the own network"
                    .into(),
            ));
        }
        let mut request = self
            .agent
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(s) = self.session.as_deref().filter(|s| !s.is_empty()) {
            request = request.header("Mcp-Session-Id", s);
        }
        let mut response = request
            .send(message.to_string().as_bytes())
            .map_err(|e| Error::Unreachable(redact(&e.to_string(), &self.url)))?;
        let status = response.status().as_u16();
        if let Some(s) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session = Some(s.to_string());
        }
        match status {
            401 => return Err(Error::Refused),
            429 => {
                let wait = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60);
                return Err(Error::RateLimited(wait));
            }
            // Only a session the memory opened can be lost; any other 404 is an answer (an unknown method).
            404 if self.session.as_deref().is_some_and(|s| !s.is_empty()) => {
                self.session = None;
                return Err(Error::Unreadable("session lost".into()));
            }
            500..=599 => return Err(Error::Unreachable(format!("HTTP {status}"))),
            202 | 204 => return Ok(Value::Null),
            _ => {}
        }
        let Some(id) = id else { return Ok(Value::Null) };
        let stream = response.body().mime_type() == Some("text/event-stream");
        let body = if stream {
            let reader =
                BufReader::new(response.body_mut().with_config().limit(ANSWER_MAX).reader());
            sse::answer_to(reader, id)
                .ok_or_else(|| Error::Unreadable("the stream ended before the answer".into()))?
        } else {
            let mut text = String::new();
            response
                .body_mut()
                .with_config()
                .limit(ANSWER_MAX)
                .reader()
                .read_to_string(&mut text)
                .map_err(|e| Error::Unreadable(redact(&e.to_string(), &self.url)))?;
            text
        };
        let value: Value = serde_json::from_str(&body)
            .map_err(|_| Error::Unreadable(format!("HTTP {status}, not JSON")))?;
        match value {
            Value::Array(items) => items
                .into_iter()
                .find(|m| m["id"] == json!(id))
                .ok_or_else(|| Error::Unreadable("no answer to the request".into())),
            v => Ok(v),
        }
    }
}
