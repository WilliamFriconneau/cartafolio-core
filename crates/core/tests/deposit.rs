//! `deposit`: many records of an app in one call. Each takes the road of a `remember` with its origin, so the
//! rules (rights by path, sizes, ids, layers) are the same, and one bad record never stops the others.

mod common;

use std::error::Error;

use common::{Res, Temp, sp};
use pensine_core::Brain;
use pensine_core::access::{self, Rights};
use pensine_core::text::Language;
use pensine_core::tools::{self, ToolError};
use pensine_core::users::{self, OWNER};
use rusqlite::Connection;
use serde_json::{Value, json};

struct World {
    _t: Temp,
    brain: Brain,
    ctl: Connection,
    sam: String,
}

fn world() -> Result<World, Box<dyn Error>> {
    let t = Temp::new("deposit");
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

fn call(w: &World, c: &access::Caller, args: Value) -> Result<Value, ToolError> {
    tools::call(&w.brain, c, "deposit", &args)
}

fn record(i: usize, text: &str) -> Value {
    json!({ "text": text, "origin": { "app": "healthapp", "id": format!("note/e{i}"), "at": "2026-11-02" } })
}

fn results(v: &Value) -> Vec<String> {
    v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| r["result"].as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_batch_is_deposited_in_one_call_and_resumes_by_id() -> Res {
    let w = world()?;
    let c = caller(&w, "healthapp", "sam-health/sources/healthapp:rw")?;
    let batch: Vec<Value> = (0..40)
        .map(|i| record(i, &format!("[HealthApp] note e{i}: sommeil {i}")))
        .collect();
    let r = call(&w, &c, json!({ "records": batch }))?;
    assert_eq!(results(&r), vec!["created"; 40]);
    assert_eq!(r["failed"], 0);
    assert_eq!(r["cursors"][0]["app"], "healthapp");
    assert_eq!(r["cursors"][0]["facts"], 40);
    // The same batch again changes nothing; one changed record replaces its earlier version.
    let mut again: Vec<Value> = (0..40)
        .map(|i| record(i, &format!("[HealthApp] note e{i}: sommeil {i}")))
        .collect();
    again[7] = record(7, "[HealthApp] note e7: sommeil corrigé");
    let r = call(&w, &c, json!({ "records": again }))?;
    let got = results(&r);
    assert_eq!(got[7], "updated", "{r}");
    assert_eq!(got.iter().filter(|x| *x == "unchanged").count(), 39);
    // What was deposited is under the app's own path, and findable.
    let found = tools::call(
        &w.brain,
        &c,
        "recall",
        &json!({ "query": "corrigé", "tags": ["sources/healthapp"] }),
    )?;
    assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn a_bad_record_is_told_by_its_place_and_stops_nothing() -> Res {
    let w = world()?;
    let c = caller(&w, "healthapp", "sam-health/sources/healthapp:rw")?;
    let r = call(
        &w,
        &c,
        json!({ "records": [
            record(0, "[HealthApp] bon"),
            { "text": "sans origine" },
            "pas un objet",
            record(3, ""),
            { "text": "dossier hors chemin", "origin": { "app": "healthapp", "id": "d1", "layer": "dossier" } },
            record(5, "[HealthApp] autre bon"),
        ] }),
    )?;
    let got = results(&r);
    assert_eq!(got[0], "created");
    assert_eq!(&got[1..5], ["error"; 4], "{got:?}");
    assert_eq!(
        got[5], "created",
        "the records after a failure are still kept"
    );
    assert_eq!(r["failed"], 4);
    for i in 1..5 {
        assert!(
            r["results"][i]["error"]
                .as_str()
                .is_some_and(|m| !m.is_empty())
        );
        assert_eq!(r["results"][i]["index"], i);
    }
    Ok(())
}

#[test]
fn the_number_of_records_and_the_rights_are_bounded() -> Res {
    let w = world()?;
    let c = caller(&w, "healthapp", "sam-health/sources/healthapp:rw")?;
    assert!(call(&w, &c, json!({ "records": [] })).is_err());
    assert!(call(&w, &c, json!({})).is_err());
    let many: Vec<Value> = (0..1200).map(|i| record(i, &format!("note {i}"))).collect();
    assert_eq!(
        results(&call(&w, &c, json!({ "records": many }))?).len(),
        1200,
        "no limit on the number of records"
    );
    // A key that only reads has no such tool; one limited to another path gets a refusal per record.
    let reader = caller(&w, "viewer", "sam-health:r")?;
    assert!(matches!(
        call(&w, &reader, json!({ "records": [record(0, "x")] })),
        Err(ToolError::Unknown)
    ));
    let inbox = caller(&w, "device", "sam-health/inbox:rw")?;
    let r = call(
        &w,
        &inbox,
        json!({ "records": [record(0, "hors de son chemin")] }),
    )?;
    assert_eq!(results(&r), vec!["error"]);
    let names: Vec<&str> = tools::definitions(&c).iter().map(|d| d.name).collect();
    assert!(names.contains(&"deposit"), "{names:?}");
    Ok(())
}
