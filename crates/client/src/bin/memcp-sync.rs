//! `memcp-sync --data DIR --peer ADDRESS [--name NAME] [--token-env VAR] [--every SECONDS] [--node-name NAME] [--window-days N|off] [--person ID]`: keeps this node's memory in
//! step with a peer. ADDRESS is the address of this node's connection on the peer (`https://…/mcp/<person>/<connection>`),
//! its key read from VAR (default MEMCP_PEER_TOKEN). `--person` names the person of this node whose spaces go both ways
//! (default: the person whose memory this node holds, the one who owns its spaces; the peer connection says who that is
//! on the peer). One pass, or one every SECONDS. Each thing a pass could not do (an offer refused, a space not made) is
//! one line on standard error, and is kept for the page (`last_pass` of the peer in `GET /sync/status`).
//! Exit: 0 done, 1 the key is refused, 2 wrong use, 3 done with problems.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use memcp_client::peer_sync::Peer;
use memcp_core::Brain;

const USAGE: &str = "usage: memcp-sync --data DIR --peer ADDRESS [--name NAME] [--token-env VAR] [--every SECONDS] [--node-name NAME] [--window-days N|off] [--person ID]\n       memcp-sync --data DIR --join ADDRESS [--join ADDRESS…] [--expect-id ID] --status-file FILE [--name NAME]";

fn command_name() -> &'static str {
    memcp_core::invoked_as("memcp-sync", "cartafolio-sync")
}

fn usage() -> String {
    USAGE.replace("memcp-sync ", &format!("{} ", command_name()))
}

/// A name that is safe in a file name and a connection: lower case letters, digits and dashes.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 30 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// `--join ADDRESS [--join ADDRESS…] [--expect-id ID]`: asks the node at ADDRESS (the first that answers) to let this
/// machine in, writes the three words to the status file for the person's page, waits for the answer, and on a yes
/// keeps the peer (its address and key, private) in `DIR/peers/` so that the node syncs with it from then on. The page
/// reads the status file; the key never goes into it. A ticket from a node of the same person comes in the variable
/// `MEMCP_JOIN_TICKET` (never on the command line): the node lets this machine in at once, with no words.
/// With `--expect-id`, the node that answers must say that id: another one is refused.
fn join(
    data: &str,
    addresses: &[String],
    expect: Option<&str>,
    status_file: &str,
    name: &str,
) -> ExitCode {
    use memcp_client::peer_sync::{join_poll, join_request_with, who_is_there};
    let write = |v: serde_json::Value| {
        let tmp = format!("{status_file}.tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, status_file);
        }
    };
    let fail = |e: String| {
        write(serde_json::json!({ "state": "failed", "error": e }));
        ExitCode::from(1)
    };
    let ticket = memcp_core::env::var("MEMCP_JOIN_TICKET")
        .ok()
        .filter(|t| !t.is_empty());
    let own_id = Brain::open(&PathBuf::from(data))
        .ok()
        .and_then(|b| memcp_core::directory::node_id(&b).ok());
    let expect = expect.and_then(memcp_core::directory::valid_id);
    // The first address that answers and is the node meant.
    let mut last = "no address".to_string();
    let mut found = None;
    for a in addresses {
        if !memcp_client::allowed_url(a) {
            last = "this address is refused: HTTPS, or in clear only on this machine or the own network".into();
            continue;
        }
        match who_is_there(a) {
            Ok(w) => {
                let said = w["id"].as_str().and_then(memcp_core::directory::valid_id);
                if expect.is_some() && said != expect {
                    last = "the node at that address is not the one the link names".into();
                    continue;
                }
                found = Some((a.clone(), w));
                break;
            }
            Err(e) => last = e,
        }
    }
    let Some((address, who)) = found else {
        return fail(last);
    };
    let address = address.as_str();
    let ask = match join_request_with(address, name, ticket.as_deref(), own_id.as_deref()) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let node = serde_json::json!({ "id": who["id"], "name": who["name"] });
    write(serde_json::json!({ "state": "waiting", "check": ask.check, "node": node }));
    for _ in 0..70 {
        std::thread::sleep(Duration::from_secs(2));
        match join_poll(address, &ask) {
            Ok(v) if v["state"] == "waiting" => {}
            Ok(v) if v["state"] == "approved" => {
                let (Some(key), Some(url)) =
                    (v["join"]["key"].as_str(), v["join"]["mcp_url"].as_str())
                else {
                    return fail("unreadable answer".into());
                };
                // The node's own name keeps two nodes on one host apart; the host is the fallback of an older node.
                let named = who["name"].as_str().map(slug).filter(|n| !n.is_empty());
                let Some(peer) = keep_peer(
                    &PathBuf::from(data).join("peers"),
                    address,
                    url,
                    key,
                    named.as_deref(),
                ) else {
                    return fail("the peer could not be kept".into());
                };
                write(serde_json::json!({ "state": "approved", "name": peer, "node": node }));
                return ExitCode::SUCCESS;
            }
            Ok(v) => {
                write(serde_json::json!({ "state": v["state"] }));
                return ExitCode::from(1);
            }
            Err(e) => return fail(e),
        }
    }
    write(serde_json::json!({ "state": "expired" }));
    ExitCode::from(1)
}

/// Whether the node that started this loop is gone: this process has another parent now (the system took it over).
/// Not on Windows, where a process is not told that its parent ended: there a loop still outlives its node.
fn orphaned(parent: Option<u32>) -> bool {
    #[cfg(unix)]
    {
        parent.is_some_and(|p| std::os::unix::process::parent_id() != p)
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        false
    }
}

/// `scheme://host:port` of an address, in lower case: what tells two nodes apart.
fn origin(address: &str) -> String {
    let (scheme, rest) = address.split_once("://").unwrap_or(("", address));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    )
}

/// Keeps a peer this node joined in `dir` (its connection's address and key, private); returns the name it goes by.
/// The name comes from the first label of the host (`peer-nas`); a node at another address whose name would be the
/// same (two nodes on one machine, `nas.home` and `nas.office`) gets `peer-nas-2`, and so on, so that no peer ever
/// replaces another. Joining the same node again replaces its record, under its name.
fn keep_peer(
    dir: &std::path::Path,
    address: &str,
    url: &str,
    key: &str,
    named: Option<&str>,
) -> Option<String> {
    let host = address
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', ':']).next())
        .unwrap_or("peer");
    // The node's own name when it told one; the first label of the host for an older node.
    let base = format!(
        "peer-{}",
        named.unwrap_or_else(|| host.split('.').next().unwrap_or("peer"))
    );
    let _ = std::fs::create_dir_all(dir);
    let joined = origin(address);
    let peer = (1..=1000)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|name| {
            let Ok(text) = std::fs::read_to_string(dir.join(format!("{name}.json"))) else {
                return true;
            };
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            // A record of an earlier version has no `joined`: the address of its connection says where it is.
            let there = v["joined"].as_str().or(v["address"].as_str()).map(origin);
            there.is_none_or(|o| o == joined)
        })?;
    let record = serde_json::json!({ "name": peer, "address": url, "key": key, "joined": joined });
    save_private(&dir.join(format!("{peer}.json")), &record.to_string()).then_some(peer)
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
    if let (Some(data), Some(status)) = (value("--data"), value("--status-file"))
        && args.iter().any(|a| a == "--join")
    {
        let addresses: Vec<String> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--join")
            .filter_map(|(i, _)| args.get(i + 1).cloned())
            .collect();
        let name = value("--name").unwrap_or_else(|| "a machine".into());
        let expect = value("--expect-id");
        return join(&data, &addresses, expect.as_deref(), &status, &name);
    }
    let (Some(data), Some(peer)) = (value("--data"), value("--peer")) else {
        eprintln!("{}", usage());
        return ExitCode::from(2);
    };
    if !memcp_client::allowed_url(&peer) {
        eprintln!(
            "{}: this address is refused: HTTPS, or in clear only on this machine or the own network",
            command_name()
        );
        return ExitCode::from(2);
    }
    let var = value("--token-env").unwrap_or_else(|| "MEMCP_PEER_TOKEN".into());
    let Ok(key) = memcp_core::env::var(&var) else {
        eprintln!("{}: no key in {var}", command_name());
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
            eprintln!("{}: {e}", command_name());
            return ExitCode::from(2);
        }
    };
    // What a crash cut off is finished or undone before events are exchanged (see core recover.rs); the lock of the
    // bytes keeps this pass and a server's on the same folder from sweeping under each other.
    match brain.recover() {
        Ok(r) if r.is_clean() && r.swept => {}
        Ok(r) => eprintln!("{}: {}", command_name(), r.summary()),
        Err(e) => eprintln!("{}: recovery: not finished yet: {e}", command_name()),
    }
    // `--node-name NAME`: the name this node goes by in the peer directory (else one it made for itself, kept).
    if let Some(n) = value("--node-name")
        && let Err(e) = memcp_core::directory::set_own_name(&brain, &n)
    {
        eprintln!("{}: {e}", command_name());
        return ExitCode::from(2);
    }
    // `--window-days N`: this replica holds the last N days only (kept in its settings); `off` holds everything.
    if let Some(w) = value("--window-days") {
        let days = match w.as_str() {
            "off" | "0" => None,
            n => match n.parse::<u32>() {
                Ok(d) => Some(d),
                Err(_) => {
                    eprintln!("{}", usage());
                    return ExitCode::from(2);
                }
            },
        };
        if let Err(e) = memcp_core::sync::set_window_days(&brain, days) {
            eprintln!("{}: {e}", command_name());
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
                eprintln!("{}: --person {person}: {e}", command_name());
                return ExitCode::from(2);
            }
        }
    }
    // `MEMCP_SYNC_PARENT`: the node that started this loop (it sets it). When that node is gone (killed, crashed),
    // the loop ends: a node started again starts its own, and two loops with one peer must never pile up.
    let parent = memcp_core::env::var("MEMCP_SYNC_PARENT")
        .ok()
        .and_then(|p| p.trim().parse::<u32>().ok());
    loop {
        if orphaned(parent) {
            return ExitCode::SUCCESS;
        }
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
                    eprintln!("{}: {p}", command_name());
                }
                if pass.problems.is_empty() { 0 } else { 3 }
            }
            Err(e) => {
                eprintln!("{}: {e}", command_name());
                if e == "refused" || e.contains("does not reach") {
                    1
                } else {
                    3
                }
            }
        };
        match every {
            Some(s) if code != 1 => {
                for _ in 0..s.max(5) {
                    if orphaned(parent) {
                        eprintln!(
                            "{}: the node that started this loop is gone; it ends with it",
                            command_name()
                        );
                        return ExitCode::SUCCESS;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            _ => return ExitCode::from(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::keep_peer;

    fn kept(dir: &std::path::Path) -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                    .filter_map(|t| serde_json::from_str(&t).ok())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|v| v["address"].to_string());
        out
    }

    #[test]
    fn a_second_node_on_the_same_host_is_kept_beside_the_first_and_a_rejoin_replaces() {
        let dir = std::env::temp_dir().join(format!("memcp-sync-peers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Two nodes of the person on one machine (two ports), and one whose name starts the same way elsewhere.
        let a = keep_peer(
            &dir,
            "http://127.0.0.1:8791",
            "http://127.0.0.1:8791/mcp/peer-a1",
            "hcp_a_1",
            None,
        );
        let b = keep_peer(
            &dir,
            "http://127.0.0.1:8792",
            "http://127.0.0.1:8792/mcp/peer-b1",
            "hcp_b_1",
            None,
        );
        let c = keep_peer(
            &dir,
            "https://nas.home.example",
            "https://nas.home.example/mcp/peer-c1",
            "hcp_c_1",
            None,
        );
        let d = keep_peer(
            &dir,
            "https://nas.office.example",
            "https://nas.office.example/mcp/peer-d1",
            "hcp_d_1",
            None,
        );
        let names = [a.clone(), b.clone(), c, d];
        assert!(names.iter().all(Option::is_some), "{names:?}");
        let mut unique = names.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            4,
            "each peer has a name of its own: {names:?}"
        );
        assert_eq!(kept(&dir).len(), 4, "no peer replaced another");
        // Joining the first node again (a new key) replaces its record and keeps its name.
        let again = keep_peer(
            &dir,
            "http://127.0.0.1:8791",
            "http://127.0.0.1:8791/mcp/peer-a2",
            "hcp_a_2",
            None,
        );
        assert_eq!(again, a);
        let all = kept(&dir);
        assert_eq!(all.len(), 4);
        assert!(all.iter().any(|v| v["key"] == "hcp_a_2"));
        assert!(!all.iter().any(|v| v["key"] == "hcp_a_1"));
        assert!(all.iter().any(|v| v["key"] == "hcp_b_1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
