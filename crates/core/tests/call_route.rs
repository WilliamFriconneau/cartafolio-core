//! The people of a note lead the router, and correcting them does not teach words.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::tools;
use memcp_core::users::OWNER;
use memcp_core::{Brain, util};
use serde_json::{Value, json};

fn caller(b: &Brain, name: &str, rights: &str) -> Result<Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        name,
        &Rights::parse(rights)?,
        None,
        false,
        false,
    )?;
    Ok(access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?)
}

fn has(v: &Value, tag: &str) -> bool {
    v["tags"]
        .as_array()
        .is_some_and(|a| a.iter().any(|t| t == tag))
}

fn teams() -> Value {
    json!({ "caller": ["Alice Martin"], "callee": ["Bob", "Carol Durand"], "app": "teams" })
}

#[test]
fn the_people_of_a_note_are_a_strong_hint_for_where_it_goes() -> Res {
    let t = Temp::new("call-route");
    let b = brain(&t)?;
    let (_, box_name) = memcp_core::pairing::create_for_peer(&b, OWNER, "owner", util::now())?;
    let box_ = sp(&box_name);
    let device = caller(&b, "peer-phone-x", &format!("{box_name}:rw"))?;
    let c = caller(&b, "person-page", "all")?;
    let drop = |text: &str, call: Value| -> Result<String, Box<dyn std::error::Error>> {
        let r = tools::call(
            &b,
            &device,
            "remember",
            &json!({ "text": text, "tags": ["inbox"], "call": call }),
        )?;
        Ok(r["fact"]["id"].as_str().ok_or("id")?.to_string())
    };
    let with_bob = json!({ "caller": ["Bob"], "callee": ["Alice"], "app": "phone" });
    // Nothing learned yet.
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "text": "Anything at all", "tags": ["personnes/bob"] }),
    )?;
    assert_eq!(p["route"], json!([]), "{p}");
    // The janitor sends: not learned.
    memcp_core::triage::send(
        &b,
        &box_,
        &drop("First words", with_bob.clone())?,
        &sp("home"),
        "memcp-janitor",
        "owner",
    )?;
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "text": "Other words", "tags": ["personnes/bob"] }),
    )?;
    assert_eq!(p["route"], json!([]), "{p}");
    // The person sends one note about Bob to work: one is enough, whatever the words.
    memcp_core::triage::send(
        &b,
        &box_,
        &drop("Budget call", with_bob.clone())?,
        &sp("work"),
        "owner",
        "owner",
    )?;
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "text": "Completely different words", "tags": ["personnes/bob", "appel"] }),
    )?;
    assert_eq!(p["route"][0]["space"], "work", "{p}");
    assert_eq!(p["route"][0]["by"], "people", "{p}");
    // Another person is not Bob.
    let p = tools::call(
        &b,
        &c,
        "tags_suggest",
        &json!({ "text": "Completely different words", "tags": ["personnes/carol"] }),
    )?;
    assert_eq!(p["route"], json!([]), "{p}");
    // A reader of another space is not told about it.
    let only_home = caller(&b, "only-home", "home:r")?;
    let p = tools::call(
        &b,
        &only_home,
        "tags_suggest",
        &json!({ "text": "x y z", "tags": ["personnes/bob"] }),
    )?;
    assert_eq!(p["route"], json!([]), "{p}");
    Ok(())
}

#[test]
fn correcting_the_people_does_not_teach_the_words() -> Res {
    let t = Temp::new("call-learn");
    let b = brain(&t)?;
    let c = caller(&b, "person-page", "all")?;
    for text in [
        "Budget review quarterly forecast spending",
        "Quarterly forecast budget spending review again",
    ] {
        let f = tools::call(
            &b,
            &c,
            "remember",
            &json!({ "space": "work", "text": text, "tags": ["inbox"], "call": teams() }),
        )?["fact"]
            .clone();
        tools::call(
            &b,
            &c,
            "tags_set",
            &json!({ "id": f["id"], "add": ["personnes/zed"] }),
        )?;
    }
    let g = tools::call(
        &b,
        &c,
        "remember",
        &json!({ "space": "work", "text": "Budget review quarterly forecast spending third" }),
    )?;
    assert!(!has(&g["fact"], "personnes/zed"), "{g}");
    Ok(())
}

#[test]
fn a_field_the_caller_gave_is_not_given_a_second_value() -> Res {
    let t = Temp::new("call-field-once");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let r = tools::call(
        &b,
        &c,
        "remember",
        &json!({ "space": "health", "text": "Tension 12/8 this morning.", "tags": ["field/tension/13/8"] }),
    )?;
    let f = &r["fact"];
    assert!(
        has(f, "field/tension/13/8") && !has(f, "field/tension/12/8"),
        "{f}"
    );
    Ok(())
}
