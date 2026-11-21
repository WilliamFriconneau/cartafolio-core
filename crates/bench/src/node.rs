//! A node of the bench: a real `memcp` process on a loopback port, in a directory of the bench, driven through its
//! command line, its page's API (with a console token) and the MCP address of its connections. Every process the bench
//! starts is kept here by its PID, and only those are ever killed.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub type R<T> = Result<T, String>;

pub fn fail<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Where the programs are, and the directory of this run.
#[derive(Clone, Debug)]
pub struct Env {
    pub bin: PathBuf,
    pub work: PathBuf,
}

impl Env {
    pub fn exe(&self, name: &str) -> PathBuf {
        self.bin
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    /// A small program put in front of `memcp-sync` for the nodes: it writes its PID in the bench's directory, then
    /// becomes `memcp-sync`. The bench knows that way every sync loop a node started by itself, and can tell one that
    /// outlived its node.
    pub fn sync_wrapper(&self) -> R<PathBuf> {
        let path = self.work.join("memcp-sync-wrapper.sh");
        if !path.is_file() {
            let pids = self.work.join("sync-pids");
            let body = format!(
                "#!/bin/sh\necho $$ >> '{}'\nexec '{}' \"$@\"\n",
                pids.display(),
                self.exe("memcp-sync").display()
            );
            std::fs::write(&path, body).map_err(fail)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .map_err(fail)?;
            }
        }
        Ok(path)
    }

    /// The PIDs of the sync loops the nodes started (through the wrapper).
    pub fn node_sync_pids(&self) -> Vec<u32> {
        std::fs::read_to_string(self.work.join("sync-pids"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }
}

/// The PIDs of every process the bench started, so that what is left at the end is killed (by PID, never by name).
pub static STARTED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

pub fn started(pid: u32) {
    STARTED.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
}

/// Whether a process is alive and its command line names the bench's directory: only such a process is ever signalled.
pub fn ours_alive(pid: u32, work: &Path) -> bool {
    let Ok(out) = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    let cmd = String::from_utf8_lossy(&out.stdout);
    out.status.success() && cmd.contains(&*work.to_string_lossy())
}

/// Kills (SIGKILL) a process of the bench by its PID, if it is still one of ours.
pub fn kill_pid(pid: u32, work: &Path) {
    if ours_alive(pid, work) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output();
    }
}

/// The parent PID of a process (`None` when it is gone).
pub fn parent_of(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Resident memory (KiB) of a process.
pub fn rss_kib(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Open files of a process: `/proc/PID/fd` where there is one (Linux), else `lsof` (macOS).
pub fn open_files(pid: u32) -> Option<usize> {
    let proc_fd = PathBuf::from(format!("/proc/{pid}/fd"));
    if proc_fd.is_dir() {
        return std::fs::read_dir(proc_fd).ok().map(|d| d.count());
    }
    let out = Command::new("lsof")
        .args(["-n", "-P", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let lines = String::from_utf8_lossy(&out.stdout).lines().count();
    Some(lines.saturating_sub(1))
}

pub fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into()
}

pub fn read_json(res: &mut ureq::http::Response<ureq::Body>) -> (u16, Value) {
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

pub fn token_in(text: &str, prefix: &str) -> R<String> {
    text.split_whitespace()
        .find(|w| w.starts_with(prefix))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .to_string()
        })
        .ok_or_else(|| format!("no {prefix} token in: {text}"))
}

/// Every token the bench was given or made up: none may appear in a node's log.
pub static TOKENS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn keep_token(t: &str) {
    TOKENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(t.to_string());
}

pub fn until(secs: u64, what: &str, mut ready: impl FnMut() -> bool) -> R<()> {
    let end = Instant::now() + Duration::from_secs(secs);
    while !ready() {
        if Instant::now() > end {
            return Err(format!("timed out waiting for {what}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// The address and key of an MCP connection.
#[derive(Clone, Debug)]
pub struct Key {
    pub url: String,
    pub key: String,
}

/// What a node keeps about a peer it joined.
#[derive(Clone, Debug)]
pub struct Peering {
    pub address: String,
    pub key: String,
    pub name: String,
}

/// A space in the person's tree, as the page reads it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub space: String,
    pub label: String,
    pub parent: Option<String>,
    pub kind: String,
}

pub struct Node {
    pub env: Env,
    pub name: String,
    pub data: PathBuf,
    pub port: u16,
    child: Option<Child>,
    console: Option<String>,
}

impl Node {
    /// `memcp init` in a new directory of the bench (`defaults`: the six usual spaces; else none).
    pub fn init(env: &Env, name: &str, defaults: bool) -> R<Node> {
        let n = Node {
            env: env.clone(),
            name: name.into(),
            data: env.work.join(name),
            port: 0,
            child: None,
            console: None,
        };
        let mut args = vec!["init"];
        if defaults {
            args.push("--defaults");
        }
        n.cli(&args)?;
        Ok(n)
    }

    /// A handle on a node's directory with no process: for the passes of `memcp-sync`, which are processes of their own.
    pub fn detached(env: &Env, name: &str, data: &Path) -> Node {
        Node {
            env: env.clone(),
            name: name.into(),
            data: data.to_path_buf(),
            port: 0,
            child: None,
            console: None,
        }
    }

    pub fn log(&self) -> PathBuf {
        self.env.work.join(format!("{}.log", self.name))
    }

    pub fn cli(&self, args: &[&str]) -> R<String> {
        let out = Command::new(self.env.exe("memcp"))
            .arg("--data")
            .arg(&self.data)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(fail)?;
        if !out.status.success() {
            return Err(format!(
                "memcp {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn cli_ok(&self, args: &[&str]) -> bool {
        Command::new(self.env.exe("memcp"))
            .arg("--data")
            .arg(&self.data)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Serves the node on a loopback port (the same one again after a stop) and waits until it answers.
    pub fn serve(&mut self) -> R<()> {
        for attempt in 0..10 {
            if self.port == 0 || attempt > 0 {
                let l = TcpListener::bind("127.0.0.1:0").map_err(fail)?;
                self.port = l.local_addr().map_err(fail)?.port();
            }
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.log())
                .map_err(fail)?;
            let err = log.try_clone().map_err(fail)?;
            let wrapper = self.env.sync_wrapper()?;
            let child = Command::new(self.env.exe("memcp"))
                .arg("--data")
                .arg(&self.data)
                .args([
                    "serve",
                    "--listen",
                    &format!("127.0.0.1:{}", self.port),
                    "--public-url",
                    &self.base(),
                    "--node-name",
                    &self.name,
                    "--threads",
                    "8",
                ])
                .env("MEMCP_SYNC_BIN", &wrapper)
                .env("PENSINE_SYNC_BIN", &wrapper)
                .stdin(Stdio::null())
                .stdout(log)
                .stderr(err)
                .spawn()
                .map_err(fail)?;
            started(child.id());
            self.child = Some(child);
            let end = Instant::now() + Duration::from_secs(30);
            loop {
                if !self.alive() {
                    break;
                }
                if let Ok(mut r) = http()
                    .get(&format!("{}/.well-known/memcp", self.base()))
                    .call()
                    && r.status().as_u16() == 200
                {
                    let _ = read_json(&mut r);
                    return Ok(());
                }
                if Instant::now() > end {
                    return Err(format!("{} did not come up", self.name));
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            if let Some(mut c) = self.child.take() {
                let _ = c.wait();
            }
        }
        Err(format!("{} could not be served", self.name))
    }

    pub fn alive(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    /// SIGKILL, no clean-up: what a power cut leaves.
    pub fn kill9(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// A key of the console (cached): what the page of the person holds.
    pub fn console(&mut self) -> R<String> {
        if self.console.is_none() {
            let out = self.cli(&["console-token", "owner"])?;
            let t = token_in(&out, "hcpc_")?;
            keep_token(&t);
            self.console = Some(t);
        }
        self.console
            .clone()
            .ok_or_else(|| "no console token".into())
    }

    pub fn admin_get(&mut self, path: &str) -> R<(u16, Value)> {
        let token = self.console()?;
        let mut r = http()
            .get(&format!("{}/admin/api{path}", self.base()))
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(fail)?;
        Ok(read_json(&mut r))
    }

    pub fn admin_post(&mut self, path: &str, body: &Value) -> R<(u16, Value)> {
        let token = self.console()?;
        let mut r = http()
            .post(&format!("{}/admin/api{path}", self.base()))
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "memcp")
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(fail)?;
        Ok(read_json(&mut r))
    }

    /// An MCP connection made with the command line (`extra`: `--all`, `--grant ...`, `--read-only`, `--user P`).
    pub fn connect(&self, agent: &str, extra: &[&str]) -> R<Key> {
        let base = self.base();
        let mut args = vec!["connect", agent, "--public-url", &base];
        args.extend_from_slice(extra);
        let out = self.cli(&args)?;
        let key = token_in(&out, "hcp_")?;
        keep_token(&key);
        Ok(Key {
            url: format!("{base}/mcp/{agent}"),
            key,
        })
    }

    /// The person's « À trier » on this node (the page makes it the first time it is opened).
    pub fn triage_box(&mut self) -> R<String> {
        let (status, v) = self.admin_get("/state")?;
        v["triage"]["space"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| format!("no À trier on {} ({status}): {v}", self.name))
    }

    /// The person's tree of spaces, as the page reads it.
    pub fn layout(&mut self) -> R<Vec<Place>> {
        let (status, v) = self.admin_get("/layout")?;
        if status != 200 {
            return Err(format!("layout of {}: {status} {v}", self.name));
        }
        let mut out: Vec<Place> = v["spaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|p| Place {
                space: p["space"].as_str().unwrap_or_default().into(),
                label: p["label"].as_str().unwrap_or_default().into(),
                parent: p["parent"].as_str().map(String::from),
                kind: p["kind"].as_str().unwrap_or_default().into(),
            })
            .collect();
        out.sort();
        Ok(out)
    }

    /// The active notes of `space`, id to text, read from the node's store (read-only): the oracle of the bench.
    pub fn notes_of(&self, space: &str) -> R<BTreeMap<String, String>> {
        let path = self.data.join(format!("{space}.sqlite"));
        if !path.is_file() {
            return Ok(BTreeMap::new());
        }
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(fail)?;
        let _ = conn.busy_timeout(Duration::from_secs(10));
        let mut st = conn
            .prepare("SELECT id, text FROM facts WHERE status = 'active'")
            .map_err(fail)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(fail)?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (id, text) = row.map_err(fail)?;
            out.insert(id, text);
        }
        Ok(out)
    }

    /// `memcp-sync --join`: this node asks `host` to let it in, the person there says yes on the page. The peer kept.
    pub fn join(&self, host: &mut Node, device: &str) -> R<Peering> {
        let before: Vec<PathBuf> = peer_files(&self.data);
        let status_file = self
            .env
            .work
            .join(format!("join-{}-{}.json", self.name, host.name));
        let mut child = Command::new(self.env.exe("memcp-sync"))
            .arg("--data")
            .arg(&self.data)
            .args(["--join", &host.base(), "--status-file"])
            .arg(&status_file)
            .args(["--name", device])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(fail)?;
        started(child.id());
        let mut found = None;
        until(30, "the join request", || {
            let Ok((_, v)) = host.admin_get("/join/pending") else {
                return false;
            };
            found = v["requests"]
                .as_array()
                .and_then(|l| l.iter().find(|r| r["device"] == json!(device)).cloned());
            found.is_some()
        })?;
        let req = found.ok_or("no request")?;
        let (status, v) = host.admin_post("/join/approve", &json!({ "id": req["id"] }))?;
        if status != 200 {
            return Err(format!("approve: {status} {v}"));
        }
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(Some(s)) = child.try_wait() {
                if !s.success() {
                    return Err(format!("the join of {} failed: {s}", self.name));
                }
                break;
            }
            if Instant::now() > end {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the join did not finish".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let new = peer_files(&self.data)
            .into_iter()
            .find(|p| !before.contains(p))
            .ok_or("no peer was kept")?;
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(new).map_err(fail)?).map_err(fail)?;
        let p = Peering {
            address: v["address"].as_str().unwrap_or_default().into(),
            key: v["key"].as_str().unwrap_or_default().into(),
            name: v["name"].as_str().unwrap_or_default().into(),
        };
        keep_token(&p.key);
        Ok(p)
    }

    /// One pass of `memcp-sync` against a peer: the exit code and what it said.
    pub fn sync(&self, peer: &Peering) -> R<(i32, String)> {
        let child = Command::new(self.env.exe("memcp-sync"))
            .arg("--data")
            .arg(&self.data)
            .args([
                "--peer",
                &peer.address,
                "--name",
                &peer.name,
                "--node-name",
                &self.name,
            ])
            .env("MEMCP_PEER_TOKEN", &peer.key)
            .env("PENSINE_PEER_TOKEN", &peer.key)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(fail)?;
        started(child.id());
        let out = child.wait_with_output().map_err(fail)?;
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.code().unwrap_or(-1), said))
    }

    /// A pass started and left running (to be killed in the middle).
    pub fn sync_spawn(&self, peer: &Peering) -> R<Child> {
        let child = Command::new(self.env.exe("memcp-sync"))
            .arg("--data")
            .arg(&self.data)
            .args([
                "--peer",
                &peer.address,
                "--name",
                &peer.name,
                "--node-name",
                &self.name,
            ])
            .env("MEMCP_PEER_TOKEN", &peer.key)
            .env("PENSINE_PEER_TOKEN", &peer.key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(fail)?;
        started(child.id());
        Ok(child)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.kill9();
    }
}

fn peer_files(data: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(data.join("peers"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Made-up words, the same for the same seed: no two texts of the bench look alike to the near-duplicate guard.
pub fn words(seed: u64, n: usize) -> String {
    const SYL: [&str; 24] = [
        "ka", "lo", "mi", "ru", "te", "vo", "an", "is", "ul", "be", "do", "fi", "go", "ha", "je",
        "ny", "pa", "qu", "si", "tu", "we", "xa", "yo", "ze",
    ];
    let mut x = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (0..n)
        .map(|_| {
            (0..3)
                .map(|_| {
                    x = x
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    SYL[(x >> 33) as usize % SYL.len()]
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The answer of an MCP tool call.
#[derive(Debug, Clone)]
pub enum Answer {
    /// The tool's result (its JSON, or the text when it is not JSON).
    Ok(Value),
    /// The tool said no (`isError`), with what it said.
    Refused(String),
    /// Not an answer of the tool: an HTTP status and the body.
    Http(u16, Value),
    /// No answer at all (connection refused, reset, timed out).
    Down(String),
}

impl Answer {
    pub fn is_ok(&self) -> bool {
        matches!(self, Answer::Ok(_))
    }
}

/// One `tools/call` at the connection's address, with its key (or with `key` replaced, for the probes).
pub fn call(agent: &ureq::Agent, url: &str, key: Option<&str>, tool: &str, args: &Value) -> Answer {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": { "name": tool, "arguments": args } });
    let mut rq = agent.post(url).header("Content-Type", "application/json");
    if let Some(k) = key {
        rq = rq.header("Authorization", &format!("Bearer {k}"));
    }
    let mut r = match rq.send(body.to_string().as_bytes()) {
        Ok(r) => r,
        Err(e) => return Answer::Down(e.to_string()),
    };
    let (status, v) = read_json(&mut r);
    if status != 200 {
        return Answer::Http(status, v);
    }
    if v.get("error").is_some() {
        return Answer::Refused(v["error"].to_string());
    }
    let text = v["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if v["result"]["isError"] == json!(true) {
        return Answer::Refused(text);
    }
    Answer::Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

/// Like [`call`], waiting and trying again while the node is busy (503, 429) or down (being restarted), up to `secs`.
pub fn call_retry(agent: &ureq::Agent, k: &Key, tool: &str, args: &Value, secs: u64) -> Answer {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let a = call(agent, &k.url, Some(&k.key), tool, args);
        let again = matches!(a, Answer::Http(503 | 429, _) | Answer::Down(_));
        if !again || Instant::now() > end {
            return a;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A line of the summary.
#[derive(Clone, Debug)]
pub struct Row {
    pub phase: &'static str,
    pub check: String,
    pub ok: Option<bool>,
    pub value: String,
}

pub fn row(phase: &'static str, check: &str, ok: Option<bool>, value: impl Into<String>) -> Row {
    Row {
        phase,
        check: check.into(),
        ok,
        value: value.into(),
    }
}

pub fn print_table(rows: &[Row], out: &mut impl Write) {
    let w_phase = rows.iter().map(|r| r.phase.len()).max().unwrap_or(5).max(5);
    let w_check = rows
        .iter()
        .map(|r| r.check.chars().count())
        .max()
        .unwrap_or(5)
        .max(5);
    let _ = writeln!(
        out,
        "{:<w_phase$}  {:<w_check$}  {:<6}  value",
        "phase", "check", "result"
    );
    let _ = writeln!(out, "{}", "-".repeat(w_phase + w_check + 24));
    for r in rows {
        let res = match r.ok {
            Some(true) => "ok",
            Some(false) => "FAIL",
            None => "-",
        };
        let pad = w_check.saturating_sub(r.check.chars().count());
        let _ = writeln!(
            out,
            "{:<w_phase$}  {}{}  {:<6}  {}",
            r.phase,
            r.check,
            " ".repeat(pad),
            res,
            r.value
        );
    }
}
