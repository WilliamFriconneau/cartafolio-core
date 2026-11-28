use super::{audit::Audit, AccountData, ACCOUNTS};
use serde_json::{json, Value};
use std::{thread, time::Duration};

pub type Reply = Result<Value, String>;

pub struct Client<'a> {
    agent: ureq::Agent,
    url: String,
    authorization: String,
    account: usize,
    forbidden: Vec<String>,
    role: usize,
    audit: Option<&'a Audit>,
}

impl<'a> Client<'a> {
    pub fn new(base: &str, data: &[AccountData; 2], account: usize, role: usize) -> Self {
        let here = &data[account];
        let other = &data[1 - account];
        let mut forbidden = vec![
            ACCOUNTS[1 - account].into(),
            other.hidden.clone(),
            other.note.clone(),
            other.todo.clone(),
            other.hidden_note.clone(),
            other.hidden_file.clone(),
        ];
        if role != 0 {
            forbidden.extend([
                here.hidden.clone(),
                here.hidden_note.clone(),
                here.hidden_file.clone(),
            ]);
        }
        Self {
            agent: agent(),
            url: format!("{base}/mcp"),
            authorization: format!("Bearer {}", here.keys[role]),
            account,
            forbidden,
            role,
            audit: None,
        }
    }

    pub fn audited(mut self, audit: &'a Audit) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn call(&self, tool: &str, args: Value) -> Reply {
        self.request(tool, args)
            .unwrap_or_else(|error| panic!("{tool}: HTTP transport failed: {error}"))
    }

    pub fn request(&self, tool: &str, args: Value) -> Result<Reply, String> {
        let body = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":tool,"arguments":args}
        })
        .to_string();
        for attempt in 0..12 {
            match self
                .agent
                .post(&self.url)
                .set("Authorization", &self.authorization)
                .set("Content-Type", "application/json")
                .send_string(&body)
            {
                Ok(response) => {
                    return self
                        .check_response(tool, response)
                        .map_err(|error| error.to_string())
                }
                Err(ureq::Error::Status(503, response)) => {
                    assert_eq!(response.header("Retry-After"), Some("1"));
                    assert!(response.into_string().unwrap().is_empty());
                    thread::sleep(Duration::from_secs(1 << attempt.min(2)));
                }
                Err(ureq::Error::Status(status, _)) => panic!("{tool}: unexpected HTTP {status}"),
                Err(ureq::Error::Transport(error)) => return Err(error.to_string()),
            }
        }
        panic!("{tool}: exhausted admission retries");
    }

    fn check_response(&self, tool: &str, response: ureq::Response) -> std::io::Result<Reply> {
        assert_eq!(response.status(), 200);
        let body = response.into_string()?;
        let response: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert!(response.get("error").is_none(), "{tool}: {response}");
        let content = response["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let text = content[0]["text"].as_str().unwrap();
        assert!(
            !text.to_ascii_lowercase().contains("database is locked"),
            "{tool}: {text}"
        );
        assert_ne!(text, "Internal error", "{tool}");
        for marker in &self.forbidden {
            assert!(!text.contains(marker), "{tool}: leaked {marker}: {text}");
        }
        if response["result"]["isError"].as_bool().unwrap() {
            return Ok(Err(text.to_owned()));
        }
        let value = serde_json::from_str(text).unwrap();
        self.check_scope(&value);
        if tool == "spaces" && self.role != 0 {
            assert_eq!(value.as_array().unwrap().len(), 1);
            assert_eq!(value[0]["name"], "shared");
        }
        if let Some(audit) = self.audit {
            audit.observe(&value, self.account, self.role != 0);
        }
        self.check_payload(tool, &value);
        Ok(Ok(value))
    }

    fn check_payload(&self, tool: &str, value: &Value) {
        match tool {
            "access" => {
                assert_eq!(value["account"], ACCOUNTS[self.account]);
                assert_eq!(value["read_only"], self.role >= 2);
                let space = if self.role == 0 { "*" } else { "shared" };
                assert_eq!(
                    value["grants"],
                    json!([{"space":space,"write":self.role < 2}])
                );
                assert!(value["file_storage"]["bytes"].is_u64());
            }
            "recall" => {
                for hit in value.as_array().unwrap() {
                    assert!(hit["fact"]["id"].is_string());
                    assert_eq!(hit["fact"]["status"], "active");
                    assert!(hit["snippet"].is_string());
                }
            }
            "list" | "file_list" => {
                for item in value["items"].as_array().unwrap() {
                    assert!(item["id"].is_string());
                    assert_eq!(item["status"], "active");
                }
            }
            "remember" => assert!(value["fact"]["id"].is_string()),
            "remember_many" => {
                let results = value.as_array().unwrap();
                assert!(!results.is_empty());
                assert!(results.iter().all(|item| item["fact"]["id"].is_string()));
            }
            "read" | "file_put" | "file_get" => assert!(value["id"].is_string()),
            "history" => assert!(!value.as_array().unwrap().is_empty()),
            "retire" | "file_retire" | "space_create" => assert_eq!(value, &json!({})),
            _ => {}
        }
    }

    fn check_scope(&self, value: &Value) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| self.check_scope(item)),
            Value::Object(fields) => {
                if self.role != 0 {
                    if let Some(space) = fields.get("space") {
                        assert_eq!(space, "shared", "{value}");
                    }
                }
                fields.values().for_each(|item| self.check_scope(item));
            }
            _ => {}
        }
    }
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(10))
        .build()
}
