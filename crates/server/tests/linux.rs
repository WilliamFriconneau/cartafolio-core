//! The Linux server app through the real program: `memcp setup`, `memcp.toml`, the page (console key and tailnet
//! identity), an assistant's connection, the doctor, and the backup timer's command.

use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
const BIN: &str = env!("CARGO_BIN_EXE_memcp");
static N: AtomicU32 = AtomicU32::new(0);

fn temp(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-linux-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::create_dir_all(&dir);
    dir
}

fn memcp(args: &[&str]) -> Result<String, Box<dyn Error>> {
    let o = Command::new(BIN)
        .args(args)
        .env_remove("MEMCP_CONFIG")
        .env_remove("MEMCP_DATA")
        .stdin(Stdio::null())
        .output()?;
    if !o.status.success() {
        return Err(format!("{args:?}: {}", String::from_utf8_lossy(&o.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn free_port() -> Result<u16, Box<dyn Error>> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(config: &Path) -> Result<Server, Box<dyn Error>> {
    let mut child = Command::new(BIN)
        .args(["--config"])
        .arg(config)
        .arg("serve")
        .stderr(Stdio::piped())
        .spawn()?;
    let mut reader = BufReader::new(child.stderr.take().ok_or("no stderr")?);
    let server = Server(child);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err("the server stopped before it listened".into());
        }
        if line.contains("listening on") {
            break;
        }
    }
    std::thread::spawn(move || for _ in reader.lines() {});
    Ok(server)
}

fn http(port: u16, raw: &str) -> Result<(String, String), Box<dyn Error>> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.write_all(raw.as_bytes())?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    let (h, b) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    Ok((h.to_string(), b.to_string()))
}

fn get(port: u16, path: &str, headers: &str) -> Result<(String, String), Box<dyn Error>> {
    http(
        port,
        &format!("GET {path} HTTP/1.1\r\nHost: x\r\n{headers}Connection: close\r\n\r\n"),
    )
}

fn tool(
    port: u16,
    agent: &str,
    key: &str,
    name: &str,
    args: Value,
) -> Result<Value, Box<dyn Error>> {
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args } })
    .to_string();
    let (head, body) = http(
        port,
        &format!(
            "POST /mcp/{agent} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
    )?;
    if !head.starts_with("HTTP/1.1 200") {
        return Err(format!("{head}\n{body}").into());
    }
    Ok(serde_json::from_str(&body)?)
}

/// The words of `text` that start with `prefix`.
fn word(text: &str, prefix: &str) -> Result<String, Box<dyn Error>> {
    text.split(|c: char| c.is_whitespace() || c == '"')
        .find(|w| w.starts_with(prefix))
        .map(String::from)
        .ok_or_else(|| format!("no {prefix}... in {text}").into())
}

// File modes and the systemd layout are those of a Linux machine.
#[cfg(unix)]
#[test]
fn setup_makes_a_private_memory_a_page_key_and_one_address_for_an_assistant() -> Res {
    let t = temp("setup");
    let data = t.join("data");
    let conf = t.join("memcp.toml");
    let port = free_port()?;
    let listen = format!("127.0.0.1:{port}");
    let out = memcp(&[
        "setup",
        "--yes",
        "--data",
        data.to_str().ok_or("path")?,
        "--config",
        conf.to_str().ok_or("path")?,
        "--listen",
        &listen,
        "--person",
        "Ana",
        "--node-name",
        "atelier",
        "--language",
        "fr",
    ])?;
    // The data folder and its files are private; the six default spaces exist.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(mode(&data)?, 0o700);
        assert_eq!(mode(&data.join("control.sqlite"))?, 0o600);
    }
    for s in ["personal", "health", "home", "finances", "hobbies", "work"] {
        assert!(data.join(format!("{s}.sqlite")).is_file(), "{s}");
    }
    assert!(fs::read_to_string(data.join("config.toml"))?.contains("language = \"fr\""));
    // The page address, a console key, and the address that carries the key.
    assert!(out.contains(&format!("http://{listen}/admin")), "{out}");
    let first_console = word(&out, "hcpc_")?;
    let with_key = word(&out, &format!("http://{listen}/mcp/assistant?key="))?;
    let key = with_key.rsplit('=').next().ok_or("key")?.to_string();
    // The file is the documented one, with what was said written in it.
    let text = fs::read_to_string(&conf)?;
    assert!(text.contains("node_name = \"atelier\""), "{text}");
    assert!(text.contains("# threads = 4"));

    // Setup again changes nothing and does not make the connection twice.
    let again = memcp(&[
        "setup",
        "--yes",
        "--data",
        data.to_str().ok_or("path")?,
        "--config",
        conf.to_str().ok_or("path")?,
    ])?;
    assert!(again.contains("left as it is"), "{again}");
    assert!(again.contains("exists already"), "{again}");
    // A console key is renewed each time it is asked for: the new one is the one that opens the page.
    let console = word(&again, "hcpc_")?;
    assert_ne!(console, first_console);

    // Served from the file alone.
    let _server = start(&conf)?;
    let (head, body) = get(port, "/.well-known/memcp", "")?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(body.contains("atelier"), "{body}");

    // Remember and recall through the address of the connection.
    let r = tool(
        port,
        "assistant",
        &key,
        "remember",
        json!({ "text": "2026-11-17, the boiler is serviced in March.", "space": "home" }),
    )?;
    assert!(r.to_string().contains("created"), "{r}");
    let r = tool(
        port,
        "assistant",
        &key,
        "recall",
        json!({ "query": "boiler", "space": "home" }),
    )?;
    assert!(r.to_string().contains("serviced in March"), "{r}");
    // The address with the key in it works the same, and a wrong key is refused.
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let post = |k: &str| {
        http(
            port,
            &format!(
                "POST /mcp/assistant?key={k} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ping}",
                ping.len()
            ),
        )
    };
    assert!(post(&key)?.0.starts_with("HTTP/1.1 200"));
    assert!(post("wrong")?.0.starts_with("HTTP/1.1 401"));

    // The page: a console key opens it; the assistant's key does not.
    let (head, body) = get(
        port,
        "/admin/api/state",
        &format!("Authorization: Bearer {console}\r\n"),
    )?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let state: Value = serde_json::from_str(&body)?;
    assert_eq!(state["me"]["display"], json!("Ana"));
    let (head, _) = get(
        port,
        "/admin/api/state",
        &format!("Authorization: Bearer {key}\r\n"),
    )?;
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    let (head, _) = get(port, "/admin", "")?;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the page itself is served"
    );

    // The doctor reads it as the service would, and the backup command of the timer works from the file.
    let backups = t.join("backups");
    let mut s = fs::read_to_string(&conf)?;
    s.push_str(&format!(
        "backup_dir = \"{}\"\nbackup_keep = 2\n",
        backups.display()
    ));
    fs::write(&conf, s)?;
    let c = conf.to_str().ok_or("path")?;
    let doctor = memcp(&["--config", c, "doctor"])?;
    assert!(doctor.contains("this node answers on"), "{doctor}");
    assert!(doctor.contains("backup: none"), "{doctor}");
    for _ in 0..3 {
        memcp(&["--config", c, "backup"])?;
        std::thread::sleep(Duration::from_millis(1100));
    }
    let kept = fs::read_dir(&backups)?.count();
    assert_eq!(kept, 2, "backup_keep is honoured");
    let doctor = memcp(&["--config", c, "doctor"])?;
    assert!(doctor.contains("backup: the newest"), "{doctor}");
    assert!(doctor.contains("settings file"), "{doctor}");
    let _ = fs::remove_dir_all(&t);
    Ok(())
}

#[test]
fn the_page_on_the_tailnet_knows_who_is_listed_and_nobody_else() -> Res {
    let t = temp("tailnet");
    let data = t.join("data");
    let conf = t.join("memcp.toml");
    let (port, identity) = (free_port()?, free_port()?);
    memcp(&[
        "setup",
        "--yes",
        "--data",
        data.to_str().ok_or("path")?,
        "--config",
        conf.to_str().ok_or("path")?,
        "--listen",
        &format!("127.0.0.1:{port}"),
    ])?;
    let mut s = fs::read_to_string(&conf)?;
    s.push_str(&format!(
        "identity_listen = \"127.0.0.1:{identity}\"\ntailnet_users = [\"you@example.org=owner\"]\n"
    ));
    fs::write(&conf, s)?;
    let c = conf.to_str().ok_or("path")?;
    assert!(memcp(&["config", "check", "--config", c])?.contains(": ok"));
    let _server = start(&conf)?;
    let login = |who: &str| format!("Tailscale-User-Login: {who}\r\n");
    let (head, body) = get(identity, "/admin/api/state", &login("you@example.org"))?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(serde_json::from_str::<Value>(&body)?["me"]["admin"], true);
    let (head, _) = get(identity, "/admin/api/state", &login("someone@example.org"))?;
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    let (head, _) = get(identity, "/admin/api/state", "")?;
    assert!(
        head.starts_with("HTTP/1.1 401"),
        "no login header, no page: {head}"
    );
    let _ = fs::remove_dir_all(&t);
    Ok(())
}

#[test]
fn a_flag_wins_over_the_file_and_a_bad_file_is_refused() -> Res {
    let t = temp("flags");
    let conf = t.join("memcp.toml");
    fs::write(
        &conf,
        "listen = \"127.0.0.1:1\"\nthreads = 3\nsurprise = true\n",
    )?;
    let c = conf.to_str().ok_or("path")?;
    let e = memcp(&["config", "check", "--config", c])
        .err()
        .ok_or("accepted")?;
    assert!(e.to_string().contains("unknown option \"surprise\""), "{e}");
    fs::write(&conf, "listen = \"0.0.0.0:0\"\n")?;
    // The file asks for a non-local address without consent: refused, as on the command line.
    let o = Command::new(BIN)
        .args(["--config", c, "--data"])
        .arg(t.join("d"))
        .args(["init"])
        .output()?;
    assert!(o.status.success());
    let o = Command::new(BIN)
        .args(["--config", c, "--data"])
        .arg(t.join("d"))
        .args(["serve"])
        .output()?;
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("in clear"));
    let _ = fs::remove_dir_all(&t);
    Ok(())
}

#[test]
fn the_companion_services_get_their_key_and_settings_from_the_file() -> Res {
    let t = temp("prepare");
    let data = t.join("data");
    let conf = t.join("memcp.toml");
    memcp(&[
        "setup",
        "--yes",
        "--data",
        data.to_str().ok_or("path")?,
        "--config",
        conf.to_str().ok_or("path")?,
        "--spaces",
        "personal",
    ])?;
    let c = conf.to_str().ok_or("path")?;
    let out = t.join("janitor");
    let o = Command::new(BIN)
        .args(["--config", c, "prepare", "janitor", "--out"])
        .arg(&out)
        .output()?;
    assert_eq!(o.status.code(), Some(78), "off by default");
    let mut s = fs::read_to_string(&conf)?;
    s.push_str(
        "janitor = true\njanitor_ollama_url = \"http://127.0.0.1:11434\"\njanitor_quiet = 60\n",
    );
    fs::write(&conf, s)?;
    memcp(&[
        "--config",
        c,
        "prepare",
        "janitor",
        "--out",
        out.to_str().ok_or("path")?,
    ])?;
    let conf_text = fs::read_to_string(out.join("janitor.conf"))?;
    assert!(
        conf_text.contains("ollama_url = http://127.0.0.1:11434"),
        "{conf_text}"
    );
    assert!(conf_text.contains("models = local"));
    assert!(fs::read_to_string(out.join("janitor.env"))?.contains("--quiet 60"));
    // Run again: the key is kept, nothing is made twice.
    let key = fs::read_to_string(out.join("key"))?;
    memcp(&[
        "--config",
        c,
        "prepare",
        "janitor",
        "--out",
        out.to_str().ok_or("path")?,
    ])?;
    assert_eq!(key, fs::read_to_string(out.join("key"))?);
    let _ = fs::remove_dir_all(&t);
    Ok(())
}
