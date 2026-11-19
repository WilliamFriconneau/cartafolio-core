//! Living boards: cards in columns, the last pass, the delta, two close cards kept apart.

mod common;

use common::{Res, Temp, brain};
use memcp_core::access::{self, Rights};
use memcp_core::tools;
use memcp_core::triage;
use memcp_core::users::OWNER;
use serde_json::json;

#[test]
fn a_board_follows_its_delta_and_keeps_every_card_apart() -> Res {
    let t = Temp::new("living");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:rw", space.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "assistant", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let s = space.as_str();
    for text in [
        "Call the supplier about the quote",
        "Idea: a shared calendar for the team",
        "The printer jams every Monday",
    ] {
        tools::call(&b, &c, "remember", &json!({ "space": s, "text": text }))?;
    }
    let first = tools::call(&b, &c, "living_get", &json!({ "space": s, "page": "next" }))?;
    assert_eq!(first["behind"], 3, "{first}");
    assert!(first["last_pass"]["at"].is_null());
    let delta = first["delta"].as_array().ok_or("delta")?.clone();
    let last = delta[2]["at"].as_i64().ok_or("at")?;
    let source = delta[0]["id"].as_str().ok_or("id")?;
    // Two close cards stay two cards.
    let made = tools::call(
        &b,
        &c,
        "living_set",
        &json!({ "space": s, "page": "next", "covers": last,
                 "add": [ { "text": format!("Call the supplier about the quote ({source})"), "column": "now" },
                          { "text": format!("Call the supplier about the delivery ({source})"), "column": "next" } ] }),
    )?;
    let ids: Vec<String> = made["added"]
        .as_array()
        .ok_or("added")?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(ids.len(), 2, "{made}");
    let read = tools::call(&b, &c, "living_get", &json!({ "space": s, "page": "next" }))?;
    assert_eq!(read["behind"], 0, "{read}");
    assert_eq!(read["last_pass"]["by"], "assistant");
    assert_eq!(read["board"]["now"].as_array().map(Vec::len), Some(1));
    assert_eq!(read["board"]["next"].as_array().map(Vec::len), Some(1));
    // Done, and dropped: the board follows, the cards keep their history.
    tools::call(
        &b,
        &c,
        "living_set",
        &json!({ "space": s, "page": "next", "move": [ { "id": ids[0], "column": "done" } ], "drop": [ ids[1].clone() ] }),
    )?;
    let after = tools::call(&b, &c, "living_get", &json!({ "space": s, "page": "next" }))?;
    assert_eq!(
        after["board"]["done"].as_array().map(Vec::len),
        Some(1),
        "{after}"
    );
    assert_eq!(after["board"]["now"].as_array().map(Vec::len), Some(0));
    assert_eq!(after["board"]["next"].as_array().map(Vec::len), Some(0));
    // A column that does not exist is refused; covers never goes back.
    assert!(tools::call(&b, &c, "living_set", &json!({ "space": s, "page": "next", "add": [ { "text": "x (f_x)", "column": "someday" } ] })).is_err());
    let back = tools::call(
        &b,
        &c,
        "living_set",
        &json!({ "space": s, "page": "next", "covers": 1 }),
    )?;
    // The move above read up to the moment it wrote, so covers may be a second past `last`; it is never behind.
    assert!(
        back["covers"].as_i64().ok_or("covers")? >= last,
        "covers went back: {back}"
    );
    // Cards are no notes of anyone's delta; each board has its own pass.
    let ideas = tools::call(
        &b,
        &c,
        "living_get",
        &json!({ "space": s, "page": "ideas" }),
    )?;
    assert_eq!(ideas["behind"], 3, "{ideas}");
    Ok(())
}

#[test]
fn a_named_list_holds_what_to_buy_from_notes_of_any_space() -> Res {
    let t = Temp::new("living-list");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:rw", space.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "assistant", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let s = space.as_str();
    tools::call(
        &b,
        &c,
        "living_set",
        &json!({ "space": s, "page": "list-shopping",
        "add": [ "Batteries for the smoke detector (f_a1)", { "text": "Bread (f_b2)", "column": "open" } ] }),
    )?;
    let list = tools::call(
        &b,
        &c,
        "living_get",
        &json!({ "space": s, "page": "list-shopping" }),
    )?;
    assert_eq!(list["columns"], json!(["open", "done"]));
    assert_eq!(
        list["board"]["open"].as_array().map(Vec::len),
        Some(2),
        "{list}"
    );
    assert!(
        tools::call(
            &b,
            &c,
            "living_get",
            &json!({ "space": s, "page": "list-Bad Name" })
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn health_targets_only_return_referential_signs() -> Res {
    let t = Temp::new("living-health");
    let b = brain(&t)?;
    let space = memcp_core::Space::new("health")?;
    b.open_space(&space)?;
    b.set_space_kind(&space, "health")?;
    let ctl = b.open_control()?;
    let rights = Rights::parse("health:rw")?;
    let tok = access::connect_for(&ctl, OWNER, "assistant", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    tools::call(
        &b,
        &c,
        "living_set",
        &json!({
            "space": "health", "page": "targets",
            "add": [{"text": "Suivi", "signs": ["Anxiété", "mal de tête", "après une longue journée"]}]
        }),
    )?;
    let read = tools::call(
        &b,
        &c,
        "living_get",
        &json!({"space": "health", "page": "targets"}),
    )?;
    assert_eq!(
        read["board"]["open"][0]["signs"],
        json!(["anxiety", "headache"])
    );
    Ok(())
}
