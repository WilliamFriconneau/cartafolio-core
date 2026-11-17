//! The memory's tools, for an assistant that runs a command, with the outbox in front.
//!
//! One JSON-RPC message per line in, one per line out (nothing else on the output). Almost everything is
//! passed on to the memory as it is: its instructions, its tools, its answers. What changes is when the
//! network is down:
//!
//! - `initialize` and `tools/list` are answered from the last answers the memory gave, so an assistant
//!   starts offline;
//! - **`remember` (with the tag `inbox`), `deposit` and `file_put` are kept** and sent when the memory
//!   can be reached. These are the writes that need no decision and no order: a note for the inbox, an
//!   app's records with their ids, a file. The answer says `queued`, never `created`, and gives no fact id;
//! - every other tool (`recall`, `supersede`, `retire`, ...) says plainly that it needs the network.
//!
//! A refused key is never queued: the assistant is told so.
//!
//! The capture program also has tools of its own, which are never passed on: `capture_text`, `capture_file` (the file is read
//! here, so its bytes never go through the assistant), `outbox_status` and `outbox_flush`. `capture_file` only reads
//! inside the folders the person named (`with_capture_dirs`), never a hidden folder, and is not offered at all until
//! one is named: an assistant, or a page it was told to read, cannot make the capture program send any file of the disk.

use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use memcp_client::{Client, Error as ClientError};
use serde_json::{Value, json};

use crate::{Config, Error, Flush, Intake, State};

const BUFFER_NOTE: &str = "\n\nThis connection goes through a capture program on this computer. When the network is down, `remember` (with the tag `inbox`), `deposit` and `file_put` are kept here and sent when it is back: the answer then says `queued`, not `created`, and nothing is written in the memory yet. Every other tool needs the network.";

const OFFLINE: &str = "The network is down: this tool needs it. Try again when it is back.";

pub struct Gateway {
    intake: Intake,
    client: Client,
    /// A backlog may be sent: the memory just answered.
    flush_due: bool,
    /// The folders `capture_file` may read from (as the system resolves them). None: the tool is off.
    capture_roots: Vec<PathBuf>,
}

/// The tools the capture program has of its own.
const LOCAL: [&str; 4] = [
    "capture_text",
    "capture_file",
    "outbox_status",
    "outbox_flush",
];

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn fail(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// Whether a call can wait: a write with no decision and no order in it.
fn bufferable(tool: &str, args: &Value) -> bool {
    match tool {
        "deposit" | "file_put" => true,
        "remember" => args["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|t| t == "inbox" || t.starts_with("inbox/")),
        _ => false,
    }
}

impl Gateway {
    pub fn new(config: Config) -> Result<Gateway, Error> {
        let mut client = crate::client_for(&config.url, &config.key);
        // An assistant is waiting: one try, then the outbox.
        client.retry.tries = 1;
        client.retry.first = Duration::from_millis(0);
        Ok(Gateway {
            intake: Intake::open(config)?,
            client,
            flush_due: false,
            capture_roots: Vec::new(),
        })
    }

    /// The folders `capture_file` may read from. A folder that does not exist is left out; with none, the
    /// tool is off.
    #[must_use]
    pub fn with_capture_dirs(mut self, dirs: &[PathBuf]) -> Gateway {
        self.capture_roots = dirs.iter().filter_map(|d| d.canonicalize().ok()).collect();
        self
    }

    pub fn intake(&self) -> &Intake {
        &self.intake
    }

    fn cache(&self, name: &str) -> std::path::PathBuf {
        self.intake.config().outbox.join(name)
    }

    fn remember_answer(&self, name: &str, value: &Value) {
        let _ = std::fs::write(self.cache(name), value.to_string());
    }

    fn cached(&self, name: &str) -> Option<Value> {
        std::fs::read_to_string(self.cache(name))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
    }

    /// The answer to one message, or `None` for a notification.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let method = message["method"].as_str().unwrap_or("");
        let id = message.get("id").filter(|v| !v.is_null())?.clone();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        Some(match method {
            "initialize" => self.initialize(&id),
            "ping" => ok(&id, json!({})),
            "tools/list" => self.tools_list(&id, &params),
            "tools/call" => self.tools_call(&id, &params),
            _ => match self.client.request(method, params) {
                Ok(result) => {
                    self.flush_due = true;
                    ok(&id, result)
                }
                Err(e) => Self::protocol_error(&id, &e),
            },
        })
    }

    fn protocol_error(id: &Value, e: &ClientError) -> Value {
        match e {
            ClientError::Refused => fail(id, -32001, "The key is refused or revoked"),
            ClientError::Rpc { code, message } => fail(id, *code, message),
            ClientError::Unreachable(_) | ClientError::RateLimited(_) => {
                fail(id, -32000, "The network is down: this needs it")
            }
            other => fail(id, -32000, &other.to_string()),
        }
    }

    fn initialize(&mut self, id: &Value) -> Value {
        let mut result = match self.client.initialize() {
            Ok(r) => {
                self.remember_answer("initialize.json", &r);
                self.flush_due = true;
                r
            }
            Err(ClientError::Refused) => return fail(id, -32001, "The key is refused or revoked"),
            Err(_) => self.cached("initialize.json").unwrap_or_else(|| {
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "memcp-capture", "version": env!("CARGO_PKG_VERSION") },
                })
            }),
        };
        // An answer that is not an object (a broken cache, an odd server) is not trusted.
        if !result.is_object() {
            result = json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "memcp-capture", "version": env!("CARGO_PKG_VERSION") },
            });
        }
        let instructions = result["instructions"].as_str().unwrap_or("").to_string();
        result["instructions"] = json!(format!("{instructions}{BUFFER_NOTE}"));
        ok(id, result)
    }

    fn tools_list(&mut self, id: &Value, params: &Value) -> Value {
        let mut result = match self.client.request("tools/list", params.clone()) {
            Ok(r) => {
                self.remember_answer("tools.json", &r);
                r
            }
            Err(ClientError::Refused) => return fail(id, -32001, "The key is refused or revoked"),
            Err(ClientError::Rpc { code, message }) => return fail(id, code, &message),
            Err(_) => self.cached("tools.json").unwrap_or_else(offline_tools),
        };
        if !result.is_object() {
            result = offline_tools();
        }
        // The capture program's own tools come after the memory's.
        let mut local = local_tools();
        if self.capture_roots.is_empty() {
            local.retain(|t| t["name"] != "capture_file");
        }
        if let Some(list) = result["tools"].as_array_mut() {
            list.extend(local);
        }
        ok(id, result)
    }

    fn tools_call(&mut self, id: &Value, params: &Value) -> Value {
        let tool = params["name"].as_str().unwrap_or("");
        let args = &params["arguments"];
        if LOCAL.contains(&tool) {
            return ok(id, self.local_tool(tool, args));
        }
        match self.client.request("tools/call", params.clone()) {
            Ok(result) => {
                self.flush_due = true;
                ok(id, result)
            }
            Err(ClientError::Refused) => {
                ok(id, text_result("The key is refused or revoked.", true))
            }
            Err(ClientError::Rpc { code, message }) => fail(id, code, &message),
            Err(ClientError::Unreachable(_) | ClientError::RateLimited(_)) => {
                if bufferable(tool, args) {
                    self.queue(id, tool, args)
                } else {
                    ok(id, text_result(OFFLINE, true))
                }
            }
            Err(other) => fail(id, -32000, &other.to_string()),
        }
    }

    fn queue(&mut self, id: &Value, tool: &str, args: &Value) -> Value {
        // A file travels as bytes next to the item, not as text inside it.
        let mut args = args.clone();
        let mut file = None;
        if tool == "file_put" {
            let Some(text) = args["data_base64"].as_str() else {
                return ok(id, text_result("file_put needs data_base64.", true));
            };
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text.trim()) else {
                return ok(id, text_result("data_base64 is not valid base64.", true));
            };
            if let Some(o) = args.as_object_mut() {
                o.remove("data_base64");
            }
            file = Some(bytes);
        }
        match self.intake.queue_call(tool, &args, file.as_deref()) {
            Ok(queued) => ok(
                id,
                text_result(
                    &json!({
                        "result": "queued", "queued": true, "id": queued,
                        "message": "The network is down: kept on this computer and sent when it is back. Nothing is written in the memory yet.",
                    })
                    .to_string(),
                    false,
                ),
            ),
            Err(e) => ok(id, text_result(&format!("Could not be kept: {e}"), true)),
        }
    }

    /// One of the capture program's own tools: the result of the call (never an error of the protocol).
    fn local_tool(&mut self, tool: &str, args: &Value) -> Value {
        match tool {
            "capture_text" => {
                let Some(text) = args["text"].as_str() else {
                    return text_result("capture_text needs `text`.", true);
                };
                match self.intake.submit_text(text) {
                    Ok(id) => self.captured(&id),
                    Err(e) => text_result(&e.to_string(), true),
                }
            }
            "capture_file" => {
                let Some(path) = args["path"].as_str() else {
                    return text_result("capture_file needs `path`.", true);
                };
                let file = match self.allowed_file(path) {
                    Ok(f) => f,
                    Err(why) => return text_result(&why, true),
                };
                let note = args["note"].as_str().unwrap_or("");
                match self.intake.submit_file(&file, note) {
                    Ok(id) => self.captured(&id),
                    Err(e) => text_result(&e.to_string(), true),
                }
            }
            "outbox_status" => {
                let items: Vec<Value> = self
                    .intake
                    .items()
                    .iter()
                    .take(30)
                    .map(|i| {
                        json!({
                            "id": i.id, "kind": format!("{:?}", i.kind).to_lowercase(),
                            "title": i.title, "bytes": i.bytes, "detail": i.detail,
                            "state": match i.state {
                                State::Queued => "queued", State::Sending => "sending",
                                State::Sent => "sent", State::Failed => "refused by the memory",
                            },
                        })
                    })
                    .collect();
                text_result(
                    &json!({ "waiting": self.intake.waiting(), "items": items }).to_string(),
                    false,
                )
            }
            _ => {
                let r = self.intake.flush_quick();
                text_result(&flush_json(&r).to_string(), false)
            }
        }
    }

    /// A capture was kept: try to send it now, and say how it stands.
    fn captured(&mut self, id: &str) -> Value {
        let r = self.intake.flush_quick();
        // This capture's own state, not the flush's: another item may have gone while this one was refused.
        let state = self.intake.item(id).map(|i| i.state);
        let sent = matches!(state, Some(crate::State::Sent));
        if let Some(crate::State::Failed) = state {
            let why = self.intake.item(id).map(|i| i.detail).unwrap_or_default();
            let mut v = flush_json(&r);
            v["id"] = json!(id);
            v["result"] = json!("refused");
            v["message"] = json!(format!("The memory refused it: {why}"));
            return text_result(&v.to_string(), true);
        }
        let message = if sent {
            "Sent to the inbox."
        } else if r.refused {
            "Kept on this computer, but the key is refused or revoked: nothing goes until there is a new key."
        } else {
            "Kept on this computer, to be sent when the memory can be reached. Nothing is written in the memory yet."
        };
        let mut v = flush_json(&r);
        v["id"] = json!(id);
        v["result"] = json!(if sent { "sent" } else { "queued" });
        v["message"] = json!(message);
        text_result(&v.to_string(), false)
    }

    /// The file `capture_file` may read: inside a folder the person named, never in a hidden one, as the
    /// system resolves the path (a link that leads out is out).
    fn allowed_file(&self, path: &str) -> Result<PathBuf, String> {
        if self.capture_roots.is_empty() {
            return Err("capture_file is off: no folder was named. Start the capture program with --capture-dir DIR.".into());
        }
        let p = Path::new(path);
        if !p.is_absolute() {
            return Err("give the full path of the file.".into());
        }
        let real = p.canonicalize().map_err(|_| "no such file.".to_string())?;
        let root = self
            .capture_roots
            .iter()
            .find(|r| real.starts_with(r))
            .ok_or("this file is outside the folders the capture program may read from.")?;
        let inside = real.strip_prefix(root).unwrap_or(&real);
        let hidden = inside
            .components()
            .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')));
        if hidden || !real.is_file() {
            return Err("only an ordinary, visible file can be captured.".into());
        }
        // Never the capture program's own folder (its queue, the memory's answers), never a file only its owner may read (a key, a
        // settings file: what keeps them private is exactly that), never something that holds a key.
        if self
            .intake
            .config()
            .outbox
            .canonicalize()
            .is_ok_and(|o| real.starts_with(o))
        {
            return Err("this file is the capture program's own.".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&real)
                .map(|m| m.permissions().mode())
                .unwrap_or(0);
            if mode & 0o044 == 0 {
                return Err("this file is private to its owner (a key or a setting, perhaps): it is never captured.".into());
            }
        }
        let head: Vec<u8> = std::fs::File::open(&real)
            .and_then(|f| {
                use std::io::Read;
                let mut b = vec![];
                f.take(256).read_to_end(&mut b)?;
                Ok(b)
            })
            .unwrap_or_default();
        if String::from_utf8_lossy(&head).contains("mtm_") {
            return Err("this file holds a key: it is never captured.".into());
        }
        Ok(real)
    }

    /// What was due after an answer: send what was kept. Called by whoever serves the messages.
    pub fn after_reply(&mut self) {
        if self.flush_due {
            self.flush_due = false;
            // One quick try: the assistant's next call must not wait behind a backlog of recordings.
            if self.intake.waiting() > 0 {
                self.intake.flush_quick();
            }
        }
    }

    /// Serves messages from `input` until it ends, answering on `out`. After an answer from the memory, what
    /// was kept is sent.
    pub fn serve<R: BufRead, W: Write>(&mut self, input: R, out: &mut W) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(message) => self.handle(&message),
                Err(_) => Some(fail(&Value::Null, -32700, "Parse error")),
            };
            if let Some(reply) = reply {
                writeln!(out, "{reply}")?;
                out.flush()?;
            }
            self.after_reply();
        }
        Ok(())
    }
}

/// What an assistant sees of the tools when it starts offline and has never seen the memory's list.
fn offline_tools() -> Value {
    let space = json!({ "type": "string", "description": "The space." });
    json!({ "tools": [
        { "name": "remember", "description": "Keeps a note. Offline, only with the tag `inbox`: kept here and sent when the network is back.",
          "inputSchema": { "type": "object", "properties": { "text": { "type": "string" }, "space": space,
              "tags": { "type": "array", "items": { "type": "string" } } }, "required": ["text"] } },
        { "name": "deposit", "description": "Deposits records of an app (each with its origin). Offline: kept here and sent when the network is back.",
          "inputSchema": { "type": "object", "properties": { "space": space, "records": { "type": "array" } }, "required": ["records"] } },
        { "name": "file_put", "description": "Keeps a file. Offline: kept here and sent when the network is back.",
          "inputSchema": { "type": "object", "properties": { "name": { "type": "string" }, "note": { "type": "string" },
              "tags": { "type": "array", "items": { "type": "string" } }, "data_base64": { "type": "string" }, "space": space },
              "required": ["name", "data_base64"] } },
    ] })
}

fn flush_json(r: &Flush) -> Value {
    json!({
        "sent": r.sent, "waiting": r.waiting, "refused_by_the_memory": r.failed,
        "memory_unreachable": r.unreachable, "key_refused": r.refused, "busy": r.busy,
    })
}

/// The capture program's own tools, as an assistant sees them.
fn local_tools() -> Vec<Value> {
    vec![
        json!({ "name": "capture_text",
            "description": "Puts a note into À trier, the person's box for everything that comes in (the memory sorts it into a space): kept on this computer first, sent at once if the memory can be reached, else when it can. The date is added. Use it for a quick idea or a fact to file later; it is never rewritten.",
            "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] } }),
        json!({ "name": "capture_file",
            "description": "Puts a file of this computer (a recording, a document) into the inbox: the file is read here, so give its full path, not its content. Only files inside the folders the person allowed, and never hidden ones. A recording that arrives without text is transcribed by the server.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "The full path of the file." }, "note": { "type": "string", "description": "What it is, in a sentence." } }, "required": ["path"] } }),
        json!({ "name": "outbox_status",
            "description": "What waits on this computer to be sent to the memory, and what was sent lately.",
            "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "outbox_flush",
            "description": "Tries to send what waits, once, without waiting.",
            "inputSchema": { "type": "object", "properties": {} } }),
    ]
}
