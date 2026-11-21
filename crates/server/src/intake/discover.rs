//! Finding nodes without typing an address.
//!
//! `discover` looks for nodes that advertise on the local network (mDNS, `_memcp._tcp`) and, when the Tailscale
//! command is there, for nodes among the machines of the tailnet (a node answers `GET /.well-known/memcp`). Finding a
//! node gives no access: joining one is asked of it, a person on that node answers after comparing a check phrase, and
//! only then does the key come, once.

use std::collections::BTreeSet;
use std::process::Command;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde_json::Value;

/// The service type nodes advertise.
const SERVICE: &str = "_memcp._tcp.local.";
/// The port nodes listen on by default, probed on the machines of a tailnet.
const DEFAULT_PORT: u16 = 8791;

/// A node that was found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    pub name: String,
    /// `http://host:port`, ready for `join`.
    pub address: String,
    pub version: String,
    /// `lan` (advertised on this network) or `tailscale`.
    pub via: String,
}

/// Looks for nodes for `wait`, on the local network and on the tailnet.
///
/// `MEMCP_DISCOVER_FOUND` is a seam for tests of the page: lines `name<TAB>address<TAB>version<TAB>via` that stand for
/// what the network would have shown, so that no network is needed.
pub fn discover(wait: Duration) -> Vec<Found> {
    if let Ok(fixed) = memcp_core::env::var("MEMCP_DISCOVER_FOUND") {
        return fixed
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f.len() == 4).then(|| Found {
                    name: f[0].into(),
                    address: f[1].into(),
                    version: f[2].into(),
                    via: f[3].into(),
                })
            })
            .collect();
    }
    let mut found: BTreeSet<Found> = BTreeSet::new();
    let tail = std::thread::spawn(|| {
        let mut nodes = tailnet_nodes();
        // The names a Home Assistant installation and a node answer to on a local network, where a container
        // cannot advertise itself.
        for host in ["homeassistant.local", "memcp.local"] {
            if let Some(mut f) = probe(host) {
                f.via = "lan".into();
                nodes.push(f);
            }
        }
        nodes
    });
    for f in lan_nodes(wait) {
        found.insert(f);
    }
    if let Ok(more) = tail.join() {
        for f in more {
            // The same node found twice is listed once, by its address on the local network first.
            if !found
                .iter()
                .any(|g| g.name == f.name && g.version == f.version)
            {
                found.insert(f);
            }
        }
    }
    found.into_iter().collect()
}

fn lan_nodes(wait: Duration) -> Vec<Found> {
    let Ok(daemon) = ServiceDaemon::new() else {
        return vec![];
    };
    let Ok(rx) = daemon.browse(SERVICE) else {
        let _ = daemon.shutdown();
        return vec![];
    };
    let end = Instant::now() + wait;
    let mut cands: Vec<(u8, Found)> = vec![];
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let name = info
                    .get_fullname()
                    .split("._memcp")
                    .next()
                    .unwrap_or("MemCP")
                    .to_string();
                let version = info
                    .get_property_val_str("v")
                    .unwrap_or_default()
                    .to_string();
                // A node lists every address it has: the one a person's devices share comes first, the ones of
                // virtual networks (containers) and link-local ones last.
                for a in info.get_addresses() {
                    let (score, text) = match a.to_ip_addr() {
                        std::net::IpAddr::V4(v4) => {
                            let o = v4.octets();
                            let score = match (o[0], o[1]) {
                                (192, 168) | (10, _) => 0,
                                (100, 64..=127) => 1,
                                (172, 16..=31) => 4,
                                _ => 2,
                            };
                            (score, v4.to_string())
                        }
                        std::net::IpAddr::V6(v6) => (9, format!("[{v6}]")),
                    };
                    cands.push((
                        score,
                        Found {
                            name: name.clone(),
                            address: format!("http://{text}:{}", info.get_port()),
                            version: version.clone(),
                            via: "lan".into(),
                        },
                    ));
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    // One entry per node, by its best address.
    cands.sort_by_key(|c| c.0);
    let mut out: Vec<Found> = vec![];
    for (_, f) in cands {
        if !out
            .iter()
            .any(|g| g.name == f.name && g.version == f.version)
        {
            out.push(f);
        }
    }
    out
}

/// Machines of the tailnet that answer as a node. Nothing when the Tailscale command is not there.
fn tailnet_nodes() -> Vec<Found> {
    let Ok(out) = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
    else {
        return vec![];
    };
    let Ok(status) = serde_json::from_slice::<Value>(&out.stdout) else {
        return vec![];
    };
    let names: Vec<String> = status["Peer"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, p)| p["Online"].as_bool().unwrap_or(false))
        .filter_map(|(_, p)| p["DNSName"].as_str())
        .map(|n| n.trim_end_matches('.').to_string())
        .collect();
    std::thread::scope(|s| {
        let probes: Vec<_> = names.iter().map(|n| s.spawn(move || probe(n))).collect();
        probes
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    })
}

fn probe(host: &str) -> Option<Found> {
    let base = format!("http://{host}:{DEFAULT_PORT}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into();
    let mut r = agent.get(format!("{base}/.well-known/memcp")).call().ok()?;
    let v: Value = serde_json::from_str(&r.body_mut().read_to_string().ok()?).ok()?;
    (v["service"] == "memcp").then(|| Found {
        name: v["name"].as_str().unwrap_or(host).to_string(),
        address: base,
        version: v["version"].as_str().unwrap_or("").to_string(),
        via: "tailscale".into(),
    })
}
