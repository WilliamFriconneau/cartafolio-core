//! A memory with one person and one vault, started late: the port is reserved and nobody listens on it until
//! [`Memory::up`], which is the computer being offline and then back.

// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used, dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use memcp_bridge::Config;
use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::{Brain, users};
use memcp_server::app::App;
use memcp_server::net::{Listeners, serve};
use serde_json::json;

pub struct Memory {
    pub app: Arc<App>,
    pub tokens: Vec<String>,
    pub port: u16,
    pub dir: PathBuf,
}

pub fn unique(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("memcp-bridge-{tag}-{}-{n}", std::process::id()))
}

/// A memory with one person and one vault, a key per entry of `rights`, and a port reserved but **not
/// listened on yet**: the computer is offline until [`Memory::up`].
pub fn memory(tag: &str, rights: &[&str]) -> Memory {
    let dir = unique(tag);
    let brain = Brain::init(&dir, &[], Language::English).expect("init");
    let tokens = {
        let ctl = brain.open_control().expect("control");
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .expect("user")
            .id;
        brain.add_vault(&sam, "sam", "me").expect("vault");
        rights
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let r = Rights::parse(spec).expect("rights");
                access::connect_for(&ctl, &sam, &format!("key-{i}"), &r, None, false, false)
                    .expect("connect")
            })
            .collect()
    };
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    Memory {
        app: Arc::new(App::new(brain)),
        tokens,
        port,
        dir,
    }
}

impl Memory {
    pub fn url(&self, i: usize) -> String {
        format!("http://127.0.0.1:{}/mcp/key-{i}", self.port)
    }

    /// The network comes back.
    pub fn up(&self) {
        let (app, port) = (Arc::clone(&self.app), self.port);
        std::thread::spawn(move || {
            let l = Listeners {
                listen: format!("127.0.0.1:{port}"),
                identity: None,
                ingress: None,
                threads: 2,
            };
            let _ = serve(app, &l, "");
        });
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn config(&self, i: usize, key: Option<&str>, outbox: &Path) -> Config {
        Config {
            url: self.url(i),
            key: key.unwrap_or(&self.tokens[i]).to_string(),
            outbox: outbox.to_path_buf(),
            utc_offset_minutes: 60,
        }
    }

    /// What the memory holds under `tag`, found by `words` (the search is by words, and a stop word finds nothing).
    pub fn texts(&self, tag: &str, words: &[&str]) -> Vec<String> {
        let mut c = memcp_client::Client::new(&self.url(1), &self.tokens[1]);
        c.retry.tries = 1;
        let mut out: Vec<String> = Vec::new();
        for w in words {
            let a = c
                .call(
                    "recall",
                    json!({ "space": "sam-me", "query": w, "tags": [tag], "limit": 50 }),
                )
                .expect("recall");
            for t in a["results"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r["text"].as_str())
            {
                if !out.iter().any(|o| o == t) {
                    out.push(t.to_string());
                }
            }
        }
        out
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn m4a(size: usize) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftypM4A \0\0\0\0");
    v.resize(size, 7);
    v
}

pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).expect("dir");
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write");
    p
}
