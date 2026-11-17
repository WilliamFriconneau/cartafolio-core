#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The peer directory: entries, merge rules, revocation, versions, exposures, connections, the `peers` tool.

mod common;

use common::{Res, Temp};
use memcp_core::access::{self, Rights};
use memcp_core::directory::{self, Local};
use memcp_core::text::Language;
use memcp_core::users::OWNER;
use memcp_core::{Brain, tools, util};
use serde_json::{Value, json};

const NOW: i64 = 1_794_800_000;
const DAY: i64 = 86_400;

fn node(t: &Temp, name: &str) -> Result<Brain, Box<dyn std::error::Error>> {
    let b = Brain::init(
        &t.path().join(name),
        &[common::sp("notes")],
        Language::English,
    )?;
    directory::set_own_name(&b, name)?;
    Ok(b)
}

fn told(name: &str, last_seen: i64, version: &str) -> Value {
    json!({ "name": name, "last_seen": last_seen, "version": version })
}

fn rows(b: &Brain, now: i64) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    Ok(directory::union(b, now, vec![])?)
}

fn row<'a>(rows: &'a [Value], name: &str) -> Option<&'a Value> {
    rows.iter().find(|r| r["name"] == name)
}

#[test]
fn a_node_writes_its_own_entry_with_its_name_and_the_program_version() -> Res {
    let t = Temp::new("dir-own");
    let a = node(&t, "alpha")?;
    directory::touch_self(&a, NOW)?;
    let all = directory::entries(&a)?;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "alpha");
    assert_eq!(all[0].version, env!("CARGO_PKG_VERSION"));
    assert_eq!((all[0].last_seen, all[0].learned_from.clone()), (NOW, None));
    // Not listed to itself.
    assert!(rows(&a, NOW)?.is_empty());
    // A node with no name given makes one and keeps it.
    let b = Brain::init(&t.path().join("anon"), &[], Language::English)?;
    let made = directory::own_name(&b)?;
    assert!(made.starts_with("node-"), "{made}");
    assert_eq!(directory::own_name(&b)?, made);
    assert!(directory::set_own_name(&b, "  ").is_err());
    Ok(())
}

#[test]
fn the_newest_last_seen_wins_per_name_and_nothing_is_invented() -> Res {
    let t = Temp::new("dir-merge");
    let a = node(&t, "alpha")?;
    directory::merge(
        &a,
        "bravo",
        &[told("charlie", NOW - 1000, "20261101.0.1")],
        NOW,
    )?;
    // An older sighting changes nothing; a newer one wins and says who told it.
    directory::merge(
        &a,
        "delta",
        &[told("charlie", NOW - 2000, "20261001.0.1")],
        NOW,
    )?;
    let r = rows(&a, NOW)?;
    assert_eq!(row(&r, "charlie").unwrap()["last_seen"], NOW - 1000);
    assert_eq!(row(&r, "charlie").unwrap()["learned_from"], "bravo");
    directory::merge(
        &a,
        "delta",
        &[told("charlie", NOW - 10, "20261110.0.2")],
        NOW,
    )?;
    let r = rows(&a, NOW)?;
    let c = row(&r, "charlie").unwrap();
    assert_eq!(c["last_seen"], NOW - 10);
    assert_eq!(c["learned_from"], "delta");
    assert_eq!(c["version"], "20261110.0.2");
    // First sight is the earliest told.
    assert_eq!(c["first_seen"], NOW - 2000);
    // Activity in the future is held back to the receiver's clock; this node's own entry is never taken.
    directory::touch_self(&a, NOW)?;
    directory::merge(
        &a,
        "bravo",
        &[
            told("echo", NOW + 10 * DAY, ""),
            told("alpha", NOW + 5, "9.9.9"),
        ],
        NOW,
    )?;
    let r = rows(&a, NOW)?;
    assert!(row(&r, "echo").unwrap()["last_seen"].as_i64().unwrap() <= NOW + 300);
    assert!(row(&r, "alpha").is_none());
    assert_eq!(
        directory::entries(&a)?
            .iter()
            .find(|e| e.name == "alpha")
            .unwrap()
            .version,
        env!("CARGO_PKG_VERSION")
    );
    // Malformed entries are skipped.
    directory::merge(&a, "bravo", &[json!({"x": 1}), json!({"name": ""})], NOW)?;
    Ok(())
}

#[test]
fn an_old_entry_is_shown_as_gone_after_ninety_days_and_kept() -> Res {
    let t = Temp::new("dir-gone");
    let a = node(&t, "alpha")?;
    directory::merge(
        &a,
        "bravo",
        &[
            told("old", NOW - 91 * DAY, ""),
            told("recent", NOW - 89 * DAY, ""),
        ],
        NOW,
    )?;
    let r = rows(&a, NOW)?;
    assert_eq!(row(&r, "old").unwrap()["gone"], true);
    assert_eq!(row(&r, "recent").unwrap()["gone"], false);
    // A year later it is still there.
    let later = rows(&a, NOW + 365 * DAY)?;
    assert!(row(&later, "old").is_some());
    Ok(())
}

#[test]
fn a_revoked_peer_is_not_listed_and_the_flag_spreads_and_sticks() -> Res {
    let t = Temp::new("dir-revoke");
    let a = node(&t, "alpha")?;
    let b = node(&t, "bravo")?;
    directory::merge(&a, "x", &[told("charlie", NOW, "20261101.0.1")], NOW)?;
    directory::merge(&b, "x", &[told("charlie", NOW, "20261101.0.1")], NOW)?;
    assert!(directory::revoke_node(&a, "charlie", NOW + 1)?);
    assert!(row(&rows(&a, NOW + 1)?, "charlie").is_none());
    // The flag goes to the other node with the rest, even though its own sighting of charlie is newer.
    directory::merge(&b, "alpha", &directory::to_wire(&a)?, NOW + 2)?;
    directory::merge(
        &b,
        "x",
        &[told("charlie", NOW + 50, "20261101.0.1")],
        NOW + 60,
    )?;
    assert!(row(&rows(&b, NOW + 60)?, "charlie").is_none());
    // The entry is kept, with the flag, so it keeps spreading.
    assert!(
        directory::entries(&b)?
            .iter()
            .any(|e| e.name == "charlie" && e.revoked)
    );
    // A node is never revoked by itself, and an unknown name leaves a flag behind for others.
    assert!(!directory::revoke_node(&a, "alpha", NOW)?);
    assert!(directory::revoke_node(&a, "ghost", NOW)?);
    assert!(
        directory::to_wire(&a)?
            .iter()
            .any(|e| e["name"] == "ghost" && e["revoked"] == true)
    );
    Ok(())
}

#[test]
fn revoking_the_connection_of_a_peer_revokes_its_entry() -> Res {
    let t = Temp::new("dir-conn");
    let a = node(&t, "alpha")?;
    let ctl = a.open_control()?;
    access::connect_for(
        &ctl,
        OWNER,
        "peer-charlie-ab12",
        &Rights::parse("all")?,
        None,
        false,
        false,
    )?;
    directory::merge(&a, "bravo", &[told("charlie", NOW, "")], NOW)?;
    directory::set_link(&a, "charlie", "peer-charlie-ab12")?;
    assert!(row(&rows(&a, NOW)?, "charlie").is_some());
    access::revoke(&ctl, "peer-charlie-ab12")?;
    assert!(row(&rows(&a, NOW)?, "charlie").is_none());
    assert!(
        directory::entries(&a)?
            .iter()
            .any(|e| e.name == "charlie" && e.revoked)
    );
    Ok(())
}

#[test]
fn a_version_older_than_this_nodes_is_behind() -> Res {
    assert!(directory::behind_of("20261101.0.9", "20261117.0.7"));
    assert!(directory::behind_of("20261117.0.6", "20261117.0.7"));
    assert!(!directory::behind_of("20261117.0.7", "20261117.0.7"));
    assert!(!directory::behind_of("20261117.0.10", "20261117.0.9"));
    assert!(!directory::behind_of("20261201.0.1", "20261117.0.7"));
    assert!(!directory::behind_of("garbage", "20261117.0.7"));
    assert!(!directory::behind_of("", "20261117.0.7"));
    let t = Temp::new("dir-behind");
    let a = node(&t, "alpha")?;
    directory::merge(
        &a,
        "bravo",
        &[
            told("old", NOW, "20200101.0.1"),
            told("same", NOW, env!("CARGO_PKG_VERSION")),
            told("new", NOW, "99999999.0.1"),
        ],
        NOW,
    )?;
    let r = rows(&a, NOW)?;
    assert_eq!(row(&r, "old").unwrap()["behind"], true);
    assert_eq!(row(&r, "same").unwrap()["behind"], false);
    assert_eq!(row(&r, "new").unwrap()["behind"], false);
    assert_eq!(
        row(&r, "same").unwrap()["version"],
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}

#[test]
fn exposures_come_from_the_settings_and_travel_with_the_entry() {
    let kinds = |v: Vec<Value>| -> Vec<String> {
        v.iter()
            .map(|e| e["kind"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        kinds(directory::exposures_from(
            "127.0.0.1:8791",
            "http://127.0.0.1:8791",
            None
        )),
        ["loopback"]
    );
    assert_eq!(
        kinds(directory::exposures_from(
            "0.0.0.0:8791",
            "http://10.0.0.14:8791",
            None
        )),
        ["lan"]
    );
    assert_eq!(
        kinds(directory::exposures_from(
            "0.0.0.0:8791",
            "https://memory.example.org",
            None
        )),
        ["lan", "public"]
    );
    assert_eq!(
        kinds(directory::exposures_from(
            "127.0.0.1:8791",
            "https://box.tail1234.ts.net",
            None
        )),
        ["loopback", "tailnet"]
    );
    assert_eq!(
        kinds(directory::exposures_from(
            "127.0.0.1:8791",
            "http://127.0.0.1:8791",
            Some("https://login.example")
        )),
        ["loopback", "tailnet"]
    );
    let public = directory::exposures_from("127.0.0.1:8791", "https://memory.example.org/", None);
    assert_eq!(
        public[1],
        json!({ "kind": "public", "address": "https://memory.example.org/" })
    );
}

#[test]
fn a_local_only_peer_says_it_syncs_when_opened_and_exposures_gossip() -> Res {
    let t = Temp::new("dir-exposures");
    let a = node(&t, "alpha")?;
    let b = node(&t, "bravo")?;
    // Bravo is reachable on the loopback and the local network only; alpha knows nothing of it yet.
    directory::set_exposures(
        &b,
        &[
            json!({"kind": "loopback", "address": "127.0.0.1:8791"}),
            json!({"kind": "lan", "address": "10.0.0.14:8791"}),
            json!({"kind": "weird"}),
        ],
    )?;
    directory::touch_self(&b, NOW)?;
    directory::merge(&a, "bravo", &directory::to_wire(&b)?, NOW)?;
    let r = rows(&a, NOW)?;
    let bravo = row(&r, "bravo").unwrap();
    assert_eq!(
        bravo["exposures"].as_array().unwrap().len(),
        2,
        "unknown kinds are dropped"
    );
    assert_eq!(bravo["exposures"][1]["address"], "10.0.0.14:8791");
    assert_eq!(bravo["only_local"], true);
    assert_eq!(bravo["note"], "syncs_on_open");
    // A node that changes how it is reached tells it with its next entry.
    directory::set_exposures(
        &b,
        &[
            json!({"kind": "public", "address": "https://memory.example.org"}),
            json!({"kind": "lan"}),
        ],
    )?;
    directory::touch_self(&b, NOW + 60)?;
    directory::merge(&a, "bravo", &directory::to_wire(&b)?, NOW + 60)?;
    let r = rows(&a, NOW + 60)?;
    let bravo = row(&r, "bravo").unwrap();
    assert_eq!(
        (bravo["only_local"].clone(), bravo["note"].clone()),
        (json!(false), Value::Null)
    );
    // A node's own description carries the same fields; an unknown exposure is not "only local".
    let me = directory::node_json(&b, NOW)?;
    assert_eq!(me["exposures"][0]["kind"], "public");
    assert_eq!(me["only_local"], false);
    directory::merge(&a, "bravo", &[told("mystery", NOW, "")], NOW)?;
    assert_eq!(
        row(&rows(&a, NOW)?, "mystery").unwrap()["only_local"],
        false
    );
    Ok(())
}

#[test]
fn an_assistant_connection_is_cloud_when_it_comes_from_the_internet_and_local_otherwise() -> Res {
    let t = Temp::new("dir-assist");
    let a = node(&t, "alpha")?;
    let ctl = a.open_control()?;
    for name in ["claude-web", "claude-code", "unused", "peer-bravo-0001"] {
        access::connect_for(
            &ctl,
            OWNER,
            name,
            &Rights::parse("all")?,
            None,
            false,
            false,
        )?;
    }
    for (name, ip) in [
        ("claude-web", "34.120.5.9"),
        ("claude-code", "127.0.0.1"),
        ("peer-bravo-0001", "8.8.8.8"),
    ] {
        ctl.execute(
            "UPDATE connections SET last_ip = ?2, last_used_at = ?3 WHERE agent = ?1",
            rusqlite::params![name, ip, NOW],
        )?;
    }
    let list = directory::connections(&a, OWNER)?;
    let names: Vec<&str> = list.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["claude-code", "claude-web", "unused"],
        "peers are not assistants"
    );
    let kind = |n: &str| list.iter().find(|c| c["name"] == n).unwrap()["kind"].clone();
    assert_eq!(kind("claude-web"), "cloud");
    assert_eq!(kind("claude-code"), "local");
    assert_eq!(kind("unused"), "local");
    assert!(list.iter().all(|c| c["via"] == "alpha"));
    assert_eq!(list[1]["last_used_at"], util::iso(NOW));
    assert_eq!(list[2]["last_used_at"], Value::Null);
    // Tailnet and private addresses are the person's own machines.
    ctl.execute(
        "UPDATE connections SET last_ip = '100.101.102.103:5555' WHERE agent = 'claude-web'",
        [],
    )?;
    assert_eq!(directory::connections(&a, OWNER)?[1]["kind"], "local");
    ctl.execute(
        "UPDATE connections SET last_ip = '[2606:4700::1]:443' WHERE agent = 'claude-web'",
        [],
    )?;
    assert_eq!(directory::connections(&a, OWNER)?[1]["kind"], "cloud");
    // Through Tailscale Funnel the address seen is the proxy's, a private one: the marker says the internet.
    ctl.execute(
        "UPDATE connections SET last_ip = 'funnel 172.30.32.1' WHERE agent = 'claude-web'",
        [],
    )?;
    assert_eq!(directory::connections(&a, OWNER)?[1]["kind"], "cloud");
    // The node's own tools (janitor, voice, sorter) are not assistants.
    access::set_internal(&ctl, "unused", true)?;
    let names: Vec<String> = directory::connections(&a, OWNER)?
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["claude-code", "claude-web"]);
    Ok(())
}

#[test]
fn the_peers_tool_lists_the_union_with_the_direct_peers_first() -> Res {
    let t = Temp::new("dir-tool");
    let a = node(&t, "alpha")?;
    let ctl = a.open_control()?;
    access::connect_for(
        &ctl,
        OWNER,
        "peer-bravo-0001",
        &Rights::parse("all")?,
        None,
        false,
        false,
    )?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        "claude-web",
        &Rights::parse("notes:r")?,
        None,
        false,
        false,
    )?;
    ctl.execute(
        "UPDATE connections SET last_ip = '34.1.2.3' WHERE agent = 'claude-web'",
        [],
    )?;
    let me = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
    directory::merge(
        &a,
        "bravo",
        &[
            told("bravo", NOW, "20261101.0.1"),
            told("charlie", NOW - 5, "20261101.0.1"),
        ],
        NOW,
    )?;
    directory::set_link(&a, "bravo", "peer-bravo-0001")?;
    let got = tools::call(&a, &me, "peers", &json!({}))?;
    assert_eq!(got["node"]["name"], "alpha");
    let peers = got["peers"].as_array().unwrap();
    assert_eq!(peers[0]["name"], "bravo");
    assert_eq!(peers[0]["connection"], "peer-bravo-0001");
    assert_eq!(peers[0]["direct"], true);
    assert_eq!(peers[0]["allowed"], true);
    assert_eq!(peers[1]["name"], "charlie");
    assert_eq!(peers[1]["direct"], false);
    assert_eq!(peers[1]["learned_from"], "bravo");
    for key in [
        "version",
        "last_seen",
        "first_seen",
        "behind",
        "gone",
        "exposures",
        "only_local",
        "note",
    ] {
        assert!(peers[1].get(key).is_some(), "{key}");
    }
    assert_eq!(got["connections"][0]["name"], "claude-web");
    assert_eq!(got["connections"][0]["kind"], "cloud");
    assert_eq!(got["connections"][0]["via"], "alpha");
    assert!(got["window"].is_object());
    // Direct peers carry their progress.
    let local = Local {
        connection: "peer-z-0001".into(),
        name: "z".into(),
        direction: "idle".into(),
        ..Default::default()
    };
    let u = directory::union(&a, NOW, vec![local])?;
    assert_eq!(u[0]["percent"], 100);
    assert_eq!(u[0]["direction"], "idle");
    Ok(())
}

#[test]
fn a_node_renamed_never_comes_back_as_another_machine_under_its_old_name() -> Res {
    let t = Temp::new("dir-rename");
    let a = node(&t, "alpha")?;
    let old = directory::own_name(&a)?;
    directory::set_own_name(&a, "Home Assistant")?;
    // A peer still knows this node under its old name and tells it.
    let told = vec![
        json!({ "name": old, "last_seen": NOW, "version": "20261117.0.8", "first_seen": NOW - 10 }),
    ];
    directory::merge(&a, "bravo", &told, NOW)?;
    let rows = directory::union(&a, NOW, vec![])?;
    assert!(rows.iter().all(|r| r["name"] != old.as_str()), "{rows:?}");
    Ok(())
}

#[test]
fn a_node_has_a_stable_id_that_its_entry_and_the_wire_carry() -> Res {
    let t = Temp::new("dir-id");
    let a = node(&t, "alpha")?;
    let id = directory::node_id(&a)?;
    assert_eq!(id.len(), 14, "{id}");
    assert_eq!(directory::valid_id(&id).as_deref(), Some(id.as_str()));
    assert_eq!(directory::node_id(&a)?, id, "made once, kept");
    directory::touch_self(&a, NOW)?;
    assert_eq!(directory::entries(&a)?[0].node_id, id);
    assert_eq!(directory::node_json(&a, NOW)?["id"], json!(id));
    assert_eq!(directory::to_wire(&a)?[0]["id"], json!(id));
    // Typed any way, an id reads the same; anything else is not one.
    let loose = id.replace('-', " ").to_lowercase();
    assert_eq!(directory::valid_id(&loose).as_deref(), Some(id.as_str()));
    assert!(directory::valid_id("ABCD-EFGH").is_none());
    assert!(
        directory::valid_id("ABCD-EFGH-IJK1").is_none(),
        "1 is not in the alphabet"
    );
    // Two nodes do not share an id.
    let b = node(&t, "beta")?;
    assert_ne!(directory::node_id(&b)?, id);
    Ok(())
}

#[test]
fn a_third_node_learns_the_id_and_the_addresses_and_never_takes_its_own_id_for_another_node() -> Res
{
    let t = Temp::new("dir-id-merge");
    let a = node(&t, "alpha")?;
    let c = node(&t, "gamma")?;
    directory::set_exposures(
        &a,
        &directory::exposures_from(
            "127.0.0.1:8791",
            "https://a.example.net",
            Some("http://100.64.0.1:8791"),
        ),
    )?;
    directory::touch_self(&a, NOW)?;
    let wire = directory::to_wire(&a)?;
    directory::merge(&c, "beta", &wire, NOW)?;
    let seen = directory::find_by_id(&c, &directory::node_id(&a)?)?.unwrap();
    assert_eq!(seen.name, "alpha");
    let list = directory::union(&c, NOW, vec![])?;
    let row = row(&list, "alpha").unwrap();
    assert_eq!(row["id"], json!(directory::node_id(&a)?));
    assert_eq!(
        row["addresses"],
        json!([
            "https://a.example.net",
            "http://100.64.0.1:8791",
            "http://127.0.0.1:8791"
        ])
    );
    // An entry that carries this node's own id (it went by another name) is not listed as another machine.
    let mine = json!({ "name": "gamma-old", "last_seen": NOW, "version": "1", "id": directory::node_id(&c)? });
    assert_eq!(
        directory::merge(&a, "x", std::slice::from_ref(&mine), NOW)?,
        1,
        "unknown to A: taken"
    );
    assert_eq!(
        directory::merge(&c, "x", &[mine], NOW)?,
        0,
        "C's own id: skipped"
    );
    // The choice of an address is among the addresses the node told.
    directory::set_via(&c, "alpha", Some("https://a.example.net"))?;
    assert_eq!(row_via(&c)?, Some("https://a.example.net".to_string()));
    assert!(directory::set_via(&c, "alpha", Some("https://elsewhere.example.net")).is_err());
    assert!(directory::set_via(&c, "nobody", Some("https://a.example.net")).is_err());
    let entry = directory::find_by_id(&c, &directory::node_id(&a)?)?.unwrap();
    assert_eq!(directory::ways_in(&c, &entry)?[0], "https://a.example.net");
    directory::set_via(&c, "alpha", None)?;
    assert_eq!(row_via(&c)?, None);
    Ok(())
}

fn row_via(b: &Brain) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(row(&rows(b, NOW)?, "alpha").and_then(|r| r["via"].as_str().map(String::from)))
}

#[test]
fn a_node_link_reads_back_and_refuses_what_is_not_one() -> Res {
    let l = directory::NodeLink {
        id: Some("ABCD-EFGH-IJKL".into()),
        name: Some("my node & co".into()),
        addresses: vec![
            "https://a.example.net".into(),
            "http://192.168.1.5:8791".into(),
        ],
        ticket: Some("t0k3n".into()),
    };
    let text = l.to_link();
    assert!(
        text.starts_with("memcp://node?id=ABCD-EFGH-IJKL&name=my%20node%20%26%20co&addr=https%3A%2F%2Fa.example.net"),
        "{text}"
    );
    assert_eq!(directory::NodeLink::parse(&text)?, l);
    assert_eq!(directory::NodeLink::parse(&format!("  {text}\n"))?, l);
    // An id, an address, and the older join link are accepted as what they name.
    assert_eq!(
        directory::NodeLink::parse("abcd efgh ijkl")?.id.as_deref(),
        Some("ABCD-EFGH-IJKL")
    );
    assert_eq!(
        directory::NodeLink::parse("https://a.example.net/")?.addresses,
        ["https://a.example.net"]
    );
    assert_eq!(
        directory::NodeLink::parse(
            "memcp://join?server=https%3A%2F%2Fa.example.net&code=ABCD-EFGH"
        )?
        .addresses,
        ["https://a.example.net"]
    );
    for bad in [
        "",
        "hello",
        "ftp://a",
        "memcp://node?id=nope",
        "memcp://node?name=x",
        "memcp://node?addr=javascript%3Aalert(1)",
        "https://a b",
    ] {
        assert!(directory::NodeLink::parse(bad).is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn requests_told_by_a_peer_are_shown_answered_once_and_forgotten_when_gone() -> Res {
    let t = Temp::new("dir-remote-joins");
    let b = node(&t, "beta")?;
    let ask = |id: &str, device: &str| json!({ "id": id, "device": device, "check": "amber-river-moss", "expires_in": 100 });
    directory::store_remote_joins(&b, "alpha", &[ask("r1", "phone"), ask("r2", "tablet")], NOW)?;
    let shown = directory::remote_joins(&b, NOW)?;
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[0]["node"], json!("alpha"));
    assert_eq!(shown[0]["check"], json!("amber-river-moss"));
    // Past its time, a request is not shown.
    assert!(directory::remote_joins(&b, NOW + 101)?.is_empty());
    // The person answers one: it leaves the list, the answer waits for alpha, and a second answer finds nothing.
    assert_eq!(
        directory::answer_remote_join(&b, "r1", "approved", NOW)?.as_deref(),
        Some("alpha")
    );
    assert_eq!(
        directory::answer_remote_join(&b, "r1", "denied", NOW)?,
        None
    );
    assert_eq!(directory::remote_joins(&b, NOW)?.len(), 1);
    assert_eq!(
        directory::answers_for(&b, "alpha")?,
        vec![json!({ "id": "r1", "result": "approved" })]
    );
    assert!(directory::answers_for(&b, "gamma")?.is_empty());
    // The next list of alpha no longer has r2 (answered or expired there): gone here; the answer for r1 stays until told.
    directory::store_remote_joins(&b, "alpha", &[], NOW + 5)?;
    assert!(directory::remote_joins(&b, NOW + 5)?.is_empty());
    assert_eq!(directory::answers_for(&b, "alpha")?.len(), 1);
    directory::answers_delivered(&b, "alpha")?;
    assert!(directory::answers_for(&b, "alpha")?.is_empty());
    Ok(())
}
