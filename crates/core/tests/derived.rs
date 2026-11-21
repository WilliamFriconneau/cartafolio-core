mod common;

use common::{Res, Temp};
use memcp_core::access::{self, Caller, Kind, Rights};
use memcp_core::text::Language;
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space, facts, sync, tools, util};
use serde_json::{Value, json};

fn node(root: &Temp, name: &str, space: &Space) -> Result<Brain, Box<dyn std::error::Error>> {
    let brain = Brain::init(&root.path().join(name), &[], Language::English)?;
    brain.add_space(space)?;
    Ok(brain)
}

fn peer(brain: &Brain, space: &Space, name: &str) -> Result<Caller, Box<dyn std::error::Error>> {
    let control = brain.open_control()?;
    let token = access::connect_for(
        &control,
        OWNER,
        name,
        &Rights::parse(&format!("{space}:rw"))?,
        None,
        false,
        false,
    )?;
    let mut caller =
        access::authenticate(&control, &token, util::now())?.ok_or("missing connection")?;
    caller.kind = Kind::Peer;
    Ok(caller)
}

fn args(space: &Space, value: &str) -> Value {
    json!({"space": space.as_str(), "records": [{
        "text": format!("Daily total: {{\"value\":{value}}}"),
        "tags": ["total/day-1"],
        "legacy_tag": "total/day-1",
        "origin": {"app": "counter", "id": "daily/day-1", "layer": "dossier"},
    }]})
}

#[test]
fn two_isolated_replicas_can_only_submit_to_one_authority() -> Res {
    let root = Temp::new("derived-authority");
    let space = Space::new("totals")?;
    let authority = node(&root, "authority", &space)?;
    let first = node(&root, "first", &space)?;
    let second = node(&root, "second", &space)?;
    std::fs::write(authority.dir().join("derived-authority"), "")?;
    let a = peer(&authority, &space, "first")?;
    let b = peer(&authority, &space, "second")?;
    let call = |brain, who, value| tools::call(brain, who, "derived_deposit", &args(&space, value));
    let initial = call(&authority, &a, "1")?;
    let copy = |source, dest| -> Res {
        let page = sync::events_after(source, &space, 0, 100, 1 << 20, None)?;
        sync::apply(dest, &space, &page.events, "authority")?;
        Ok(())
    };
    for replica in [&first, &second] {
        copy(&authority, replica)?;
        assert_eq!(call(replica, &a, "2")?["result"], "authority_unavailable");
    }

    // Both devices reconnect with the same recalculation. Only one creates a successor.
    let (a_reply, b_reply) = std::thread::scope(|s| {
        let a = s.spawn(|| call(&authority, &a, "2"));
        let b = s.spawn(|| call(&authority, &b, "2.0"));
        (a.join(), b.join())
    });
    let a_reply = a_reply.map_err(|_| "first request panicked")??;
    let b_reply = b_reply.map_err(|_| "second request panicked")??;
    let a_out = &a_reply["results"][0];
    let b_out = &b_reply["results"][0];
    assert_eq!(a_out["id"], b_out["id"]);
    assert_ne!(a_out["result"], b_out["result"]);
    let next = call(&authority, &a, "3")?;
    let latest = next["results"][0]["id"]
        .as_str()
        .ok_or("missing current id")?;
    let initial = initial["results"][0]["id"]
        .as_str()
        .ok_or("missing first id")?;
    for replica in [&first, &second] {
        copy(&authority, replica)?;
        let conn = replica.open_space(&space)?;
        let chain = facts::history(&conn, initial)?;
        assert_eq!(chain.len(), 3);
        assert!(
            chain
                .iter()
                .any(|fact| fact.id == latest && fact.status == "active")
        );
        assert_eq!(sync::dispute_count(&conn)?, 0);
    }
    Ok(())
}

#[test]
fn a_new_device_cannot_create_a_replacement_for_unbound_history() -> Res {
    let root = Temp::new("derived-history");
    let space = Space::new("totals")?;
    let authority = node(&root, "authority", &space)?;
    let legacy = node(&root, "legacy", &space)?;
    let old_caller = peer(&legacy, &space, "writer")?;
    let new_caller = peer(&authority, &space, "new-device")?;
    std::fs::write(authority.dir().join("derived-authority"), "")?;
    let original = tools::call(&legacy, &old_caller, "deposit", &args(&space, "1"))?;
    let id = original["results"][0]["id"]
        .as_str()
        .ok_or("missing original")?;
    let events = sync::events_after(&legacy, &space, 0, 100, 1 << 20, None)?;
    sync::apply(&authority, &space, &events.events, "legacy")?;

    let result = tools::call(
        &authority,
        &new_caller,
        "derived_deposit",
        &args(&space, "2"),
    )?;
    assert_eq!(result["results"][0]["result"], "identity_unverified");
    assert_eq!(facts::history(&authority.open_space(&space)?, id)?.len(), 1);

    // A date without any evidence in the authority’s storage can be created.
    let other_day = |value: &str| {
        let raw = args(&space, value)
            .to_string()
            .replace("day-1", "day-2")
            .replace("Daily total:", "Second daily total:");
        serde_json::from_str(&raw)
    };
    let created = tools::call(&authority, &new_caller, "derived_deposit", &other_day("8")?)?;
    assert_eq!(created["results"][0]["result"], "created");
    let corrected = tools::call(&authority, &new_caller, "derived_deposit", &other_day("9")?)?;
    assert_eq!(corrected["results"][0]["result"], "updated");
    let corrected_id = corrected["results"][0]["id"]
        .as_str()
        .ok_or("missing head")?;
    let head =
        facts::read_fact(&authority.open_space(&space)?, corrected_id)?.ok_or("missing fact")?;
    assert_eq!(
        head.replaces.as_deref(),
        created["results"][0]["id"].as_str()
    );
    Ok(())
}
