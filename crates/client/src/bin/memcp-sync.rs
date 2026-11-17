//! `memcp-sync --data DIR --peer ADDRESS [--name NAME] [--token-env VAR] [--every SECONDS] [--node-name NAME] [--window-days N|off] [--person ID]`: keeps this node's memory in
//! step with a peer. ADDRESS is the address of this node's connection on the peer (`https://…/mcp/<person>/<connection>`),
//! its key read from VAR (default MEMCP_PEER_TOKEN). `--person` names the person of this node whose spaces go both ways
//! (default: the first administrator). One pass, or one every SECONDS. Exit: 0 done, 1 the key is refused,
//! 2 wrong use, 3 done with problems (said on standard error).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use memcp_client::peer_sync::Peer;
use memcp_core::Brain;

const USAGE: &str = "usage: memcp-sync --data DIR --peer ADDRESS [--name NAME] [--token-env VAR] [--every SECONDS] [--node-name NAME] [--window-days N|off] [--person ID]\n       memcp-sync --data DIR --join ADDRESS --status-file FILE [--name NAME]";

/// `--join ADDRESS`: asks the node at ADDRESS to let this machine in, writes the three words to the status file for the
/// person's page, waits for the answer, and on a yes keeps the peer (its address and key, private) in `DIR/peers/` so that
/// the node syncs with it from then on. The page reads the status file; the key never goes into it.
fn join(data: &str, address: &str, status_file: &str, name: &str) -> ExitCode {
    use memcp_client::peer_sync::{join_poll, join_request};
    let write = |v: serde_json::Value| {
        let tmp = format!("{status_file}.tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, status_file);
        }
    };
    let ask = match join_request(address, name) {
        Ok(a) => a,
        Err(e) => {
            write(serde_json::json!({ "state": "failed", "error": e }));
            return ExitCode::from(1);
        }
    };
    write(serde_json::json!({ "state": "waiting", "check": ask.check }));
    for _ in 0..70 {
        std::thread::sleep(Duration::from_secs(2));
        match join_poll(address, &ask) {
            Ok(v) if v["state"] == "waiting" => {}
            Ok(v) if v["state"] == "approved" => {
                let (Some(key), Some(url)) =
                    (v["join"]["key"].as_str(), v["join"]["mcp_url"].as_str())
                else {
                    write(serde_json::json!({ "state": "failed", "error": "unreadable answer" }));
                    return ExitCode::from(1);
                };
                let host = address
                    .split("://")
                    .nth(1)
                    .and_then(|r| r.split(['/', ':']).next())
                    .unwrap_or("peer");
                let peer = format!("peer-{}", host.split('.').next().unwrap_or("peer"));
                let dir = PathBuf::from(data).join("peers");
                let _ = std::fs::create_dir_all(&dir);
                let record = serde_json::json!({ "name": peer, "address": url, "key": key });
                if !save_private(&dir.join(format!("{peer}.json")), &record.to_string()) {
                    write(
                        serde_json::json!({ "state": "failed", "error": "the peer could not be kept" }),
                    );
                    return ExitCode::from(1);
                }
                write(serde_json::json!({ "state": "approved", "name": peer }));
                return ExitCode::SUCCESS;
            }
            Ok(v) => {
                write(serde_json::json!({ "state": v["state"] }));
                return ExitCode::from(1);
            }
            Err(e) => {
                write(serde_json::json!({ "state": "failed", "error": e }));
                return ExitCode::from(1);
            }
        }
    }
    write(serde_json::json!({ "state": "expired" }));
    ExitCode::from(1)
}

fn save_private(path: &std::path::Path, text: &str) -> bool {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    o.mode(0o600);
    o.open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .is_ok()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let (Some(data), Some(address), Some(status)) =
        (value("--data"), value("--join"), value("--status-file"))
    {
        if !memcp_client::allowed_url(&address) {
            let refused = serde_json::json!({ "state": "failed", "error": "this address is refused: HTTPS, or in clear only on this machine or the own network" });
            let _ = std::fs::write(&status, refused.to_string());
            return ExitCode::from(2);
        }
        let name = value("--name").unwrap_or_else(|| "a machine".into());
        return join(&data, &address, &status, &name);
    }
    let (Some(data), Some(peer)) = (value("--data"), value("--peer")) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if !memcp_client::allowed_url(&peer) {
        eprintln!(
            "memcp-sync: this address is refused: HTTPS, or in clear only on this machine or the own network"
        );
        return ExitCode::from(2);
    }
    let var = value("--token-env").unwrap_or_else(|| "MEMCP_PEER_TOKEN".into());
    let Ok(key) = std::env::var(&var) else {
        eprintln!("memcp-sync: no key in {var}");
        return ExitCode::from(2);
    };
    let name = value("--name").unwrap_or_else(|| {
        let host = peer
            .split("://")
            .nth(1)
            .and_then(|r| r.split(['/', ':']).next())
            .unwrap_or("peer");
        format!("peer-{}", host.split('.').next().unwrap_or("peer"))
    });
    let every = value("--every").and_then(|v| v.parse::<u64>().ok());
    let brain = match Brain::open(&PathBuf::from(&data)) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("memcp-sync: {e}");
            return ExitCode::from(2);
        }
    };
    // `--node-name NAME`: the name this node goes by in the peer directory (else one it made for itself, kept).
    if let Some(n) = value("--node-name")
        && let Err(e) = memcp_core::directory::set_own_name(&brain, &n)
    {
        eprintln!("memcp-sync: {e}");
        return ExitCode::from(2);
    }
    // `--window-days N`: this replica holds the last N days only (kept in its settings); `off` holds everything.
    if let Some(w) = value("--window-days") {
        let days = match w.as_str() {
            "off" | "0" => None,
            n => match n.parse::<u32>() {
                Ok(d) => Some(d),
                Err(_) => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
        };
        if let Err(e) = memcp_core::sync::set_window_days(&brain, days) {
            eprintln!("memcp-sync: {e}");
            return ExitCode::from(2);
        }
    }
    let mut peer = Peer::new(&peer, &key, &name);
    if let Some(person) = value("--person") {
        let found = brain
            .open_control()
            .map_err(|e| e.to_string())
            .and_then(|c| memcp_core::users::resolve(&c, &person).map_err(|e| e.to_string()));
        match found {
            Ok(u) => peer = peer.with_person(&u.id),
            Err(e) => {
                eprintln!("memcp-sync: --person {person}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let code = match peer.sync_once(&brain, now) {
            Ok(pass) => {
                println!(
                    "{} spaces, {} received, {} sent, {} in dispute, {} pruned, {} offered, {} archived",
                    pass.spaces,
                    pass.pulled,
                    pass.pushed,
                    pass.disputed,
                    pass.pruned,
                    pass.offered,
                    pass.archived
                );
                for p in &pass.problems {
                    eprintln!("memcp-sync: {p}");
                }
                if pass.problems.is_empty() { 0 } else { 3 }
            }
            Err(e) => {
                eprintln!("memcp-sync: {e}");
                if e == "refused" || e.contains("does not reach") {
                    1
                } else {
                    3
                }
            }
        };
        match every {
            Some(s) if code != 1 => std::thread::sleep(Duration::from_secs(s.max(5))),
            _ => return ExitCode::from(code),
        }
    }
}
