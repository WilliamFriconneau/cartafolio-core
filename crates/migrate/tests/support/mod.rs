use axum::{extract::State, routing::post, Json, Router};
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine;
use pensine_access::{Account, Accounts};
use pensine_server::{Config, Handle, Server};
use serde_json::{json, Value};
use std::{
    process::{Command, Output},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

pub const OLD_KEY: &str = "fictional-secret-for-fixture";
const SPACES: [&str; 3] = ["root", "child", "capture"];

#[derive(Default)]
struct Old {
    delta: AtomicUsize,
    todo: AtomicUsize,
    refuse: AtomicUsize,
    bad_note: AtomicUsize,
    fail: AtomicUsize,
}

pub struct Fixture {
    pub account: Account,
    old: Arc<Old>,
    from: String,
    to: Handle,
    key: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(dir.path()).unwrap();
        let account = accounts.account_create("fixture").unwrap();
        let (_, key) = accounts
            .connection_create_owner("fixture", "Migration")
            .unwrap();
        let to = Server::start(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.path().into(),
            ..Config::default()
        })
        .await
        .unwrap();
        let old = Arc::new(Old::default());
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let from = format!("http://{}/mcp", socket.local_addr().unwrap());
        let app = Router::new()
            .route("/mcp", post(old_call))
            .with_state(old.clone());
        tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        Self {
            account,
            old,
            from,
            to,
            key: key.as_str().into(),
            _dir: dir,
        }
    }

    pub fn run(&self, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pensine-migrate"))
            .args([
                "--from",
                &self.from,
                "--from-key-env",
                "FIXTURE_OLD_KEY",
                "--to",
                &format!("http://{}/mcp", self.to.local_addr()),
                "--to-key-env",
                "FIXTURE_NEW_KEY",
                "--batch",
                "29",
            ])
            .env("FIXTURE_OLD_KEY", OLD_KEY)
            .env("FIXTURE_NEW_KEY", &self.key)
            .args(extra)
            .output()
            .unwrap()
    }

    pub fn delta(&self) {
        self.old.delta.store(1, Ordering::Relaxed);
    }
    pub fn old_todo_note(&self) {
        self.old.todo.store(1, Ordering::Relaxed);
    }
    pub fn refuse(&self) {
        self.old.refuse.store(1, Ordering::Relaxed);
    }
    pub fn reject_one_note(&self) {
        self.old.bad_note.store(1, Ordering::Relaxed);
    }
    pub fn fail_requests(&self, n: usize) {
        self.old.fail.store(n, Ordering::Relaxed);
    }
    pub fn failed_requests(&self) -> usize {
        self.old.fail.load(Ordering::Relaxed)
    }
    pub fn new_key(&self) -> &str {
        &self.key
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.to.shutdown();
    }
}

fn fact(space: &str, n: usize) -> Value {
    json!({
        "id":format!("{space}-{n}"), "space":space, "text":format!("Fictional {space} entry {n}"),
        "type":"note", "created_at":1000+n, "author":"Synthetic scribe",
        "tags": if n == 0 { vec!["ÉTÉ", "bad tag", if space == "capture" {"inbox"} else {"sample"}] } else { vec!["sample"] },
        "status": if n == 999 {"retired"} else {"active"}
    })
}

fn todo_fact(space: &str) -> Value {
    json!({
        "id":format!("{space}-todo"), "space":space, "text":"Fictional todo entry",
        "type":"note", "created_at":500, "author":"Synthetic scribe",
        "tags":["TODO"], "status":"active"
    })
}

async fn old_call(
    State(old): State<Arc<Old>>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Response {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(format!("Bearer {OLD_KEY}").as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if old
        .fail
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .is_ok()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let args = &req["params"]["arguments"];
    let space = args["space"].as_str().unwrap_or_default();
    let mut error = old.refuse.load(Ordering::Relaxed) == 1;
    let mut binary = None;
    let data = if error {
        json!(OLD_KEY)
    } else {
        match req["params"]["name"].as_str().unwrap() {
            "spaces" => json!({"spaces":[
                {"name":"child","label":"Child","parent":"root","kind":"general"},
                {"name":"root","label":"Root","kind":"general"},
                {"name":"capture","label":"Inbox","kind":"inbox"}
            ]}),
            "space_view" => {
                let offset = args["cursor"]
                    .as_str()
                    .map_or(0, |x| x.parse::<usize>().unwrap());
                assert_eq!(args["view"], "chronology");
                let todo = old.todo.load(Ordering::Relaxed) == 1;
                let count = 104 + old.delta.load(Ordering::Relaxed) + usize::from(todo);
                let end = (offset + 35).min(count);
                let items: Vec<_> = (offset..end)
                    .map(|i| {
                        if todo && i == 0 {
                            let mut note = todo_fact(space);
                            note["text"] = json!("truncated");
                            return note;
                        }
                        let mut note = fact(space, count - 1 - i);
                        note["text"] = json!("truncated");
                        note
                    })
                    .collect();
                json!({"items":items,"page":{"next_cursor": (end < count).then(||end.to_string())}})
            }
            "read_fact" => {
                let id = args["id"].as_str().unwrap();
                let value = match id.strip_suffix("-todo") {
                    Some(space) => todo_fact(space),
                    None => {
                        let (space, n) = id.rsplit_once('-').unwrap();
                        let mut value = fact(space, n.parse().unwrap());
                        if n == "2" && old.bad_note.load(Ordering::Relaxed) == 1 {
                            value["text"] = json!("x".repeat(200_001));
                        }
                        value
                    }
                };
                json!({"fact":value})
            }
            "stats" => json!({"root":{"retired":1},"child":{"retired":1},"capture":{"retired":1}}),
            "recall" => {
                assert_eq!(args["include_retired"], true);
                json!({"results":[fact(space,999)]})
            }
            "file_list" => json!({"files": if space == SPACES[0] { vec![
            json!({"id":"file-small","name":"sample.txt","mime":"text/plain","size":7,"note":"Fixture"}),
            json!({"id":"file-large","name":"large.bin","mime":"application/octet-stream","size":8000001})
        ] } else { vec![] }}),
            "file_get" if args["id"] == "file-small" => {
                binary = Some(json!({"type":"text","text":"example"}));
                json!({"file":{"id":"file-small"}})
            }
            "file_get" => {
                binary = Some(
                    json!({"type":"resource","resource":{"blob":base64::engine::general_purpose::STANDARD.encode(vec![0u8; 8_000_001])}}),
                );
                json!({"file":{"id":"file-large"}})
            }
            _ => {
                error = true;
                json!({})
            }
        }
    };
    let mut content = vec![json!({"type":"text","text":data.to_string()})];
    content.extend(binary);
    Json(json!({"jsonrpc":"2.0","id":req["id"],"result":{"content":content,"isError":error}}))
        .into_response()
}
