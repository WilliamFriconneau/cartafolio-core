//! An assistant that knows neither the memory's address nor its key: the settings file, the socket and `connect`; and a
//! folder absorbed into a vault, with names replaced by posts and proposals put down as files.
#![cfg(unix)]
// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

mod common;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{memory, unique};
use serde_json::{Value, json};

fn bridge() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_memcp-bridge"));
    for v in [
        "MEMCP_URL",
        "MEMCP_VAULT",
        "MEMCP_OUTBOX",
        "MEMCP_KEY_FILE",
        "MEMCP_CAPTURE_DIRS",
        "MEMCP_TOKEN",
    ] {
        c.env_remove(v);
    }
    c
}

fn private_dir(tag: &str) -> std::path::PathBuf {
    let d = unique(tag);
    std::fs::create_dir_all(&d).expect("dir");
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    d
}

fn settings(dir: &Path, body: &str, mode: u32) -> std::path::PathBuf {
    let f = dir.join("bridge.conf");
    std::fs::write(&f, body).expect("conf");
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(mode)).expect("chmod");
    f
}

#[test]
fn an_assistant_reaches_the_memory_through_a_socket_knowing_only_its_path() {
    let mem = memory("socket", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let dir = private_dir("socket");
    let conf_body = format!(
        "# the gateway's own settings\nurl = {}\nkey = {}\nvault = sam-me\noutbox = {}\n",
        mem.url(0),
        mem.tokens[0],
        dir.join("outbox").display()
    );
    // Readable by others: refused, and the address is not said.
    let open = settings(&dir, &conf_body, 0o644);
    let o = bridge()
        .args(["status", "--config"])
        .arg(&open)
        .output()
        .expect("run");
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("chmod 600"));
    let conf = settings(&dir, &conf_body, 0o600);
    // An address on the command line next to the file: refused.
    let o = bridge()
        .args(["status", "--url", "http://127.0.0.1:1/mcp/x", "--config"])
        .arg(&conf)
        .output()
        .expect("run");
    assert_eq!(o.status.code(), Some(2));

    let sock = dir.join("bridge.sock");
    let mut gw = bridge()
        .arg("--config")
        .arg(&conf)
        .arg("--socket")
        .arg(&sock)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("gateway");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let mode = std::fs::metadata(&sock)
        .expect("socket")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "only the gateway's user (or its group of two) may open it"
    );

    // The assistant's side: `connect PATH`, nothing else, no key, no address.
    let mut c = bridge()
        .arg("connect")
        .arg(&sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("connect");
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"remember","arguments":{"space":"sam-me","text":"Une idée par la socket.","tags":["inbox"]}}}),
    ];
    {
        let mut stdin = c.stdin.take().expect("stdin");
        for m in &msgs {
            writeln!(stdin, "{m}").expect("write");
        }
    }
    let out = c.wait_with_output().expect("wait");
    let lines: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert_eq!(lines.len(), 2, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        lines[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("created"),
        "{}",
        lines[1]
    );
    assert!(
        mem.texts("inbox", &["socket"])
            .iter()
            .any(|t| t.ends_with("Une idée par la socket."))
    );
    let _ = gw.kill();
    let _ = gw.wait();

    // A socket in a folder others can write to is refused.
    let shared = unique("shared");
    std::fs::create_dir_all(&shared).expect("dir");
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).expect("chmod");
    let o = bridge()
        .arg("--config")
        .arg(&conf)
        .arg("--socket")
        .arg(shared.join("s.sock"))
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(o.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(shared);
}

#[test]
fn a_folder_is_absorbed_new_text_only_names_become_posts_and_proposals_come_back_as_files() {
    let mem = memory(
        "absorb",
        &[
            "sam-me/inbox:rw,sam-me/proposals/pro:r,sam-me/guide:rw",
            "sam-me:rw",
        ],
    );
    mem.up();
    let dir = private_dir("absorb");
    let pro = dir.join("Notes");
    std::fs::create_dir_all(pro.join("Projects")).expect("dir");
    std::fs::create_dir_all(pro.join("Raw")).expect("dir");
    std::fs::create_dir_all(pro.join(".obsidian")).expect("dir");
    std::fs::write(pro.join("Projects/Alpha.md"), "# Décisions\nOn garde A.\n").expect("note");
    std::fs::write(pro.join("Raw/transcript.md"), "Jamais lu.\n").expect("note");
    std::fs::write(
        pro.join("Projects/Confidentiel budget.md"),
        "Jamais lu non plus.\n",
    )
    .expect("note");
    std::fs::write(
        dir.join("roles.md"),
        "| Nom | Poste |\n|---|---|\n| Camille Martin | responsable des achats |\n",
    )
    .expect("roles");
    let conf = settings(
        &dir,
        &format!(
            "url = {}\nkey = {}\noutbox = {}\nabsorb = {} -> sam-me\nabsorb_exclude = Raw\nabsorb_protect = confidentiel\nabsorb_roles = {}\ndeliver = sam-me proposals/pro -> {}\n",
            mem.url(0),
            mem.tokens[0],
            dir.join("state").display(),
            pro.display(),
            dir.join("roles.md").display(),
            pro.join("Ideas").display()
        ),
        0o600,
    );
    let run = |extra: &[&str]| {
        let o = bridge()
            .arg("absorb")
            .args(extra)
            .arg("--config")
            .arg(&conf)
            .output()
            .expect("run");
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    // First pass: stock only, nothing sent (the folder was imported before).
    let (code, out, err) = run(&[]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(
        out.contains("pieces sent 0") && out.contains("taken stock of 1"),
        "{out}"
    );
    // A change: only the new text goes, with the name replaced by the post.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(
        pro.join("Projects/Alpha.md"),
        "# Décisions\nOn garde A.\n\n# Suite\nCamille Martin valide le budget le 2026-11-10.\n",
    )
    .expect("note");
    let (code, out, err) = run(&[]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("pieces sent 1"), "{out}");
    let inbox = mem.texts("inbox/absorb", &["budget"]);
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert!(
        inbox[0].contains("responsable des achats valide le budget")
            && !inbox[0].contains("Camille"),
        "{inbox:?}"
    );
    assert!(
        inbox[0].starts_with("D'après la note Projects/Alpha.md"),
        "{inbox:?}"
    );
    // Nothing new: nothing sent.
    let (_, out, _) = run(&[]);
    assert!(
        out.contains("pieces sent 0") && out.contains("files read 0"),
        "{out}"
    );
    // A proposal noted for the work assistant is put down as a file, once.
    let mut owner = memcp_client::Client::new(&mem.url(1), &mem.tokens[1]);
    owner
        .call("remember", json!({"space":"sam-me","text":"Idée : un tableau de bord des tickets par client.","tags":["proposals/pro"]}))
        .expect("proposal");
    let (_, out, _) = run(&[]);
    assert!(out.contains("1 put down as files"), "{out}");
    // Its guide waits for the person's approval on the page, as an app's guide.
    let ctl = mem.app.brain.open_control().expect("control");
    let waiting: i64 = mem
        .app
        .brain
        .open_space(&memcp_core::Space::new("sam-me").expect("space"))
        .expect("vault")
        .query_row(
            "SELECT count(*) FROM app_guides WHERE app = 'key-0' AND text LIKE '%proposals/pro%'",
            [],
            |r| r.get(0),
        )
        .expect("guides");
    drop(ctl);
    assert_eq!(waiting, 1);
    let files: Vec<_> = std::fs::read_dir(pro.join("Ideas"))
        .expect("ideas")
        .flatten()
        .collect();
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(files[0].path()).expect("idea");
    assert!(
        text.contains("source: mémoire") && text.contains("tableau de bord des tickets"),
        "{text}"
    );
    let (_, out, _) = run(&[]);
    assert!(
        out.contains("0 put down as files") && out.contains("pieces sent 0"),
        "the idea file is not sent back: {out}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
