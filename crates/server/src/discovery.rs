//! Finding a node without typing its address.
//!
//! A node that listens beyond the loopback **advertises itself** on the local network (mDNS, the service `_memcp._tcp`),
//! with its name and its version, and answers `GET /.well-known/memcp` with the same, so that what is found can be
//! checked. A device that finds it shows it to the person, who asks to join (join.rs) and is answered by a person.
//! Advertising gives no access: it says that a node is here, nothing of what it holds, and joining always needs a yes.

use std::collections::HashMap;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use serde_json::{Value, json};

/// The service type nodes advertise.
pub const SERVICE: &str = "_memcp._tcp.local.";

/// What `GET /.well-known/memcp` says.
pub fn well_known(name: &str) -> Value {
    json!({
        "service": "memcp",
        "product": "cartafolio",
        "version": env!("CARGO_PKG_VERSION"),
        "name": name,
        "join": "/join",
        "view_contract": memcp_core::view::CONTRACT,
    })
}

/// [`well_known`] with the node's id (`ABCD-EFGH-IJKL`): a person who scanned a link checks that the node that answers
/// is the one the link names. The id names a node; it opens nothing.
pub fn well_known_with_id(name: &str, id: &str) -> Value {
    let mut v = well_known(name);
    v["id"] = json!(id);
    v
}

/// A label that is safe in a DNS name: letters, digits and dashes.
fn host_label(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_lowercase();
    if s.is_empty() { "memcp".into() } else { s }
}

/// Starts advertising this node. The returned daemon advertises for as long as it lives. `None` when the system
/// cannot (no network interface, no multicast): the node then works as before, found by its address.
pub fn advertise(port: u16, name: &str) -> Option<ServiceDaemon> {
    let daemon = ServiceDaemon::new().ok()?;
    let mut txt: HashMap<String, String> = HashMap::new();
    txt.insert("v".into(), env!("CARGO_PKG_VERSION").into());
    txt.insert("join".into(), "/join".into());
    txt.insert("wk".into(), "/.well-known/memcp".into());
    let host = format!("{}.local.", host_label(name));
    let info = ServiceInfo::new(SERVICE, name, &host, "", port, txt)
        .ok()?
        .enable_addr_auto();
    daemon.register(info).ok()?;
    Some(daemon)
}
