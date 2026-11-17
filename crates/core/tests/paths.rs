//! A connection limited to paths inside a vault (`health/sources/bevel:rw`): it reads and writes
//! only what is filed under them, sees only the few tools that can be bounded to a path, and
//! learns nothing about what lies outside.

mod common;

use std::error::Error;

use common::{Res, Temp, sp};
use memcp_core::Brain;
use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::tools::{self, ToolError};
use memcp_core::users::{self, OWNER};
use rusqlite::Connection;
use serde_json::{Value, json};

struct World {
    _t: Temp,
    brain: Brain,
    ctl: Connection,
    sam: String,
}

fn world() -> Result<World, Box<dyn Error>> {
    let t = Temp::new("paths");
    let brain = Brain::init(&t.path().join("data"), &[sp("legacy")], Language::English)?;
    let ctl = brain.open_control()?;
    let sam = users::create_user(&ctl, OWNER, "sam", "Sam", false)?.id;
    brain.add_vault(&sam, "sam", "health")?;
    Ok(World {
        _t: t,
        brain,
        ctl,
        sam,
    })
}

fn caller(w: &World, agent: &str, spec: &str) -> Result<access::Caller, Box<dyn Error>> {
    let r = Rights::parse(spec)?;
    let t = access::connect_for(&w.ctl, &w.sam, agent, &r, None, false, false)?;
    Ok(access::authenticate(&w.ctl, &t, 1_900_000_000)?.ok_or("refused")?)
}

fn call(w: &World, c: &access::Caller, tool: &str, args: Value) -> Result<Value, ToolError> {
    tools::call(&w.brain, c, tool, &args)
}

fn texts(v: &Value) -> Vec<String> {
    v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r["text"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_path_is_parsed_labelled_and_never_mixed_with_the_whole_vault() -> Res {
    let r = Rights::parse("sam-health/sources/bevel:rw,work:r")?;
    assert_eq!(r.paths.len(), 1);
    assert_eq!(r.label(), "work:r,sam-health/sources/bevel:rw");
    for bad in [
        "sam-health:rw,sam-health/sources/bevel:rw",
        "sam-health/sources/bevel:rw,sam-health:r",
        "sam-health/@work:rw",
        "sam-health/sources/bevel:rw,sam-health/sources/bevel:r",
        "sam-health/Not A Tag!:rw",
    ] {
        assert!(Rights::parse(bad).is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn such_a_connection_sees_only_the_tools_that_can_be_bounded() -> Res {
    let w = world()?;
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    let names: Vec<&str> = tools::definitions(&bevel).iter().map(|d| d.name).collect();
    for n in ["recall", "remember", "retire", "file_put", "stats"] {
        assert!(names.contains(&n), "{n}: {names:?}");
    }
    for n in [
        "read_fact",
        "history",
        "supersede",
        "file_get",
        "file_list",
        "file_retire",
    ] {
        assert!(!names.contains(&n), "{n} must not be offered: {names:?}");
        let r = call(&w, &bevel, n, json!({ "id": "x" }));
        assert!(matches!(r, Err(ToolError::Unknown)), "{n}: {r:?}");
    }
    Ok(())
}

#[test]
fn it_writes_under_its_path_and_nowhere_else() -> Res {
    let w = world()?;
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    // No tag: filed under its single path.
    let r = call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Weight 2026-11-01: 71.0 kg" }),
    )?;
    assert_eq!(r["result"], "created", "{r}");
    assert!(
        r["fact"]["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x == "sources/bevel"))
    );
    // An origin files it under its own source: allowed when that is the path.
    let r = call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Steps 2026-11-01: 9100", "origin": { "app": "bevel", "id": "2026-11-01/steps" } }),
    )?;
    assert_eq!(r["result"], "created", "{r}");
    // Another tag outside the path, another app's source, the sorted layer: refused.
    for args in [
        json!({ "text": "Outside", "tags": ["symptoms/dizziness"] }),
        json!({ "text": "As another app", "origin": { "app": "healthapp", "id": "x" } }),
        json!({ "text": "As the dossier", "origin": { "app": "bevel", "id": "x", "layer": "dossier" } }),
    ] {
        let r = call(&w, &bevel, "remember", args.clone());
        assert!(matches!(r, Err(ToolError::Denied(_))), "{args}: {r:?}");
    }
    // A file too.
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode("a,b\n1,2\n");
    let r = call(
        &w,
        &bevel,
        "file_put",
        json!({ "name": "export.csv", "data_base64": data, "tags": ["symptoms"] }),
    );
    assert!(matches!(r, Err(ToolError::Denied(_))), "{r:?}");
    let ok = call(
        &w,
        &bevel,
        "file_put",
        json!({ "name": "export.csv", "data_base64": data }),
    )?;
    assert_eq!(ok["result"], "kept", "{ok}");
    Ok(())
}

#[test]
fn it_reads_only_what_lies_under_its_path() -> Res {
    let w = world()?;
    let owner = caller(&w, "owner-key", "all")?;
    call(
        &w,
        &owner,
        "remember",
        json!({ "space": "sam-health", "text": "Penicillin allergy since childhood", "tags": ["allergies"] }),
    )?;
    call(
        &w,
        &owner,
        "remember",
        json!({ "space": "sam-health", "text": "Untagged note about the knee" }),
    )?;
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    let healthfit = caller(&w, "healthfit", "sam-health/sources/healthfit:rw")?;
    call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "[Bevel] Resting heart rate 2026-11-01: 58 bpm" }),
    )?;
    call(
        &w,
        &healthfit,
        "remember",
        json!({ "text": "[HealthFit] Resting heart rate 2026-11-01: 59 bpm" }),
    )?;

    // Its own records come back although the raw layer is left out of a plain recall elsewhere.
    let mine = texts(&call(
        &w,
        &bevel,
        "recall",
        json!({ "query": "resting heart rate" }),
    )?);
    assert_eq!(mine, vec!["[Bevel] Resting heart rate 2026-11-01: 58 bpm"]);
    // The owner's facts and another app's are invisible to it.
    for q in ["penicillin allergy", "knee", "HealthFit"] {
        assert!(
            texts(&call(&w, &bevel, "recall", json!({ "query": q }))?).is_empty(),
            "{q}"
        );
    }
    // Asking for another path by tag does not widen it.
    let r = call(
        &w,
        &bevel,
        "recall",
        json!({ "query": "resting heart rate", "tags": ["sources/healthfit"] }),
    )?;
    assert!(texts(&r).is_empty());
    Ok(())
}

#[test]
fn it_is_never_told_about_facts_outside_its_path() -> Res {
    let w = world()?;
    let owner = caller(&w, "owner-key", "all")?;
    call(
        &w,
        &owner,
        "remember",
        json!({ "space": "sam-health", "text": "Heart rate 2026-11-01: 61 bpm in the morning", "tags": ["vitals"] }),
    )?;
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    // Identical text held outside its path: nothing written, nothing shown.
    let same = call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Heart rate 2026-11-01: 61 bpm in the morning" }),
    )?;
    assert_eq!(same["result"], "duplicate", "{same}");
    assert!(same.get("fact").is_none(), "{same}");
    // A close fact outside its path is no reason to stop, and is not named.
    let close = call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Heart rate 2026-11-01: 61 bpm in the evening" }),
    )?;
    assert_eq!(close["result"], "created", "{close}");
    assert!(
        close.get("tidy").is_none() && !close.to_string().contains("morning"),
        "{close}"
    );
    // The same through an origin.
    let o = call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Heart rate 2026-11-01: 61 bpm in the morning", "origin": { "app": "bevel", "id": "hr" } }),
    )?;
    assert_eq!(o["result"], "same_text_held", "{o}");
    assert!(o["fact"].is_null(), "{o}");
    Ok(())
}

#[test]
fn it_cannot_retire_what_it_did_not_write_under_its_path() -> Res {
    let w = world()?;
    let owner = caller(&w, "owner-key", "all")?;
    let theirs = call(
        &w,
        &owner,
        "remember",
        json!({ "space": "sam-health", "text": "A fact of the owner", "tags": ["vitals"] }),
    )?;
    let id = theirs["fact"]["id"].as_str().ok_or("id")?.to_string();
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    let r = call(&w, &bevel, "retire", json!({ "id": id, "reason": "mine" }));
    assert!(
        matches!(r, Err(ToolError::Invalid(_))),
        "unknown, not forbidden: {r:?}"
    );
    // Its own, yes.
    let own = call(&w, &bevel, "remember", json!({ "text": "Own record" }))?;
    let own_id = own["fact"]["id"].as_str().ok_or("id")?.to_string();
    assert_eq!(
        call(
            &w,
            &bevel,
            "retire",
            json!({ "id": own_id, "reason": "wrong" })
        )?["result"],
        "retired"
    );
    // The owner's fact is untouched.
    let still = call(
        &w,
        &owner,
        "recall",
        json!({ "query": "fact of the owner" }),
    )?;
    assert_eq!(texts(&still).len(), 1);
    Ok(())
}

#[test]
fn its_counts_describe_only_what_falls_under_its_path() -> Res {
    let w = world()?;
    let owner = caller(&w, "owner-key", "all")?;
    call(
        &w,
        &owner,
        "remember",
        json!({ "space": "sam-health", "text": "Secret diagnosis note", "tags": ["conditions/private"] }),
    )?;
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Steps 2026-11-01: 9100", "origin": { "app": "bevel", "id": "s" } }),
    )?;
    let stats = call(&w, &bevel, "stats", json!({}))?;
    let h = &stats["sam-health"];
    assert_eq!(h["restricted_to"], json!(["sources/bevel"]), "{stats}");
    assert_eq!(h["sources"]["bevel"]["facts"], 1);
    assert!(
        h.get("tags").is_none() && h.get("active").is_none(),
        "{stats}"
    );
    assert!(!stats.to_string().contains("conditions"), "{stats}");
    Ok(())
}

#[test]
fn a_connection_with_several_write_paths_must_say_which() -> Res {
    let w = world()?;
    let healthapp = caller(
        &w,
        "healthapp",
        "sam-health/sources/healthapp:rw,sam-health/proposals/healthapp:rw",
    )?;
    let r = call(
        &w,
        &healthapp,
        "remember",
        json!({ "text": "No tag given" }),
    );
    assert!(matches!(r, Err(ToolError::Denied(_))), "{r:?}");
    let ok = call(
        &w,
        &healthapp,
        "remember",
        json!({ "text": "A proposal", "tags": ["proposals/healthapp"] }),
    )?;
    assert_eq!(ok["result"], "created", "{ok}");
    let read = texts(&call(
        &w,
        &healthapp,
        "recall",
        json!({ "query": "proposal", "tags": ["proposals/healthapp"] }),
    )?);
    assert_eq!(read, vec!["A proposal"]);
    Ok(())
}

#[test]
fn a_read_only_path_reads_and_cannot_write() -> Res {
    let w = world()?;
    let bevel = caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    call(
        &w,
        &bevel,
        "remember",
        json!({ "text": "Weight 2026-11-01: 71.0 kg" }),
    )?;
    let reader = caller(&w, "reader", "sam-health/sources/bevel:r")?;
    assert_eq!(
        texts(&call(&w, &reader, "recall", json!({ "query": "weight" }))?).len(),
        1
    );
    let names: Vec<&str> = tools::definitions(&reader).iter().map(|d| d.name).collect();
    assert!(!names.contains(&"remember"), "{names:?}");
    Ok(())
}

#[test]
fn rights_with_paths_survive_a_regrant_and_show_in_the_list() -> Res {
    let w = world()?;
    caller(&w, "bevel", "sam-health/sources/bevel:rw")?;
    let list = access::list(&w.ctl)?;
    assert!(
        list.iter()
            .any(|c| c.agent == "bevel" && c.rights == "sam-health/sources/bevel:rw"),
        "{list:?}"
    );
    access::regrant(
        &w.ctl,
        "bevel",
        &Rights::parse("sam-health/sources/bevel:r,sam-health/sources/other:rw")?,
    )?;
    let list = access::list(&w.ctl)?;
    let c = list.iter().find(|c| c.agent == "bevel").ok_or("gone")?;
    assert_eq!(
        c.rights,
        "sam-health/sources/bevel:r,sam-health/sources/other:rw"
    );
    // Widening back to the whole vault drops the paths.
    access::regrant(&w.ctl, "bevel", &Rights::parse("sam-health:rw")?)?;
    let c2 = access::authenticate(&w.ctl, "no", 0)?;
    assert!(c2.is_none());
    Ok(())
}

#[test]
fn one_tag_of_its_own_does_not_carry_the_others_outside_its_path() -> Res {
    let w = world()?;
    let app = caller(&w, "healthapp", "sam-health/sources/healthapp:rw")?;
    let denied = |r: Result<Value, ToolError>| matches!(r, Err(ToolError::Denied(_)));
    // Its own records: fine.
    let ok = call(
        &w,
        &app,
        "remember",
        json!({"space": "sam-health", "text": "Sommeil 7 h", "origin": {"app": "healthapp", "id": "s/1"}}),
    )?;
    assert_eq!(ok["result"], "created", "{ok}");
    // Another app's record, the sorted layer, the inbox, the index: refused, even next to its own tag.
    assert!(denied(call(
        &w,
        &app,
        "remember",
        json!({"space": "sam-health", "text": "Forged HRV", "tags": ["sources/healthapp"], "origin": {"app": "bevel", "id": "hrv/1"}}),
    )));
    assert!(denied(call(
        &w,
        &app,
        "remember",
        json!({"space": "sam-health", "text": "Forged dossier", "origin": {"app": "expert", "id": "d/1", "layer": "dossier"}}),
    )));
    for extra in ["inbox", "index", "dossier/expert"] {
        assert!(
            denied(call(
                &w,
                &app,
                "remember",
                json!({"space": "sam-health", "text": format!("Planted {extra}"), "tags": ["sources/healthapp", extra]}),
            )),
            "{extra}"
        );
    }
    let batch = call(
        &w,
        &app,
        "deposit",
        json!({"space": "sam-health", "records": [
            {"text": "own", "origin": {"app": "healthapp", "id": "s/2"}},
            {"text": "other", "origin": {"app": "bevel", "id": "x"}},
        ]}),
    )?;
    assert_eq!(batch["results"][0]["result"], "created", "{batch}");
    assert_eq!(batch["results"][1]["result"], "error", "{batch}");
    Ok(())
}

#[test]
fn the_handshake_tells_a_path_limited_key_nothing_outside_its_path() -> Res {
    let w = world()?;
    let owner = caller(&w, "owner-key", "sam-health:rw")?;
    call(
        &w,
        &owner,
        "remember",
        json!({"space": "sam-health", "text": "Index: treatments under treatments/, the cardiologist's letters under letters/", "tags": ["index"]}),
    )?;
    call(
        &w,
        &owner,
        "remember",
        json!({"space": "sam-health", "text": "A note to sort", "tags": ["inbox"]}),
    )?;
    memcp_core::guide::set(
        &w.ctl,
        &w.sam,
        "space:sam-health",
        "Private rules of the health vault",
    )?;
    let whole = memcp_core::guide::instructions(&w.brain, &owner)?;
    assert!(
        whole.contains("cardiologist")
            && whole.contains("Private rules")
            && whole.contains("INBOX")
    );
    let app = caller(&w, "healthapp", "sam-health/sources/healthapp:rw")?;
    let told = memcp_core::guide::instructions(&w.brain, &app)?;
    for secret in ["cardiologist", "Private rules", "INBOX", "INDEX"] {
        assert!(!told.contains(secret), "{secret} leaked: {told}");
    }
    Ok(())
}

#[test]
fn a_connection_can_be_kept_from_the_persons_own_guide() -> Res {
    let w = world()?;
    w.brain.add_vault(&w.sam, "sam", "work")?;
    memcp_core::guide::set(
        &w.ctl,
        &w.sam,
        &format!("person:{}", w.sam),
        "My family, my knee: private",
    )?;
    memcp_core::guide::set(&w.ctl, &w.sam, "space:sam-work", "Work rules")?;
    let token = access::connect_for(
        &w.ctl,
        &w.sam,
        "pro",
        &Rights::parse("sam-work:rw")?,
        None,
        false,
        false,
    )?;
    let auth = || -> Result<access::Caller, Box<dyn Error>> {
        Ok(access::authenticate(&w.ctl, &token, 1_900_000_000)?.ok_or("refused")?)
    };
    assert!(
        memcp_core::guide::instructions(&w.brain, &auth()?)?.contains("private"),
        "told by default"
    );
    assert!(access::set_no_person_guide(&w.ctl, "pro", true)?);
    let told = memcp_core::guide::instructions(&w.brain, &auth()?)?;
    assert!(
        told.contains("Work rules") && !told.contains("private"),
        "{told}"
    );
    assert!(
        access::list(&w.ctl)?
            .iter()
            .any(|c| c.agent == "pro" && c.no_person_guide)
    );
    access::set_no_person_guide(&w.ctl, "pro", false)?;
    assert!(memcp_core::guide::instructions(&w.brain, &auth()?)?.contains("private"));
    Ok(())
}

#[test]
fn an_assistant_is_told_which_tools_read_the_proposals_of_a_vault() -> Res {
    let w = world()?;
    caller(
        &w,
        "healthapp",
        "sam-health/sources/healthapp:rw,sam-health/proposals/healthapp:r",
    )?;
    let owner = caller(&w, "claude", "sam-health:rw")?;
    let told = memcp_core::guide::instructions(&w.brain, &owner)?;
    assert!(
        told.contains("PROPOSALS of the space \"sam-health\"")
            && told.contains("`healthapp` (tag `proposals/healthapp`)"),
        "{told}"
    );
    // Not to a key that only reads, nor to the tool itself.
    let reader = caller(&w, "reader", "sam-health:r")?;
    assert!(!memcp_core::guide::instructions(&w.brain, &reader)?.contains("PROPOSALS"));
    Ok(())
}
