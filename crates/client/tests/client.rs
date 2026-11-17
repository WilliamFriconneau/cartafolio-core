//! The client against a scripted memory (a small HTTP server in the test) and against the real engine.

// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use memcp_client::{BATCH, Client, Error, allowed_url};
use serde_json::{Value, json};

struct Seen {
    authorization: String,
    session: String,
    body: Value,
}

enum Reply {
    Json(String),
    JsonWithSession(&'static str, String),
    Sse(String),
    Status(u16, &'static str),
}

fn tool_ok(text: &Value) -> Reply {
    Reply::Json(
        json!({"jsonrpc":"2.0","id":0,"result":{"content":[{"type":"text","text":text.to_string()}]}}).to_string(),
    )
}

/// A memory that answers the script, one reply per request, and remembers what it saw. The ids in the
/// replies are copied from the request, so the script does not count them.
fn mock(script: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("listen");
    let port = server.server_addr().to_ip().expect("ip").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for reply in script {
            let Ok(mut rq) = server.recv() else { return };
            let mut text = String::new();
            let _ = rq.as_reader().read_to_string(&mut text);
            let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let header = |name: &str| {
                rq.headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default()
            };
            let (authorization, session) = (header("authorization"), header("mcp-session-id"));
            let id = body["id"].clone();
            log.lock().expect("log").push(Seen {
                authorization,
                session,
                body,
            });
            let h = |k: &str, v: &str| tiny_http::Header::from_bytes(k, v).expect("header");
            let with_id = |t: String| {
                let mut v: Value = serde_json::from_str(&t).expect("json");
                v["id"] = id.clone();
                v.to_string()
            };
            let _ = match reply {
                Reply::Json(t) => rq.respond(
                    tiny_http::Response::from_string(with_id(t))
                        .with_header(h("Content-Type", "application/json")),
                ),
                Reply::JsonWithSession(s, t) => rq.respond(
                    tiny_http::Response::from_string(with_id(t))
                        .with_header(h("Content-Type", "application/json"))
                        .with_header(h("Mcp-Session-Id", s)),
                ),
                Reply::Sse(t) => rq.respond(
                    tiny_http::Response::from_string(format!(
                        "event: message\ndata: {}\n\n",
                        with_id(t)
                    ))
                    .with_header(h("Content-Type", "text/event-stream")),
                ),
                Reply::Status(code, retry) => {
                    let mut r = tiny_http::Response::from_string("{}").with_status_code(code);
                    if !retry.is_empty() {
                        r = r.with_header(h("Retry-After", retry));
                    }
                    rq.respond(r)
                }
            };
        }
    });
    (format!("http://127.0.0.1:{port}/mcp/test"), seen)
}

fn init() -> Vec<Reply> {
    vec![
        Reply::JsonWithSession(
            "s1",
            json!({"jsonrpc":"2.0","id":0,"result":{}}).to_string(),
        ),
        Reply::Status(202, ""),
    ]
}

fn fast(c: &mut Client) {
    c.retry.first = Duration::from_millis(1);
    c.retry.longest = Duration::from_millis(5);
}

#[test]
fn a_call_starts_a_session_sends_the_key_and_gives_the_answer_as_json() {
    let mut script = init();
    script.push(tool_ok(&json!({"result":"created","fact":{"id":"f_1"}})));
    let (url, seen) = mock(script);
    let mut c = Client::new(&url, "hcp_key");
    let answer = c.call("remember", json!({"text":"x"})).expect("answer");
    assert_eq!(answer["fact"]["id"], "f_1");
    let seen = seen.lock().expect("seen");
    let methods: Vec<&str> = seen
        .iter()
        .map(|s| s.body["method"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        methods,
        ["initialize", "notifications/initialized", "tools/call"]
    );
    assert!(seen.iter().all(|s| s.authorization == "Bearer hcp_key"));
    assert_eq!(
        seen[2].session, "s1",
        "the session the memory gave comes back"
    );
    assert_eq!(seen[2].body["params"]["name"], "remember");
}

#[test]
fn an_answer_in_a_stream_is_found_by_its_id() {
    let mut script = init();
    script.push(Reply::Sse(
        json!({"jsonrpc":"2.0","id":0,"result":{"content":[{"type":"text","text":"{\"ok\":true}"}]}}).to_string(),
    ));
    let (url, _) = mock(script);
    let mut c = Client::new(&url, "k");
    assert_eq!(c.call("stats", json!({})).expect("answer")["ok"], true);
}

#[test]
fn a_memory_that_fails_is_tried_again_and_a_refused_key_never_is() {
    let mut script = init();
    script.push(Reply::Status(503, ""));
    script.push(Reply::Status(502, ""));
    script.push(tool_ok(&json!({"ok":1})));
    let (url, seen) = mock(script);
    let mut c = Client::new(&url, "k");
    fast(&mut c);
    assert_eq!(
        c.call("stats", json!({})).expect("after two failures")["ok"],
        1
    );
    assert_eq!(seen.lock().expect("seen").len(), 5);

    let (url, seen) = mock(vec![Reply::Status(401, "")]);
    let mut c = Client::new(&url, "revoked");
    fast(&mut c);
    assert!(matches!(c.call("stats", json!({})), Err(Error::Refused)));
    assert_eq!(seen.lock().expect("seen").len(), 1, "no second try");
}

#[test]
fn a_rate_limit_is_waited_out_as_the_memory_says_and_then_tried_again() {
    let mut script = init();
    script.push(Reply::Status(429, "0"));
    script.push(tool_ok(&json!({"ok":2})));
    let (url, seen) = mock(script);
    let mut c = Client::new(&url, "k");
    fast(&mut c);
    assert_eq!(c.call("stats", json!({})).expect("after the wait")["ok"], 2);
    assert_eq!(seen.lock().expect("seen").len(), 4);
}

#[test]
fn a_refusal_in_the_memorys_own_words_is_not_retried() {
    let mut script = init();
    script.push(Reply::Json(
        json!({"jsonrpc":"2.0","id":0,"result":{"isError":true,"content":[{"type":"text","text":"this connection writes only under inbox"}]}}).to_string(),
    ));
    let (url, seen) = mock(script);
    let mut c = Client::new(&url, "k");
    fast(&mut c);
    match c.call("remember", json!({"text":"x"})) {
        Err(Error::Tool(m)) => assert!(m.contains("only under inbox")),
        other => panic!("{other:?}"),
    }
    assert_eq!(seen.lock().expect("seen").len(), 3);
}

#[test]
fn a_memory_that_stays_down_ends_in_an_error_after_the_tries() {
    let (url, _) = mock(vec![]);
    let mut c = Client::new(&url, "k");
    fast(&mut c);
    c.retry.tries = 2;
    assert!(matches!(
        c.call("stats", json!({})),
        Err(Error::Unreachable(_))
    ));
}

#[test]
fn a_long_list_of_records_goes_in_batches_with_the_positions_of_the_whole_list() {
    let mut script = init();
    for (n, count) in [(0, BATCH), (1, BATCH), (2, 50)] {
        let results: Vec<Value> = (0..count)
            .map(|i| {
                if n == 1 && i == 3 {
                    json!({"index": i, "result":"error", "error":"no"})
                } else {
                    json!({"index": i, "result":"created", "id": format!("f_{n}_{i}")})
                }
            })
            .collect();
        script.push(tool_ok(&json!({ "results": results, "failed": 0 })));
    }
    let (url, seen) = mock(script);
    let mut c = Client::new(&url, "k");
    let total = 2 * BATCH + 50;
    let records: Vec<Value> = (0..total)
        .map(
            |i| json!({"text": format!("t{i}"), "origin": {"app":"healthapp","id": format!("n/{i}")}}),
        )
        .collect();
    let d = c.deposit("owner-health", &records).expect("deposited");
    assert_eq!(d.results.len(), total);
    assert_eq!(d.failed, 1);
    assert_eq!(
        d.results[BATCH + 3]["result"],
        "error",
        "a record's place in the whole list"
    );
    assert_eq!(d.results[total - 1]["index"], total - 1);
    let seen = seen.lock().expect("seen");
    let calls: Vec<&Seen> = seen
        .iter()
        .filter(|s| s.body["method"] == "tools/call")
        .collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[0].body["params"]["arguments"]["records"]
            .as_array()
            .map(Vec::len),
        Some(BATCH)
    );
    assert_eq!(
        calls[2].body["params"]["arguments"]["records"]
            .as_array()
            .map(Vec::len),
        Some(50)
    );
    assert_eq!(calls[0].body["params"]["name"], "deposit");
}

/// The real engine, with a key limited to `inbox` and one limited to an app's paths.
fn engine(rights: &[(&str, &str)]) -> (String, Vec<String>, std::path::PathBuf) {
    use memcp_core::access::{self, Rights};
    use memcp_core::text::Language;
    use memcp_core::{Brain, users};
    use memcp_server::app::App;
    use memcp_server::net::{Listeners, serve_on};

    let dir = std::env::temp_dir().join(format!(
        "memcp-client-{}-{}",
        std::process::id(),
        rights.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[], Language::English).expect("init");
    {
        let ctl = brain.open_control().expect("control");
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .expect("user")
            .id;
        brain.add_vault(&sam, "sam", "me").expect("vault");
    }
    let tokens: Vec<String> = {
        let ctl = brain.open_control().expect("control");
        let sam = users::resolve(&ctl, "sam").expect("sam").id;
        rights
            .iter()
            .map(|(agent, spec)| {
                let r = Rights::parse(spec).expect("rights");
                access::connect_for(&ctl, &sam, agent, &r, None, false, false).expect("connect")
            })
            .collect()
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let app = Arc::new(App::new(brain));
    std::thread::spawn(move || {
        let l = Listeners {
            listen: format!("127.0.0.1:{port}"),
            identity: None,
            ingress: None,
            threads: 2,
        };
        let _ = serve_on(app, listener, &l, "");
    });
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    (format!("http://127.0.0.1:{port}/mcp"), tokens, dir)
}

#[test]
fn against_the_real_engine_a_feed_and_a_recording_are_kept_under_the_keys_own_paths() {
    let (base, tokens, dir) = engine(&[
        ("healthapp", "sam-me/sources/healthapp:rw"),
        ("device", "sam-me/inbox:rw"),
    ]);
    // An app's feed: 1030 records in three calls, and the same again changes nothing.
    let mut feed = Client::new(&format!("{base}/healthapp"), &tokens[0]);
    let records: Vec<Value> = (0..2 * BATCH + 30)
        .map(|i| json!({"text": format!("[HealthApp] note e{i}: sommeil {i}"), "origin": {"app":"healthapp","id": format!("note/e{i}"),"at":"2026-11-02"}}))
        .collect();
    let d = feed.deposit("sam-me", &records).expect("first send");
    assert_eq!((d.results.len(), d.failed), (2 * BATCH + 30, 0));
    assert!(d.results.iter().all(|r| r["result"] == "created"));
    let again = feed.deposit("sam-me", &records).expect("second send");
    assert!(again.results.iter().all(|r| r["result"] == "unchanged"));
    // The inbox key: a note, and a recording of about 5 MB, then the refusal outside its path.
    let mut intake = Client::new(&format!("{base}/device"), &tokens[1]);
    intake
        .call(
            "remember",
            json!({"space":"sam-me","text":"Une idée jetée vite."}),
        )
        .expect("a note");
    let mut m4a = vec![0, 0, 0, 0x20];
    m4a.extend_from_slice(b"ftypM4A \0\0\0\0");
    m4a.resize(5 * 1024 * 1024, 7);
    let kept = intake
        .file_put(
            "sam-me",
            "2026-11-02-10h00-idee.m4a",
            "dictée",
            &["inbox/audio"],
            &m4a,
        )
        .expect("recording");
    assert_eq!(kept["file"]["mime"], "audio/mp4");
    match intake.call(
        "remember",
        json!({"space":"sam-me","text":"hors chemin","tags":["work"]}),
    ) {
        Err(Error::Tool(m)) => assert!(m.contains("only under inbox"), "{m}"),
        other => panic!("{other:?}"),
    }
    let found = intake
        .call(
            "recall",
            json!({"space":"sam-me","query":"idée","tags":["inbox"]}),
        )
        .expect("recall");
    // The note, and the describing fact of the recording (its name holds the word).
    assert_eq!(found["results"].as_array().map(Vec::len), Some(2));
    // A wrong key is refused at once.
    let mut wrong = Client::new(&format!("{base}/device"), "hcp_wrong");
    assert!(matches!(
        wrong.call("stats", json!({})),
        Err(Error::Refused)
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_key_only_travels_in_clear_on_the_own_network() {
    for ok in [
        "https://serveur.tail1234.ts.net/mcp/x",
        "http://127.0.0.1:8791/mcp/x",
        "http://localhost:8791/mcp/x",
        "http://192.168.0.20:8791/mcp/dictee",
        "http://10.0.0.5/mcp/x",
        "http://172.20.1.1/mcp/x",
        "http://100.64.0.1:4000/v1/chat/completions",
        "http://serveur.local/mcp/x",
        "http://nas.lan/mcp/x",
    ] {
        assert!(allowed_url(ok), "{ok}");
    }
    for no in [
        "http://example.com/mcp/x",
        "http://172.32.0.1/mcp/x",
        "http://100.63.0.1/mcp/x",
        "http://100.128.0.1/mcp/x",
        "ftp://example.com/x",
        "example.com",
        "http://192.168.0.20.example.com/mcp/x",
    ] {
        assert!(!allowed_url(no), "{no}");
    }
}

#[test]
fn a_recording_is_read_back_byte_for_byte() {
    let (base, tokens, dir) = engine(&[("device", "sam-me:rw")]);
    let mut c = Client::new(&format!("{base}/device"), &tokens[0]);
    let mut m4a = vec![0, 0, 0, 0x20];
    m4a.extend_from_slice(b"ftypM4A \0\0\0\0");
    m4a.resize(3 * 1024 * 1024, 9);
    let kept = c
        .file_put(
            "sam-me",
            "2026-11-02-10h00-idee.m4a",
            "dictée",
            &["inbox/audio"],
            &m4a,
        )
        .expect("kept");
    let id = kept["file"]["id"].as_str().expect("id");
    let (meta, back) = c.file_get(id).expect("read back");
    assert_eq!(meta["name"], "2026-11-02-10h00-idee.m4a");
    assert_eq!(meta["mime"], "audio/mp4");
    assert_eq!(back, m4a);
    assert!(matches!(c.file_get("f_nope"), Err(Error::Tool(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_device_pairs_with_a_code_and_its_key_works_once() {
    use memcp_core::text::Language;
    use memcp_core::{Brain, pairing, users};
    use memcp_server::app::App;
    use memcp_server::net::{Listeners, serve_on};

    let dir = std::env::temp_dir().join(format!("memcp-client-pair-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[], Language::English).expect("init");
    let code = {
        let ctl = brain.open_control().expect("control");
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .expect("user")
            .id;
        brain.add_vault(&sam, "sam", "me").expect("vault");
        pairing::create_for_peer(&brain, &sam, "sam", memcp_core::util::now())
            .expect("code")
            .0
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let mut app = App::new(brain);
    app.public_url = format!("http://127.0.0.1:{port}");
    let app = Arc::new(app);
    std::thread::spawn(move || {
        let l = Listeners {
            listen: format!("127.0.0.1:{port}"),
            identity: None,
            ingress: None,
            threads: 2,
        };
        let _ = serve_on(app, listener, &l, "");
    });
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let server = format!("http://127.0.0.1:{port}");
    let p = memcp_client::pair(&server, &code.to_lowercase(), "Sam's laptop").expect("paired");
    // A peer: every space of the person, read and write.
    assert!(
        p.spaces.iter().any(|v| v.starts_with("sam-me")),
        "{:?}",
        p.spaces
    );
    assert!(p.agent.starts_with("peer-"), "{}", p.agent);
    assert!(p.url.ends_with(&format!("/mcp/{}", p.agent)));
    let mut c = Client::new(&p.url, &p.key);
    let r = c
        .call(
            "remember",
            json!({"space": "sam-me", "text": "Une note du portable."}),
        )
        .expect("write");
    // A peer writes like the person: nothing is forced into an inbox.
    assert!(r["fact"]["id"].is_string(), "{r}");
    match memcp_client::pair(&server, &code, "again") {
        Err(Error::Tool(m)) => assert!(m.contains("unknown, used or expired"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        memcp_client::pair("http://example.com", &code, "x"),
        Err(Error::Tool(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_octal_looking_address_is_not_the_own_network() {
    for no in [
        "http://010.0.0.1/mcp/x",
        "http://0127.0.0.1/mcp/x",
        "http://10.0.0.01/mcp/x",
        "http://192.168.1/mcp/x",
    ] {
        assert!(!allowed_url(no), "{no}");
    }
    assert!(allowed_url("http://10.0.0.1/mcp/x"));
    // The client itself refuses such an address, whoever forgot to check it.
    let mut c = Client::new("http://example.com/mcp/x", "hcp_secret");
    assert!(matches!(c.call("stats", json!({})), Err(Error::Tool(_))));
}

#[test]
fn an_error_never_names_the_memorys_address() {
    // Nothing listens there: the error says so without the address.
    let mut c = Client::new("http://127.0.0.1:9/mcp/secret-name", "hcp_x");
    c.retry.tries = 1;
    let e = c
        .call("stats", json!({}))
        .expect_err("unreachable")
        .to_string();
    assert!(
        !e.contains("127.0.0.1") && !e.contains("secret-name"),
        "{e}"
    );
    let refused = Client::new("http://example.com/mcp/x", "k")
        .call("stats", json!({}))
        .expect_err("refused")
        .to_string();
    assert!(!refused.contains("example.com"), "{refused}");
    let p = memcp_client::pair("http://example.com", "AAAA-AAAA", "x")
        .expect_err("refused")
        .to_string();
    assert!(!p.contains("example.com"), "{p}");
}

#[test]
fn a_program_may_take_an_address_in_clear_when_it_says_so() {
    let mut c = Client::new("http://127.0.0.1:9/mcp/x", "k");
    c.retry.tries = 1;
    // Allowed anyway (the loopback): unreachable, not refused.
    assert!(matches!(
        c.call("stats", json!({})),
        Err(Error::Unreachable(_))
    ));
    let mut far = Client::new("http://192.0.2.1:9/mcp/x", "k");
    far.retry.tries = 1;
    assert!(
        matches!(far.call("stats", json!({})), Err(Error::Tool(_))),
        "refused by default"
    );
    far.allow_clear = true;
    far.retry = memcp_client::Retry {
        tries: 1,
        first: Duration::ZERO,
        longest: Duration::ZERO,
    };
    assert!(
        !matches!(far.call("stats", json!({})), Err(Error::Tool(_))),
        "taken when the program says so"
    );
}
