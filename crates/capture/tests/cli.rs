//! The `memcp-capture` program, run for real against the real engine.
#![cfg(unix)]
// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

mod common;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use common::{memory, unique, write};
use serde_json::{Value, json};

fn capture_cmd(args: &[&str], key: &str, stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_memcp-capture"));
    cmd.args(args)
        .env("MEMCP_TOKEN", key)
        .env_remove("MEMCP_URL")
        .env_remove("MEMCP_SPACE")
        .env_remove("MEMCP_OUTBOX")
        .env_remove("MEMCP_KEY_FILE")
        .env_remove("MEMCP_CAPTURE_DIRS")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn");
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes()).expect("stdin");
    }
    child.wait_with_output().expect("run")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_note_is_kept_while_offline_listed_and_sent_when_the_memory_is_back() {
    let mem = memory("cli-text", &["sam-me/inbox:rw", "sam-me:rw"]);
    let outbox = unique("outbox");
    let (url, o) = (mem.url(0), outbox.to_str().expect("path"));
    let args = |mode: &'static str| vec![mode, "--url", &url, "--outbox", o, "--utc-offset", "60"];
    let _ = args;
    let offline = capture_cmd(
        &[
            "text",
            "Une idée du terminal.",
            "--url",
            &url,
            "--outbox",
            o,
        ],
        &mem.tokens[0],
        None,
    );
    assert_eq!(offline.status.code(), Some(0), "{}", err(&offline));
    assert!(
        out(&offline).contains("kept ") && out(&offline).contains("cannot be reached"),
        "{}",
        out(&offline)
    );
    let status = capture_cmd(
        &["status", "--url", &url, "--outbox", o],
        &mem.tokens[0],
        None,
    );
    assert!(
        out(&status).contains("waiting") && out(&status).contains("Une idée du terminal."),
        "{}",
        out(&status)
    );

    mem.up();
    let flushed = capture_cmd(
        &["flush", "--url", &url, "--outbox", o],
        &mem.tokens[0],
        None,
    );
    assert_eq!(flushed.status.code(), Some(0));
    assert!(
        out(&flushed).contains("sent 1, waiting 0"),
        "{}",
        out(&flushed)
    );
    assert!(
        mem.texts("inbox", &["terminal"])
            .iter()
            .any(|t| t.ends_with("Une idée du terminal."))
    );
    let after = capture_cmd(
        &["status", "--url", &url, "--outbox", o],
        &mem.tokens[0],
        None,
    );
    assert!(out(&after).starts_with("sent"), "{}", out(&after));
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn files_are_sent_and_a_file_the_memory_refuses_exits_with_three() {
    let mem = memory("cli-send", &["sam-me/inbox:rw", "sam-me:rw"]);
    mem.up();
    let (work, outbox) = (unique("files"), unique("outbox"));
    let good = write(&work, "devis.txt", b"un devis");
    let bad = write(&work, "binaire.dat", &[0xFF, 0x00, 0xFE, 0x01]);
    let (url, o) = (mem.url(0), outbox.to_str().expect("path"));
    let ok = capture_cmd(
        &[
            "send",
            good.to_str().expect("p"),
            "--note",
            "devis du plombier",
            "--url",
            &url,
            "--outbox",
            o,
        ],
        &mem.tokens[0],
        None,
    );
    assert_eq!(ok.status.code(), Some(0), "{}{}", out(&ok), err(&ok));
    assert!(out(&ok).contains("sent 1"));
    let refused = capture_cmd(
        &[
            "send",
            bad.to_str().expect("p"),
            "--url",
            &url,
            "--outbox",
            o,
        ],
        &mem.tokens[0],
        None,
    );
    assert_eq!(refused.status.code(), Some(3), "{}", out(&refused));
    assert!(out(&refused).contains("refused by the memory 1"));
    let _ = std::fs::remove_dir_all((work, outbox).0);
}

#[test]
fn a_refused_key_exits_with_one_and_wrong_use_with_two() {
    let mem = memory("cli-refused", &["sam-me/inbox:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let (url, o) = (mem.url(0), outbox.to_str().expect("path"));
    let r = capture_cmd(
        &["text", "x", "--url", &url, "--outbox", o],
        "mtm_wrong",
        None,
    );
    assert_eq!(r.status.code(), Some(1), "{}", out(&r));
    assert!(out(&r).contains("REFUSED OR REVOKED") && out(&r).contains("waiting 1"));
    // Wrong use: no address, a space named (there is none to choose), no key; an address in clear is only warned about.
    let none = capture_cmd(&["status"], "k", None);
    assert_eq!(none.status.code(), Some(2));
    assert!(err(&none).contains("missing --url"));
    let clear = capture_cmd(
        &["status", "--url", "http://example.com/mcp/x", "--outbox", o],
        "k",
        None,
    );
    // Taken, with a warning: the capture program is the person's own relay, never a wall.
    assert_eq!(clear.status.code(), Some(0), "{}", err(&clear));
    assert!(
        err(&clear).contains("WARNING") && err(&clear).contains("in clear"),
        "{}",
        err(&clear)
    );
    let space = capture_cmd(
        &[
            "text", "x", "--url", &url, "--vault", "sam-me", "--outbox", o,
        ],
        "k",
        None,
    );
    assert_eq!(space.status.code(), Some(2));
    assert!(err(&space).contains("À trier"), "{}", err(&space));
    let nokey = capture_cmd(
        &[
            "status",
            "--url",
            &url,
            "--outbox",
            o,
            "--token-env",
            "MEMCP_NOT_SET",
        ],
        "k",
        None,
    );
    assert_eq!(nokey.status.code(), Some(2));
    assert!(err(&nokey).contains("no key"));
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn the_default_mode_is_the_gateway_on_standard_input_and_output() {
    let mem = memory("cli-gateway", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let (url, o) = (mem.url(0), outbox.to_str().expect("path"));
    let lines = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"remember","arguments":{"space":"sam-me","text":"Par la passerelle.","tags":["inbox"]}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    let r = capture_cmd(
        &["--url", &url, "--outbox", o],
        &mem.tokens[0],
        Some(&lines),
    );
    assert_eq!(r.status.code(), Some(0), "{}", err(&r));
    let replies: Vec<Value> = out(&r)
        .lines()
        .map(|l| serde_json::from_str(l).expect("only protocol on stdout"))
        .collect();
    assert_eq!(replies.len(), 3, "a notification gets no answer");
    assert!(
        replies[0]["result"]["instructions"]
            .as_str()
            .expect("instructions")
            .contains("kept here")
    );
    assert_eq!(replies[1]["id"], 2);
    assert!(
        replies[1]["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("created")
    );
    assert!(
        mem.texts("inbox", &["passerelle"])
            .iter()
            .any(|t| t == "Par la passerelle.")
    );
    let _ = std::fs::remove_dir_all(outbox);
}

/// A capture program serving over HTTP, killed when the test ends.
struct Serving(std::process::Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve_over_http(url: &str, key: &str, outbox: &str) -> (Serving, u16) {
    use std::io::{BufRead, BufReader};
    // Port 0: the system picks one, and the capture program says which on its standard error. No port is chosen here and
    // left free for another program to take.
    let mut child = Command::new(env!("CARGO_BIN_EXE_memcp-capture"))
        .args(["--url", url, "--outbox", outbox, "--listen", "127.0.0.1:0"])
        .env("MEMCP_TOKEN", key)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let stderr = child.stderr.take().expect("stderr");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    reader.read_line(&mut line).expect("banner");
    // Keep reading: a closed pipe would make the capture program's next message fail.
    std::thread::spawn(move || for _ in reader.lines() {});
    let port: u16 = line
        .rsplit(':')
        .next()
        .and_then(|rest| rest.split(',').next())
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));
    (Serving(child), port)
}

fn raw(port: u16, request: &str) -> String {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.write_all(request.as_bytes()).expect("write");
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn over_http_on_this_machine_a_program_uses_the_tools_without_holding_the_key() {
    let mem = memory("cli-http", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let (_serving, port) =
        serve_over_http(&mem.url(0), &mem.tokens[0], outbox.to_str().expect("path"));
    // Any MCP client may use it, and it needs no key: the capture program has it.
    let mut c = memcp_client::Client::new(&format!("http://127.0.0.1:{port}/mcp"), "no key needed");
    c.retry.tries = 1;
    let sent = c
        .call("capture_text", json!({"text": "Par l'écoute locale."}))
        .expect("capture");
    assert_eq!(sent["result"], "sent", "{sent}");
    let made = c
        .call(
            "remember",
            json!({"space": "sam-me", "text": "Une écriture ordinaire.", "tags": ["inbox"]}),
        )
        .expect("remember");
    assert_eq!(made["result"], "created");
    assert!(
        mem.texts("inbox", &["locale"])
            .iter()
            .any(|t| t.ends_with("Par l'écoute locale."))
    );

    // What a web page could try is refused: another Host, another Origin, another method.
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let post = |extra: &str| {
        format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    assert!(raw(port, &post("")).starts_with("HTTP/1.1 200"));
    assert!(raw(port, &post("Origin: https://evil.example\r\n")).starts_with("HTTP/1.1 403"));
    assert!(raw(port, &post("Origin: http://localhost:3000\r\n")).starts_with("HTTP/1.1 200"));
    let rebinding = format!(
        "POST /mcp HTTP/1.1\r\nHost: evil.example\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    assert!(raw(port, &rebinding).starts_with("HTTP/1.1 403"));
    assert!(
        raw(
            port,
            &format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
        )
        .starts_with("HTTP/1.1 405")
    );
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn the_bridge_refuses_to_listen_anywhere_but_on_this_machine() {
    let mem = memory("cli-listen", &["sam-me/inbox:rw"]);
    let outbox = unique("outbox");
    let o = outbox.to_str().expect("path");
    for addr in ["0.0.0.0:9999", "10.0.0.20:9999"] {
        let r = capture_cmd(
            &["--url", &mem.url(0), "--outbox", o, "--listen", addr],
            &mem.tokens[0],
            None,
        );
        assert_eq!(r.status.code(), Some(2), "{addr}");
        assert!(err(&r).contains("this machine only"), "{addr}: {}", err(&r));
    }
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn the_key_can_come_from_a_file_only_its_owner_reads_and_that_file_wins_over_the_environment() {
    use std::os::unix::fs::PermissionsExt;
    let mem = memory("cli-keyfile", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let (url, outbox) = (mem.url(0), unique("outbox"));
    let o = outbox.to_str().expect("path");
    let dir = unique("keydir");
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("key");
    std::fs::write(&file, format!("{}\n", mem.tokens[0])).expect("key");
    let f = file.to_str().expect("path");
    let args = |extra: &[&'static str]| {
        let mut v = vec![
            "text",
            "Une note par fichier de clé.",
            "--url",
            &url,
            "--outbox",
            o,
            "--key-file",
            f,
        ];
        v.extend_from_slice(extra);
        v
    };
    // A file the group can read is refused, and nothing is sent.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).expect("chmod");
    let open = capture_cmd(&args(&[]), "ignored", None);
    assert_eq!(open.status.code(), Some(2), "{}", out(&open));
    assert!(err(&open).contains("chmod 600"), "{}", err(&open));
    // Mode 600: it works, with a wrong key in the environment (the file is what is used).
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let sent = capture_cmd(&args(&[]), "mtm_wrong", None);
    assert_eq!(sent.status.code(), Some(0), "{}", err(&sent));
    assert!(
        mem.texts("inbox", &["fichier"])
            .iter()
            .any(|t| t.ends_with("Une note par fichier de clé."))
    );
    // A missing file is an error, not a fall back to the environment.
    let missing = capture_cmd(
        &[
            "text",
            "x",
            "--url",
            &url,
            "--outbox",
            o,
            "--key-file",
            "/nonexistent/key",
        ],
        &mem.tokens[0],
        None,
    );
    assert_eq!(missing.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(outbox);
    let _ = std::fs::remove_dir_all(dir);
}
