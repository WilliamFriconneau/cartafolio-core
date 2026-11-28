use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{io::Read, thread, time::Duration};

pub type Result<T> = std::result::Result<T, String>;

pub struct Client<'a> {
    agent: ureq::Agent,
    url: &'a str,
    auth: String,
}

pub struct Reply {
    pub data: Value,
    pub blocks: Vec<Value>,
}

impl<'a> Client<'a> {
    pub fn new(url: &'a str, key: &str) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(Duration::from_secs(65))
            .build();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("Expected an HTTP or HTTPS MCP endpoint".into());
        }
        Ok(Self {
            agent,
            url,
            auth: format!("Bearer {key}"),
        })
    }

    pub fn call(&self, tool: &str, arguments: Value) -> Result<Reply> {
        self.request(tool, arguments)
            .and_then(decode_reply)
            .map_err(|error| format!("{tool}: {error}"))
    }

    fn request(&self, tool: &str, arguments: Value) -> Result<Value> {
        let request = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":tool,"arguments":arguments}
        })
        .to_string();
        for attempt in 0..=5 {
            match self.send(&request) {
                Ok(response) => return Ok(response),
                Err(Failure::Permanent(reason)) => return Err(reason),
                Err(Failure::Transient(reason)) if attempt == 5 => return Err(reason),
                Err(Failure::Transient(_)) => thread::sleep(Duration::from_millis(100 << attempt)),
            }
        }
        Err("Retry limit reached".into())
    }

    fn send(&self, request: &str) -> std::result::Result<Value, Failure> {
        let response = self
            .agent
            .post(self.url)
            .set("Authorization", &self.auth)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .send_string(request)
            .map_err(classify)?;
        let events = response.content_type() == "text/event-stream";
        let mut body = String::new();
        response
            .into_reader()
            .take(256 * 1024 * 1024 + 1)
            .read_to_string(&mut body)
            .map_err(|_| Failure::Transient("Could not read HTTP response".into()))?;
        if body.len() > 256 * 1024 * 1024 {
            return Err(Failure::Permanent(
                "HTTP response exceeds the size limit".into(),
            ));
        }
        if events {
            for event in body.replace("\r\n", "\n").split("\n\n") {
                let data = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Ok(value) = serde_json::from_str::<Value>(&data) {
                    if value["id"] == 1 {
                        return Ok(value);
                    }
                }
            }
            return Err(Failure::Permanent("Missing MCP event response".into()));
        }
        serde_json::from_str(&body).map_err(|_| Failure::Permanent("Invalid JSON response".into()))
    }
}

enum Failure {
    Permanent(String),
    Transient(String),
}

fn classify(error: ureq::Error) -> Failure {
    match error {
        ureq::Error::Status(code, _) if matches!(code, 408 | 425 | 429 | 500..=599) => {
            Failure::Transient(format!("HTTP {code} after retries"))
        }
        ureq::Error::Status(code, _) => Failure::Permanent(format!("HTTP {code}")),
        ureq::Error::Transport(error)
            if matches!(
                error.kind(),
                ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme
            ) =>
        {
            Failure::Permanent("Invalid MCP endpoint".into())
        }
        ureq::Error::Transport(_) => {
            Failure::Transient("HTTP transport failed after retries".into())
        }
    }
}

fn decode_reply(response: Value) -> Result<Reply> {
    if response["jsonrpc"] != "2.0" || response["id"] != 1 {
        return Err("Invalid JSON-RPC response".into());
    }
    let result = &response["result"];
    if !response["error"].is_null() || result["isError"] == true {
        // A remote error can include credentials or note text.
        return Err("Remote server rejected the request".into());
    }
    let mut blocks = array(&result["content"])?.clone().into_iter();
    let first = blocks.next().ok_or("Missing MCP content")?;
    let data = serde_json::from_str(text(&first["text"])?).map_err(|_| "Invalid MCP content")?;
    Ok(Reply {
        data,
        blocks: blocks.collect(),
    })
}

pub fn array(value: &Value) -> Result<&Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| "Expected an array in the MCP response".into())
}

pub fn text(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| "Expected a string in the MCP response".into())
}

pub fn parse<T: DeserializeOwned>(value: &Value) -> Result<T> {
    T::deserialize(value).map_err(|_| "Invalid record in the MCP response".into())
}

pub fn next<'a>(
    value: &'a Value,
    seen: &mut std::collections::HashSet<String>,
) -> Result<Option<&'a str>> {
    if value.is_null() {
        return Ok(None);
    }
    let cursor = text(value)?;
    if cursor.is_empty() || !seen.insert(cursor.to_owned()) {
        return Err("Invalid or repeated page cursor".into());
    }
    Ok(Some(cursor))
}
