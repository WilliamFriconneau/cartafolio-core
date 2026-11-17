//! The command line against a node that is running (docs/CLI.md): `memcp --node ADDRESS --key-file FILE <command>`.
//!
//! The commands are the same, and so is what they print. A console key opens the node's own routes (`/admin/api/...`): most
//! commands go through `POST /admin/api/cli`, which runs the very function the local command line runs, on the node's
//! data, with the rights of the key's person; the files a command names are sent first, a chunk at a time, and an export
//! comes down as a stream. A connection key (`--agent NAME`) opens MCP tools: the note commands and `space list`.
//!
//! Exit codes: 0 done; 1 the command failed; 2 the command line is wrong; 3 the node refused the key; 4 the key lacks the
//! rights for this command; 5 the node cannot be reached; 6 the node answered something unexpected.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::cli::{Failure, bad};
use crate::cli_table::{self, Remote, Up};
use crate::commands::{Args, VALUE_FLAGS};

pub const EXIT_REFUSED: u8 = 3;
pub const EXIT_RIGHTS: u8 = 4;
pub const EXIT_UNREACHABLE: u8 = 5;
pub const EXIT_ODD: u8 = 6;

/// The options that choose the node and say how to talk to it; the node never sees them.
pub const OPTIONS: &[&str] = &["--node", "--key-file", "--key", "--agent"];
/// One chunk of a file sent to the node.
const CHUNK: usize = 8 * 1024 * 1024;

/// Which node, with which key.
pub struct Target {
    pub node: String,
    key: String,
    pub agent: Option<String>,
    pub json: bool,
}

/// The target when the command is to run on a node: `--node`, or `MEMCP_NODE`, unless `--local` says otherwise.
pub fn target(a: &Args) -> Result<Option<Target>, Failure> {
    if a.switches.iter().any(|s| s == "--local") {
        return Ok(None);
    }
    let node = match a.flags.get("--node") {
        Some(n) => n.clone(),
        None => match std::env::var("MEMCP_NODE") {
            Ok(n) if !n.trim().is_empty() => n,
            _ => return Ok(None),
        },
    };
    if a.flags.contains_key("--data") {
        return Err(bad(
            "--data works on a data directory, --node on a running node: give one of them (--local forces the directory)",
        ));
    }
    let node = node.trim().trim_end_matches('/').to_string();
    if !(node.starts_with("http://") || node.starts_with("https://")) {
        return Err(bad("--node: an address such as http://127.0.0.1:8791"));
    }
    let from_file = |f: &str| {
        std::fs::read_to_string(f)
            .map(|t| t.trim().to_string())
            .map_err(|e| bad(&format!("the key file {f}: {e}")))
    };
    let key = if let Some(f) = a.flags.get("--key-file") {
        from_file(f)?
    } else if let Some(k) = a.flags.get("--key") {
        k.trim().to_string()
    } else if let Ok(f) = std::env::var("MEMCP_KEY_FILE") {
        from_file(&f)?
    } else if let Ok(k) = std::env::var("MEMCP_KEY") {
        k.trim().to_string()
    } else {
        return Err(bad(
            "a node needs a key: --key-file FILE (or MEMCP_KEY_FILE), or MEMCP_KEY. A console key comes from `memcp console-token PERSON` on the node",
        ));
    };
    if key.is_empty() {
        return Err(bad("the key is empty"));
    }
    Ok(Some(Target {
        node,
        key,
        agent: a
            .flags
            .get("--agent")
            .cloned()
            .or_else(|| std::env::var("MEMCP_AGENT").ok())
            .filter(|s| !s.is_empty()),
        json: a.switches.iter().any(|s| s == "--json"),
    }))
}

/// The words the node is to run: what was typed, without the options that choose the node.
pub fn strip(raw: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut it = raw.iter();
    while let Some(x) = it.next() {
        if OPTIONS.contains(&x.as_str()) {
            it.next();
        } else if x == "--json" || x == "--local" {
        } else {
            out.push(x.clone());
            if VALUE_FLAGS.contains(&x.as_str())
                && let Some(v) = it.next()
            {
                out.push(v.clone());
            }
        }
    }
    out
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        // A redirect would resend the key where it points.
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .build()
        .into()
}

fn unreachable(t: &Target, e: impl std::fmt::Display) -> Failure {
    (
        EXIT_UNREACHABLE,
        format!("cannot reach the node at {}: {e}", t.node),
    )
}

/// What a status the command did not expect means, for the person.
fn refusal(status: u16, body: &str) -> Failure {
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(String::from))
        .unwrap_or_default();
    match status {
        401 => (
            EXIT_REFUSED,
            "the node refused the key: it is unknown, revoked or expired, or it is a connection key (that opens the \
             assistants' tools, add --agent NAME for it; the command line needs a console key, made with \
             `memcp console-token PERSON` on the node)"
                .into(),
        ),
        403 => (
            EXIT_RIGHTS,
            if said.is_empty() {
                "this key does not have the rights for this command".into()
            } else {
                format!("not allowed with this key: {said}")
            },
        ),
        404 if said.is_empty() => (
            EXIT_ODD,
            "the node has no such route: it is older than this program, or its page is not open on this address \
             (the node's --console-admin)"
                .into(),
        ),
        429 => (
            EXIT_ODD,
            "too many tries: wait a few minutes, then try again".into(),
        ),
        _ => (
            if (400..500).contains(&status) { 1 } else { EXIT_ODD },
            if said.is_empty() {
                format!("the node answered {status}")
            } else {
                said
            },
        ),
    }
}

/// One call on the node's routes: the status and the body.
fn call(
    t: &Target,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<(u16, String), Failure> {
    let url = format!("{}/admin/api{path}", t.node);
    let a = agent();
    let result = if method == "GET" {
        a.get(&url)
            .header("Authorization", &format!("Bearer {}", t.key))
            .header("X-Requested-With", "memcp")
            .call()
    } else {
        a.post(&url)
            .header("Authorization", &format!("Bearer {}", t.key))
            .header("X-Requested-With", "memcp")
            .header("Content-Type", "application/json")
            .send(body.unwrap_or(b"{}"))
    };
    let mut res = result.map_err(|e| unreachable(t, e))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| unreachable(t, e))?;
    Ok((status, text))
}

fn call_json(t: &Target, method: &str, path: &str, body: &Value) -> Result<Value, Failure> {
    let (status, text) = call(t, method, path, Some(body.to_string().as_bytes()))?;
    if status != 200 {
        return Err(refusal(status, &text));
    }
    serde_json::from_str(&text).map_err(|_| (EXIT_ODD, "the node's answer is not JSON".into()))
}

// --- sending the files a command names ---------------------------------------------------------------------------------

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files_under(&p, out)?;
        } else if p.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

/// The size of a chunk: 8 MiB, or `MEMCP_UPLOAD_CHUNK` bytes (between 1 KiB and 32 MiB) on a slow link, and for the tests.
fn chunk() -> usize {
    std::env::var("MEMCP_UPLOAD_CHUNK")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(CHUNK, |n| n.clamp(1024, 32 * 1024 * 1024))
}

fn send_file(t: &Target, id: &str, rel: &str, path: &Path) -> Result<(), Failure> {
    let mut f = std::fs::File::open(path).map_err(|e| bad(&format!("{}: {e}", path.display())))?;
    let mut offset: u64 = 0;
    let size = chunk();
    let mut buf = vec![0u8; size];
    loop {
        let mut n = 0;
        while n < size {
            let r = f
                .read(&mut buf[n..])
                .map_err(|e| bad(&format!("{}: {e}", path.display())))?;
            if r == 0 {
                break;
            }
            n += r;
        }
        let url = format!(
            "{}/admin/api/upload?id={id}&path={}&offset={offset}",
            t.node,
            pct(rel)
        );
        let mut res = agent()
            .post(&url)
            .header("Authorization", &format!("Bearer {}", t.key))
            .header("X-Requested-With", "memcp")
            .header("Content-Type", "application/octet-stream")
            .send(&buf[..n])
            .map_err(|e| unreachable(t, e))?;
        let status = res.status().as_u16();
        if status != 200 {
            let text = res.body_mut().read_to_string().unwrap_or_default();
            return Err(refusal(status, &text));
        }
        offset += n as u64;
        if n < size {
            return Ok(());
        }
    }
}

fn pct(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Sends a file or a folder of this machine and answers the word the node understands for it.
fn send_path(t: &Target, id: &str, what: &str) -> Result<String, Failure> {
    let p = Path::new(what);
    if p.is_dir() {
        let mut files = vec![];
        files_under(p, &mut files).map_err(|e| bad(&format!("{what}: {e}")))?;
        if files.is_empty() {
            return Err(bad(&format!("{what}: nothing in this folder")));
        }
        for f in &files {
            let rel = f
                .strip_prefix(p)
                .map_err(|_| bad("a path outside the folder"))?
                .to_string_lossy()
                .replace('\\', "/");
            send_file(t, id, &rel, f)?;
        }
        Ok(format!("@upload:{id}"))
    } else if p.is_file() {
        let name = p
            .file_name()
            .map_or("file".into(), |n| n.to_string_lossy().to_string());
        send_file(t, id, &name, p)?;
        Ok(format!("@upload:{id}/{name}"))
    } else {
        Err(bad(&format!("{what}: no such file or folder")))
    }
}

/// The raw words with the files they name replaced by what was sent for them. `f` is given each word that names a file
/// (its kind and text) and answers its replacement.
fn rewrite(
    raw: &[String],
    mut wanted: impl FnMut(Option<&str>, usize, &str) -> Result<Option<String>, Failure>,
) -> Result<Vec<String>, Failure> {
    let mut out = vec![];
    let mut pos = 0;
    let mut it = raw.iter();
    while let Some(x) = it.next() {
        if VALUE_FLAGS.contains(&x.as_str()) {
            out.push(x.clone());
            if let Some(v) = it.next() {
                out.push(wanted(Some(x), 0, v)?.unwrap_or_else(|| v.clone()));
            }
        } else if x.starts_with("--") || x == "-h" {
            out.push(x.clone());
        } else {
            out.push(wanted(None, pos, x)?.unwrap_or_else(|| x.clone()));
            pos += 1;
        }
    }
    Ok(out)
}

// --- running a command -------------------------------------------------------------------------------------------------

fn print_reply(t: &Target, v: &Value) -> Result<(), Failure> {
    if t.json {
        crate::sayln!("{v}");
    } else {
        crate::say!("{}", v["stdout"].as_str().unwrap_or(""));
        crate::esay!("{}", v["stderr"].as_str().unwrap_or(""));
    }
    match v["code"].as_i64().unwrap_or(1) {
        0 => Ok(()),
        // The message is already in what was printed.
        c => Err((u8::try_from(c).unwrap_or(1), String::new())),
    }
}

/// Runs the command of `raw` (the words as typed, `a` their reading) on the node.
pub fn run(t: &Target, raw: &[String], a: &Args) -> Result<(), Failure> {
    let normal = crate::commands::normalize(a);
    let Some(entry) = cli_table::find(&normal.pos) else {
        return Err(bad(&format!(
            "unknown command {:?}\n\n{}",
            normal.pos.first().map_or("", String::as_str),
            crate::commands::USAGE
        )));
    };
    if t.agent.is_some() {
        return connection_key(t, &normal, entry);
    }
    let words = strip(raw);
    match entry.remote {
        Remote::Local(why) => Err(bad(&format!(
            "`{}` works on the machine itself, not on a node: {why}. Run it without --node (or with --local).",
            entry.words
        ))),
        Remote::Direct => direct(t, &normal, entry.words),
        Remote::Download => export(t, &normal),
        Remote::Exec => exec(t, &words, a),
        Remote::Upload(ups) => {
            let sent = rewrite(&words, |flag, pos, word| {
                let hit = ups.iter().any(|u| match (u, flag) {
                    (Up::Flag(f), Some(g)) => *f == g,
                    (Up::Pos(i), None) => pos == *i,
                    (Up::PosFrom(i), None) => pos >= *i,
                    _ => false,
                });
                if !hit {
                    return Ok(None);
                }
                // Each file or folder of a command is sent under an upload of its own.
                send_path(t, &memcp_core::util::random_base32(16), word).map(Some)
            })?;
            let out = exec_words(t, &sent, a);
            // What was sent and not used (a refused command) is forgotten.
            for w in sent.iter().filter(|_| !crate::cli_remote::keeps_upload(a)) {
                if let Some(rest) = w.strip_prefix("@upload:") {
                    let id = rest.split('/').next().unwrap_or("");
                    let _ = call_json(t, "POST", "/upload/discard", &json!({ "id": id }));
                }
            }
            out
        }
    }
}

fn exec(t: &Target, words: &[String], a: &Args) -> Result<(), Failure> {
    exec_words(t, words, a)
}

/// `merge --plan EXPORT --out FILE`: the node prints the plan, this machine writes it where `--out` says.
fn exec_words(t: &Target, words: &[String], a: &Args) -> Result<(), Failure> {
    let out_file =
        if a.pos.first().map(String::as_str) == Some("merge") && a.flags.contains_key("--plan") {
            a.flags.get("--out").cloned()
        } else {
            None
        };
    let mut sent = words.to_vec();
    if out_file.is_some() {
        let mut v = vec![];
        let mut it = sent.iter();
        while let Some(x) = it.next() {
            if x == "--out" {
                it.next();
            } else {
                v.push(x.clone());
            }
        }
        sent = v;
    }
    let reply = call_json(t, "POST", "/cli", &json!({ "argv": sent }))?;
    if let (Some(path), 0) = (&out_file, reply["code"].as_i64().unwrap_or(1)) {
        std::fs::write(path, reply["stdout"].as_str().unwrap_or(""))
            .map_err(|e| (1, format!("{path}: {e}")))?;
        let mut shown = reply.clone();
        shown["stdout"] = json!("");
        shown["stderr"] = json!(format!(
            "{}plan written to {path}; nothing else was written.\n",
            reply["stderr"].as_str().unwrap_or("")
        ));
        return print_reply(t, &shown);
    }
    print_reply(t, &reply)
}

/// `export --to DIR`: the memory comes down as a stream and is written here as it arrives.
fn export(t: &Target, a: &Args) -> Result<(), Failure> {
    let to = a.flags.get("--to").ok_or_else(|| bad("missing --to DIR"))?;
    if a.flags.contains_key("--user") {
        return Err(bad(
            "a key exports the memory of its own person: drop --user (or run the command on the node's machine)",
        ));
    }
    let url = format!("{}/admin/api/export", t.node);
    let mut res = agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {}", t.key))
        .header("X-Requested-With", "memcp")
        .call()
        .map_err(|e| unreachable(t, e))?;
    let status = res.status().as_u16();
    if status != 200 {
        let text = res.body_mut().read_to_string().unwrap_or_default();
        return Err(refusal(status, &text));
    }
    let dest = Path::new(to);
    std::fs::create_dir_all(dest).map_err(|e| (1, format!("{to}: {e}")))?;
    let tops = crate::tar::unpack(&mut res.body_mut().as_reader(), dest)
        .map_err(|e| (1, format!("the memory could not be written to {to}: {e}")))?;
    let top = tops
        .first()
        .ok_or_else(|| (EXIT_ODD, "the node sent an empty export".to_string()))?;
    let dir = dest.join(top);
    if t.json {
        crate::sayln!("{}", json!({ "exported_to": dir.display().to_string() }));
        return Ok(());
    }
    let manifest = memcp_core::export::verify_export(&dir).map_err(|e| (1, e.to_string()))?;
    crate::cli::export_report(
        manifest["person"]["name"].as_str().unwrap_or("?"),
        &dir,
        &manifest,
    );
    Ok(())
}

// --- the commands that only exist against a running node ---------------------------------------------------------------

fn direct(t: &Target, a: &Args, words: &str) -> Result<(), Failure> {
    let arg = |i: usize, what: &str| {
        a.pos
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| bad(&format!("missing {what}")))
    };
    let flag = |n: &str| a.flags.get(n).map(String::as_str);
    let show = |v: &Value| crate::sayln!("{v}");
    match words {
        "folder get" => {
            let space = arg(2, "the space")?;
            let (status, text) = call(
                t,
                "GET",
                &format!("/space/folder?space={}", pct(space)),
                None,
            )?;
            folder_answer(t, status, &text)
        }
        "folder set" => {
            let space = arg(2, "the space")?;
            let folder = if a.switches.iter().any(|s| s == "--clear") {
                Value::Null
            } else {
                json!(arg(3, "the folder (or --clear)")?)
            };
            let body = json!({ "space": space, "folder": folder });
            let (status, text) = call(
                t,
                "POST",
                "/space/folder",
                Some(body.to_string().as_bytes()),
            )?;
            folder_answer(t, status, &text)
        }
        "invite code" => {
            let v = call_json(t, "POST", "/pairing", &json!({}))?;
            if t.json {
                show(&v);
            } else {
                crate::sayln!(
                    "Code (once, valid {} minutes): {}",
                    v["expires_in"].as_i64().unwrap_or(600) / 60,
                    v["code"].as_str().unwrap_or("?")
                );
                crate::sayln!("Link for a phone: {}", v["link"].as_str().unwrap_or("?"));
            }
            Ok(())
        }
        "invite ticket" => {
            let v = call_json(t, "POST", "/nodes/invite", &json!({}))?;
            if t.json {
                show(&v);
            } else {
                crate::sayln!(
                    "Link with a one-time ticket (valid {} minutes): {}",
                    v["expires_in"].as_i64().unwrap_or(600) / 60,
                    v["link"].as_str().unwrap_or("?")
                );
                crate::sayln!("On the other node: `memcp --node ... join LINK`.");
            }
            Ok(())
        }
        "invite pending" => {
            let (status, text) = call(t, "GET", "/join/pending", None)?;
            if status != 200 {
                return Err(refusal(status, &text));
            }
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if t.json {
                show(&v);
                return Ok(());
            }
            let list = v["requests"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                crate::sayln!("No device is waiting.");
            }
            for r in list {
                crate::sayln!(
                    "{}\t{}\twords {}\twaiting {} s",
                    r["id"].as_str().unwrap_or("?"),
                    r["device"].as_str().unwrap_or("?"),
                    r["check"].as_str().unwrap_or("?"),
                    r["waited"]
                );
            }
            Ok(())
        }
        "invite approve" | "invite deny" => {
            let id = arg(2, "the request id")?;
            let verb = if words == "invite approve" {
                "approve"
            } else {
                "deny"
            };
            let v = call_json(t, "POST", &format!("/join/{verb}"), &json!({ "id": id }))?;
            if t.json {
                show(&v);
            } else {
                crate::sayln!("{id}: {}.", v["result"].as_str().unwrap_or(verb));
            }
            Ok(())
        }
        "invite allow" => {
            let device = arg(2, "the device name")?;
            let minutes: i64 = flag("--minutes").and_then(|m| m.parse().ok()).unwrap_or(30);
            let v = call_json(
                t,
                "POST",
                "/join/allow-device",
                &json!({ "device": device, "minutes": minutes }),
            )?;
            if t.json {
                show(&v);
            } else {
                crate::sayln!(
                    "{device} may join within {} minutes, once, without the three words.",
                    v["minutes"]
                );
            }
            Ok(())
        }
        "join" => join(t, a),
        _ => Err(bad(&format!("`{words}` has no remote form"))),
    }
}

fn folder_answer(t: &Target, status: u16, text: &str) -> Result<(), Failure> {
    if status == 404 || status == 405 {
        return Err((
            EXIT_ODD,
            "pending: this node has no space folders yet (the routes /admin/api/space/folder come with the branch space-folders)"
                .into(),
        ));
    }
    if status != 200 {
        return Err(refusal(status, text));
    }
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if t.json {
        crate::sayln!("{v}");
    } else {
        match v["folder"].as_str() {
            Some(f) => crate::sayln!("{f}"),
            None => crate::sayln!("(no folder)"),
        }
    }
    Ok(())
}

/// `join ADDRESS [--ticket T]`: asks that node to let this node in, and follows the answer.
fn join(t: &Target, a: &Args) -> Result<(), Failure> {
    let target = a
        .pos
        .get(1)
        .ok_or_else(|| bad("join LINK|ID|ADDRESS [--ticket TICKET]"))?;
    // A ticket given apart goes into a link with the address, as the node reads it.
    let link = match a.flags.get("--ticket") {
        Some(ticket) if target.starts_with("http") => {
            format!("memcp://node?addr={}&t={}", pct(target), pct(ticket))
        }
        Some(_) => {
            return Err(bad(
                "--ticket goes with an address (a link carries its own)",
            ));
        }
        None => target.clone(),
    };
    let started = call_json(t, "POST", "/nodes/add", &json!({ "link": link }))?;
    let id = started["id"]
        .as_str()
        .ok_or_else(|| (EXIT_ODD, "the node gave no request id".to_string()))?
        .to_string();
    let mut told = false;
    for _ in 0..80 {
        std::thread::sleep(Duration::from_secs(2));
        let (status, text) = call(t, "GET", &format!("/join-memory?id={id}"), None)?;
        if status != 200 {
            return Err(refusal(status, &text));
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        match v["state"].as_str().unwrap_or("") {
            "asking" => {}
            "waiting" => {
                if !told && !t.json {
                    crate::sayln!(
                        "Say yes on the other node (`invite approve`), after comparing these words: {}",
                        v["check"].as_str().unwrap_or("?")
                    );
                    told = true;
                }
            }
            "approved" => {
                if t.json {
                    crate::sayln!("{v}");
                } else {
                    crate::sayln!(
                        "Joined: {}.",
                        v["name"].as_str().unwrap_or("the other node")
                    );
                }
                return Ok(());
            }
            other => {
                if t.json {
                    crate::sayln!("{v}");
                }
                return Err((
                    1,
                    format!(
                        "not joined: {other}{}",
                        v["error"]
                            .as_str()
                            .map_or(String::new(), |e| format!(" ({e})"))
                    ),
                ));
            }
        }
    }
    Err((1, "not joined: no answer in time".into()))
}

// --- a connection key: MCP tools ---------------------------------------------------------------------------------------

/// One tool, called with a connection key at `/mcp/AGENT`.
fn tool(t: &Target, name: &str, args: &Value) -> Result<Value, Failure> {
    let agent_name = t.agent.as_deref().unwrap_or("");
    let url = format!("{}/mcp/{agent_name}", t.node);
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args } });
    let mut res = agent()
        .post(&url)
        .header("Authorization", &format!("Bearer {}", t.key))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .send(body.to_string().as_bytes())
        .map_err(|e| unreachable(t, e))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| unreachable(t, e))?;
    if status == 401 || status == 403 {
        return Err((
            EXIT_REFUSED,
            "the node refused the key for this connection (unknown, revoked or expired)".into(),
        ));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        (
            EXIT_ODD,
            format!("the node answered {status}, not an MCP answer"),
        )
    })?;
    if let Some(e) = v.get("error") {
        return Err((
            EXIT_ODD,
            e["message"].as_str().unwrap_or("MCP error").to_string(),
        ));
    }
    let content = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    if v["result"]["isError"].as_bool() == Some(true) {
        let denied = content.contains("denied") || content.contains("not allowed");
        return Err((if denied { EXIT_RIGHTS } else { 1 }, content.to_string()));
    }
    Ok(serde_json::from_str(content).unwrap_or(Value::Null))
}

/// With a connection key (`--agent NAME`): the commands an assistant's tools can do. The rest needs a console key.
fn connection_key(t: &Target, a: &Args, entry: &cli_table::Entry) -> Result<(), Failure> {
    let arg = |i: usize, what: &str| {
        a.pos
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| bad(&format!("missing {what}")))
    };
    let tags: Vec<String> = a
        .flags
        .get("--tag")
        .map(|v| {
            v.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let out = |v: &Value| {
        if t.json {
            crate::sayln!("{v}");
            true
        } else {
            false
        }
    };
    match (a.pos[0].as_str(), a.pos.get(1).map(String::as_str)) {
        ("note", Some("add")) => {
            let v = tool(
                t,
                "remember",
                &json!({ "space": arg(2, "the space")?, "text": arg(3, "the text")?, "tags": tags }),
            )?;
            if !out(&v) {
                match v["result"].as_str() {
                    Some("created") => {
                        crate::sayln!("note {} created.", v["fact"]["id"].as_str().unwrap_or("?"))
                    }
                    Some("duplicate") => crate::sayln!(
                        "already there: {}.",
                        v["fact"]["id"].as_str().unwrap_or("?")
                    ),
                    _ => {
                        for c in v["candidates"].as_array().into_iter().flatten() {
                            crate::sayln!(
                                "close to {}: {}",
                                c["id"].as_str().unwrap_or("?"),
                                c["text"].as_str().unwrap_or("")
                            );
                        }
                        return Err(bad("close notes exist: correct one with `note edit`"));
                    }
                }
            }
            Ok(())
        }
        ("note", Some("recall" | "list")) => {
            let query = if a.pos[1] == "recall" {
                arg(3, "the words to look for")?
            } else {
                ""
            };
            let v = tool(
                t,
                "recall",
                &json!({ "space": arg(2, "the space")?, "query": query, "tags": tags,
                    "limit": a.flags.get("--limit").and_then(|n| n.parse::<i64>().ok()).unwrap_or(if query.is_empty() { 50 } else { 10 }) }),
            )?;
            if !out(&v) {
                for r in v["results"].as_array().into_iter().flatten() {
                    crate::sayln!(
                        "{}\t{}",
                        r["id"].as_str().unwrap_or("?"),
                        r["text"].as_str().unwrap_or("")
                    );
                }
            }
            Ok(())
        }
        ("note", Some("show")) => {
            let v = tool(t, "history", &json!({ "id": arg(3, "the note id")? }))?;
            if !out(&v) {
                for f in v["chain"].as_array().into_iter().flatten() {
                    crate::sayln!(
                        "{}\t{}\t{}\t{}",
                        f["id"].as_str().unwrap_or("?"),
                        f["status"].as_str().unwrap_or("?"),
                        f["created_at"].as_str().unwrap_or("?"),
                        f["text"].as_str().unwrap_or("")
                    );
                }
            }
            Ok(())
        }
        ("note", Some("edit")) => {
            let v = tool(
                t,
                "supersede",
                &json!({ "id": arg(3, "the note id")?, "text": arg(4, "the new text")?, "tags": tags, "reason": a.flags.get("--reason") }),
            )?;
            if !out(&v) {
                crate::sayln!(
                    "note replaced by {}.",
                    v["fact"]["id"].as_str().unwrap_or("?")
                );
            }
            Ok(())
        }
        ("note", Some("retire")) => {
            let v = tool(
                t,
                "retire",
                &json!({ "id": arg(3, "the note id")?, "reason": a.flags.get("--reason").ok_or_else(|| bad("missing --reason"))? }),
            )?;
            if !out(&v) {
                crate::sayln!(
                    "note {} retired (it stays in the history; `note restore` brings it back).",
                    v["fact"]["id"].as_str().unwrap_or("?")
                );
            }
            Ok(())
        }
        ("note", Some("restore")) => {
            let v = tool(
                t,
                "restore",
                &json!({ "id": arg(3, "the note id")?, "reason": a.flags.get("--reason") }),
            )?;
            if !out(&v) {
                crate::sayln!(
                    "note restored as {}.",
                    v["fact"]["id"].as_str().unwrap_or("?")
                );
            }
            Ok(())
        }
        ("note", Some("tags")) => {
            let given = |n: &str| {
                a.flags.get(n).map(|v| {
                    v.split(',')
                        .map(|x| x.trim().to_string())
                        .collect::<Vec<_>>()
                })
            };
            let v = tool(
                t,
                "tags_set",
                &json!({ "id": arg(3, "the note id")?, "tags": given("--tag"), "add": given("--add"), "remove": given("--remove") }),
            )?;
            if !out(&v) {
                let list: Vec<&str> = v["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                crate::sayln!("note {}: tags {}.", arg(3, "")?, list.join(", "));
            }
            Ok(())
        }
        ("space", Some("list")) => {
            let v = tool(t, "spaces", &json!({}))?;
            if !out(&v) {
                fn walk(list: &Value) {
                    for s in list.as_array().into_iter().flatten() {
                        crate::sayln!("{}", s["name"].as_str().unwrap_or("?"));
                        walk(&s["children"]);
                    }
                }
                walk(&v["spaces"]);
            }
            Ok(())
        }
        _ => Err((
            EXIT_RIGHTS,
            format!(
                "`{}` is not something a connection key can do (it opens the assistants' tools: the note commands and `space list`); \
                 use a console key (--key-file) for it",
                entry.words
            ),
        )),
    }
}
