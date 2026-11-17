//! `memcp-sync --data DIR --peer ADDRESS [--name NAME] [--token-env VAR] [--every SECONDS] [--window-days N|off]`: keeps this node's memory in
//! step with a peer. ADDRESS is the address of this node's connection on the peer (`https://…/mcp/<person>/<connection>`),
//! its key read from VAR (default MEMCP_PEER_TOKEN). One pass, or one every SECONDS. Exit: 0 done, 1 the key is refused,
//! 2 wrong use, 3 done with problems (said on standard error).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use memcp_client::peer_sync::Peer;
use memcp_core::Brain;

const USAGE: &str = "usage: memcp-sync --data DIR --peer ADDRESS [--name NAME] [--token-env VAR] [--every SECONDS] [--window-days N|off]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
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
    let peer = Peer::new(&peer, &key, &name);
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let code = match peer.sync_once(&brain, now) {
            Ok(pass) => {
                println!(
                    "{} spaces, {} received, {} sent, {} in dispute, {} pruned",
                    pass.spaces, pass.pulled, pass.pushed, pass.disputed, pass.pruned
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
