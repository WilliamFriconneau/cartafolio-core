//! A call recording carries its participants: the `call` object of `remember`, `deposit` and `file_put`, the tags and
//! fields it becomes, the correction of the participants and the list of names.

mod common;

use base64::Engine as _;
use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::call::{self, Call};
use memcp_core::tools::{self, ToolError};
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

fn tags(v: &Value) -> Vec<String> {
    v["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn has(v: &Value, tag: &str) -> bool {
    tags(v).iter().any(|t| t == tag)
}

fn iphone() -> Value {
    json!({
        "caller": ["Alice Martin"],
        "callee": ["+33 6 12 34 56 78"],
        "app": "phone",
        "direction": "in",
        "started_at": 1_763_380_800,
        "duration_s": 754
    })
}

fn teams() -> Value {
    json!({ "caller": ["Alice Martin"], "callee": ["Bob", "Carol Durand"], "app": "teams" })
}

fn say(b: &Brain, c: &Caller, space: &str, text: &str, call: Value) -> Result<Value, ToolError> {
    tools::call(
        b,
        c,
        "remember",
        &json!({ "space": space, "text": text, "call": call }),
    )
}

#[test]
fn an_iphone_call_becomes_fields_and_tags() -> Res {
    let t = Temp::new("call-iphone");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let r = say(&b, &c, "personal", "Call about the contract.", iphone())?;
    let f = &r["fact"];
    for tag in [
        "field/caller/alice-martin",
        "field/callee/33612345678",
        "field/call_app/phone",
        "field/call_direction/in",
        "field/call_started_at/1763380800",
        "field/call_duration_s/754",
        "personnes/alice-martin",
        "appel",
        "appel/phone",
        "personal",
    ] {
        assert!(has(f, tag), "{tag} missing in {f}");
    }
    // A number names nobody.
    assert!(!tags(f).iter().any(|t| t.starts_with("personnes/3")), "{f}");
    assert_eq!(f["fields"]["caller"], json!(["alice-martin"]), "{f}");
    assert_eq!(f["fields"]["callee"], json!(["+33612345678"]), "{f}");
    assert_eq!(f["fields"]["call_app"], "phone", "{f}");
    assert_eq!(f["fields"]["call_direction"], "in", "{f}");
    Ok(())
}

#[test]
fn a_teams_meeting_with_three_people_has_one_tag_each() -> Res {
    let t = Temp::new("call-teams");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let f = say(&b, &c, "work", "Weekly sync.", teams())?["fact"].clone();
    for tag in [
        "field/caller/alice-martin",
        "field/callee/bob",
        "field/callee/carol-durand",
        "personnes/alice-martin",
        "personnes/bob",
        "personnes/carol-durand",
        "appel/teams",
        "field/call_app/teams",
    ] {
        assert!(has(&f, tag), "{tag} missing in {f}");
    }
    assert_eq!(f["fields"]["callee"], json!(["bob", "carol-durand"]), "{f}");
    assert!(f["fields"].get("call_direction").is_none(), "{f}");
    Ok(())
}

#[test]
fn the_slug_keeps_a_slash_as_a_level_and_folds_accents() -> Res {
    let c = Call::parse(
        &json!({ "caller": ["Zoé Lefèvre", "ops/oncall"], "callee": ["0033 1 23 45"], "app": "Zoom" }),
    )?;
    let tags = c.tags();
    assert!(
        tags.contains(&"field/caller/zoe-lefevre".to_string()),
        "{tags:?}"
    );
    assert!(
        tags.contains(&"field/caller/ops/oncall".to_string()),
        "{tags:?}"
    );
    assert!(
        tags.contains(&"personnes/ops/oncall".to_string()),
        "{tags:?}"
    );
    assert!(
        tags.contains(&"field/callee/3312345".to_string()),
        "{tags:?}"
    );
    assert!(tags.contains(&"appel/zoom".to_string()), "{tags:?}");
    Ok(())
}

#[test]
fn the_form_is_checked() -> Res {
    let t = Temp::new("call-validation");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let many: Vec<String> = (0..21)
        .map(|i| format!("person {}", (b'a' + i) as char))
        .collect();
    let twenty: Vec<String> = many[..20].to_vec();
    let long = "x".repeat(81);
    let bad = [
        json!("alice"),
        json!({ "callee": ["bob"], "app": "phone" }),
        json!({ "caller": [], "callee": [], "app": "phone" }),
        json!({ "caller": ["alice"], "callee": ["bob"] }),
        json!({ "caller": ["alice"], "callee": ["bob"], "app": "skype" }),
        json!({ "caller": ["alice"], "callee": ["bob"], "app": "phone", "direction": "sideways" }),
        json!({ "caller": ["alice"], "callee": ["bob"], "app": "phone", "duration_s": -3 }),
        json!({ "caller": ["alice"], "callee": ["bob"], "app": "phone", "started_at": "yesterday" }),
        json!({ "caller": ["alice"], "callee": [7], "app": "phone" }),
        json!({ "caller": ["al\u{0}ice"], "callee": ["bob"], "app": "phone" }),
        json!({ "caller": ["al\nice"], "callee": ["bob"], "app": "phone" }),
        json!({ "caller": [long], "callee": ["bob"], "app": "phone" }),
        json!({ "caller": ["!!!"], "callee": ["bob"], "app": "phone" }),
        json!({ "caller": many, "callee": [], "app": "phone" }),
    ];
    for (i, call) in bad.iter().enumerate() {
        let r = say(&b, &c, "home", &format!("note {i}"), call.clone());
        assert!(matches!(r, Err(ToolError::Invalid(_))), "{call}: {r:?}");
    }
    // The limits themselves pass: twenty people, eighty characters.
    let ok = say(
        &b,
        &c,
        "home",
        "twenty",
        json!({ "caller": twenty, "callee": [], "app": "meet" }),
    )?;
    assert_eq!(ok["result"], "created", "{ok}");
    let eighty = "y".repeat(80);
    say(
        &b,
        &c,
        "home",
        "eighty",
        json!({ "caller": [eighty], "callee": ["bob"], "app": "other" }),
    )?;
    // Nothing is written for a refused call.
    let r = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "space": "home", "query": "note" }),
    )?;
    assert_eq!(r["results"].as_array().map(Vec::len), Some(0), "{r}");
    Ok(())
}

#[test]
fn a_deposit_record_carries_its_call() -> Res {
    let t = Temp::new("call-deposit");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let r = tools::call(
        &b,
        &c,
        "deposit",
        &json!({ "space": "work", "records": [
            { "text": "Meeting one.", "origin": { "app": "calls", "id": "1" }, "call": teams() },
            { "text": "Meeting two.", "origin": { "app": "calls", "id": "2" },
              "call": { "caller": [], "callee": [], "app": "teams" } },
        ] }),
    )?;
    assert_eq!(r["results"][0]["result"], "created", "{r}");
    assert_eq!(r["results"][1]["result"], "error", "{r}");
    let id = r["results"][0]["id"].as_str().ok_or("id")?;
    let f = tools::call(&b, &c, "read_fact", &json!({ "id": id }))?;
    let f = f.get("fact").unwrap_or(&f);
    assert!(has(f, "personnes/bob"), "{f}");
    assert_eq!(f["fields"]["caller"], json!(["alice-martin"]), "{f}");
    Ok(())
}

fn m4a() -> Vec<u8> {
    let mut b = vec![
        0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ',
    ];
    b.extend_from_slice(&[0; 32]);
    b
}

#[test]
fn a_recording_keeps_its_call_on_its_description() -> Res {
    let t = Temp::new("call-file");
    let b = brain(&t)?;
    let c = caller(&b, "device", "all")?;
    let put = tools::call(
        &b,
        &c,
        "file_put",
        &json!({
            "space": "personal", "name": "call-1.m4a", "note": "Call recording",
            "tags": ["inbox/audio"],
            "data_base64": base64::engine::general_purpose::STANDARD.encode(m4a()),
            "call": iphone(),
        }),
    )?;
    let fact_id = put["file"]["fact_id"].as_str().ok_or("fact id")?;
    let f = tools::call(&b, &c, "read_fact", &json!({ "id": fact_id }))?;
    let f = f.get("fact").unwrap_or(&f);
    for tag in [
        "inbox/audio",
        "personnes/alice-martin",
        "appel/phone",
        "field/callee/33612345678",
    ] {
        assert!(has(f, tag), "{tag} missing in {f}");
    }
    assert_eq!(f["fields"]["callee"], json!(["+33612345678"]), "{f}");
    // A bad call writes no file.
    let bad = tools::call(
        &b,
        &c,
        "file_put",
        &json!({
            "space": "personal", "name": "call-2.m4a", "tags": ["inbox/audio"],
            "data_base64": base64::engine::general_purpose::STANDARD.encode(m4a()),
            "call": { "caller": ["a"], "callee": ["b"], "app": "carrier-pigeon" },
        }),
    );
    assert!(matches!(bad, Err(ToolError::Invalid(_))), "{bad:?}");
    Ok(())
}

#[test]
fn the_fields_come_back_from_recall_and_history() -> Res {
    let t = Temp::new("call-expose");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    say(&b, &c, "work", "Weekly sync about budget.", teams())?;
    // Scalar fields stay scalar next to the lists.
    let r = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "space": "work", "query": "budget", "tags": ["appel"] }),
    )?;
    let hit = &r["results"][0];
    assert_eq!(hit["fields"]["caller"], json!(["alice-martin"]), "{r}");
    assert_eq!(hit["fields"]["call_app"], "teams", "{r}");
    Ok(())
}

#[test]
fn tags_set_corrects_the_participants() -> Res {
    let t = Temp::new("call-correct");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let f = say(&b, &c, "work", "Weekly sync.", teams())?["fact"].clone();
    // Remove a wrong participant: the person tag goes with the field.
    let r = tools::call(
        &b,
        &c,
        "tags_set",
        &json!({ "id": f["id"], "remove": ["field/callee/bob"], "add": ["field/callee/dave"], "learn": false }),
    )?;
    let g = &r["fact"];
    assert!(
        !has(g, "field/callee/bob") && !has(g, "personnes/bob"),
        "{g}"
    );
    assert!(
        has(g, "field/callee/dave") && has(g, "personnes/dave"),
        "{g}"
    );
    assert_eq!(
        g["fields"]["callee"],
        json!(["carol-durand", "dave"]),
        "{g}"
    );
    // Whole list through `fields`.
    let r = tools::call(
        &b,
        &c,
        "tags_set",
        &json!({ "id": g["id"], "fields": { "callee": ["Erin", "+44 20 7946 0958"] }, "learn": false }),
    )?;
    let h = &r["fact"];
    assert_eq!(
        h["fields"]["callee"],
        json!(["+442079460958", "erin"]),
        "{h}"
    );
    assert!(
        has(h, "personnes/erin") && !has(h, "personnes/carol-durand"),
        "{h}"
    );
    assert!(has(h, "personnes/alice-martin"), "{h}");
    // Caller emptied.
    let r = tools::call(
        &b,
        &c,
        "tags_set",
        &json!({ "id": h["id"], "fields": { "caller": [] }, "learn": false }),
    )?;
    assert!(r["fact"]["fields"].get("caller").is_none(), "{r}");
    assert!(!has(&r["fact"], "personnes/alice-martin"), "{r}");
    Ok(())
}

#[test]
fn a_person_stays_when_still_on_the_other_side() -> Res {
    let t = Temp::new("call-both-sides");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    let f = say(
        &b,
        &c,
        "work",
        "Callback.",
        json!({ "caller": ["Alice"], "callee": ["Alice", "Bob"], "app": "phone" }),
    )?["fact"]
        .clone();
    let r = tools::call(
        &b,
        &c,
        "tags_set",
        &json!({ "id": f["id"], "remove": ["field/caller/alice"], "learn": false }),
    )?;
    assert!(has(&r["fact"], "personnes/alice"), "{r}");
    Ok(())
}

#[test]
fn the_autotagger_does_not_duplicate_the_call_fields() -> Res {
    let t = Temp::new("call-autotag");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    // The text says a measure; the call fields are not touched, and each tag is there once.
    let f = say(
        &b,
        &c,
        "health",
        "Doctor call, tension 12/8, douleur 3/10.",
        json!({ "caller": ["Dr Alice"], "callee": ["Bob"], "app": "facetime" }),
    )?["fact"]
        .clone();
    let all = tags(&f);
    let mut sorted = all.clone();
    sorted.dedup();
    assert_eq!(all, sorted, "{f}");
    assert_eq!(
        all.iter()
            .filter(|t| t.starts_with("field/call_app/"))
            .count(),
        1,
        "{f}"
    );
    assert!(has(&f, "field/tension/12/8"), "{f}");
    assert_eq!(f["fields"]["caller"], json!(["dr-alice"]), "{f}");
    Ok(())
}

#[test]
fn people_names_counts_the_distinct_names() -> Res {
    let t = Temp::new("call-people");
    let b = brain(&t)?;
    let c = caller(&b, "app", "all")?;
    say(&b, &c, "work", "One.", teams())?;
    say(
        &b,
        &c,
        "work",
        "Two.",
        json!({ "caller": ["Alice Martin"], "callee": ["+33612345678"], "app": "phone" }),
    )?;
    say(
        &b,
        &c,
        "home",
        "Three.",
        json!({ "caller": ["Bob"], "callee": ["Zed"], "app": "phone" }),
    )?;
    let work = call::people_names(&b, Some(&sp("work")))?;
    assert_eq!(
        work,
        vec![
            ("alice-martin".to_string(), 2),
            ("bob".to_string(), 1),
            ("carol-durand".to_string(), 1)
        ]
    );
    let all = call::people_names(&b, None)?;
    assert_eq!(all[0], ("alice-martin".to_string(), 2));
    assert_eq!(all[1], ("bob".to_string(), 2));
    assert!(all.contains(&("zed".to_string(), 1)));
    assert!(all.iter().all(|(n, _)| !n.starts_with('3')));
    // A retired note no longer counts.
    let r = tools::call(
        &b,
        &c,
        "recall",
        &json!({ "space": "home", "query": "Three" }),
    )?;
    tools::call(
        &b,
        &c,
        "retire",
        &json!({ "id": r["results"][0]["id"], "reason": "test" }),
    )?;
    assert!(
        !call::people_names(&b, None)?
            .iter()
            .any(|(n, _)| n == "zed")
    );
    Ok(())
}

#[test]
fn rights_apply_to_a_call_like_to_any_write() -> Res {
    let t = Temp::new("call-rights");
    let b = brain(&t)?;
    let reader = caller(&b, "reader", "work:r")?;
    let r = say(&b, &reader, "work", "Nope.", teams());
    assert!(matches!(r, Err(ToolError::Unknown)), "{r:?}");
    let other = caller(&b, "home-only", "home:rw")?;
    let r = say(&b, &other, "work", "Nope.", teams());
    assert!(r.is_err(), "{r:?}");
    // A key limited to paths still says who spoke, but writes only under its paths: a tag of its own outside them is refused.
    let scoped = caller(&b, "scoped", "work/sources/app:rw")?;
    let r = tools::call(
        &b,
        &scoped,
        "remember",
        &json!({ "space": "work", "text": "Scoped call.", "tags": ["home/x"], "call": teams() }),
    );
    assert!(matches!(r, Err(ToolError::Denied(_))), "{r:?}");
    let ok = say(&b, &scoped, "work", "Scoped call.", teams())?;
    assert_eq!(ok["result"], "created", "{ok}");
    assert!(
        has(&ok["fact"], "sources/app") && has(&ok["fact"], "personnes/bob"),
        "{ok}"
    );
    let r = say(
        &b,
        &scoped,
        "work",
        "Bad one.",
        json!({ "caller": ["a"], "callee": [], "app": "x" }),
    );
    assert!(matches!(r, Err(ToolError::Invalid(_))), "{r:?}");
    // A reader sees the fields of the notes it can read.
    let owner = caller(&b, "owner-app", "all")?;
    say(&b, &owner, "work", "Visible.", teams())?;
    let seen = tools::call(
        &b,
        &reader,
        "recall",
        &json!({ "space": "work", "query": "Visible" }),
    )?;
    assert_eq!(
        seen["results"][0]["fields"]["callee"],
        json!(["bob", "carol-durand"])
    );
    Ok(())
}
