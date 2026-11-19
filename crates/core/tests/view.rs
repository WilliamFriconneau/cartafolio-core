//! The space view contract: capabilities, dashboard, targets and the timestamped register, on a
//! generic space, a health space and (in the server's tests) a money space; and its rights.

mod common;

use std::collections::BTreeSet;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::tools::{self, ToolError};
use memcp_core::users::OWNER;
use memcp_core::{Brain, util, view};
use serde_json::{Value, json};

const NOW: i64 = 1_795_000_000;
const DAY: i64 = 86_400;

fn caller(
    b: &Brain,
    name: &str,
    spec: &str,
    read_only: bool,
) -> Result<Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        name,
        &Rights::parse(spec)?,
        None,
        read_only,
        false,
    )?;
    Ok(access::authenticate(&ctl, &tok, NOW)?.ok_or("refused")?)
}

fn put(
    b: &Brain,
    space: &str,
    text: &str,
    tags: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let s = sp(space);
    let conn = b.open_space(&s)?;
    let mut n = NewFact::agent(text, "fixture");
    n.tags = tags.iter().map(|t| (*t).to_string()).collect();
    match facts::remember(&conn, &s, "fixture", n, &[])? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn at(b: &Brain, space: &str, id: &str, when: i64) -> Res {
    let conn = b.open_space(&sp(space))?;
    conn.execute(
        "UPDATE facts SET created_at = ?1, valid_from = ?1 WHERE id = ?2",
        rusqlite::params![when, id],
    )?;
    Ok(())
}

fn world() -> Result<(Temp, Brain, Caller), Box<dyn std::error::Error>> {
    let t = Temp::new("view");
    let b = brain(&t)?;
    b.set_space_kind(&sp("health"), "health")?;
    b.set_space_kind(&sp("home"), "home")?;
    let c = caller(&b, "reader", "all", true)?;
    Ok((t, b, c))
}

fn call(b: &Brain, c: &Caller, args: Value) -> Result<Value, ToolError> {
    tools::call(b, c, "space_view", &args)
}

#[test]
fn capabilities_publish_the_home_and_the_views_in_every_space() -> Res {
    let (_t, b, c) = world()?;
    for (space, sensitive, health) in [("work", false, false), ("health", true, true)] {
        let v = call(&b, &c, json!({ "space": space, "view": "capabilities" }))?;
        assert_eq!(v["contract"], view::CONTRACT, "{v}");
        assert_eq!(v["contract_version"], json!(view::CONTRACT_VERSION));
        assert_eq!(v["space"], space);
        assert_eq!(v["sensitive"], sensitive);
        assert_eq!(v["writable"], false, "a read-only caller cannot write");
        let ids: Vec<&str> = v["views"]
            .as_array()
            .ok_or("views")?
            .iter()
            .filter_map(|x| x["id"].as_str())
            .collect();
        assert_eq!(ids, ["targets", "chronology", "stats", "files"], "{v}");
        assert_eq!(v["home"], "targets", "{v}");
        assert!(v["schemas"]["target_metrics"].is_object(), "{v}");
        let types: BTreeSet<&str> = v["item_types"]
            .as_object()
            .ok_or("item_types")?
            .keys()
            .map(String::as_str)
            .collect();
        assert!(types.contains("note") && types.contains("target") && types.contains("dashboard"));
        assert_eq!(types.contains("sign"), health, "{v}");
        assert_eq!(types.contains("measure"), health, "{v}");
    }
    Ok(())
}

#[test]
fn the_register_pages_with_a_stable_cursor_and_temporal_filters() -> Res {
    let (_t, b, c) = world()?;
    let mut ids = vec![];
    let texts = [
        "Alpha: the quarterly report is due on Friday.",
        "Beta: the budget was approved on Monday.",
        "Gamma: two candidates join the team in December.",
        "Delta: the invoice arrived this morning.",
        "Epsilon: the trip to Lisbon is booked.",
    ];
    for (i, text) in texts.iter().enumerate() {
        let id = put(&b, "work", text, &["work/alpha"])?;
        at(&b, "work", &id, NOW - (i as i64) * DAY)?;
        ids.push(id);
    }
    let mut seen = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..4 {
        let mut args = json!({ "space": "work", "view": "register", "limit": 2 });
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let v = call(&b, &c, args)?;
        let items = v["items"].as_array().ok_or("items")?;
        assert!(items.len() <= 2);
        for item in items {
            let id = item["id"].as_str().ok_or("id")?.to_string();
            assert!(!seen.contains(&id), "a page repeated {id}");
            seen.push(id);
        }
        match v["page"]["next_cursor"].as_str() {
            Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
            _ => break,
        }
    }
    assert_eq!(seen, ids, "newest first, every note once");
    // The same cursor gives the same page.
    let first = call(
        &b,
        &c,
        json!({ "space": "work", "view": "register", "limit": 2 }),
    )?;
    let next = first["page"]["next_cursor"]
        .as_str()
        .ok_or("cursor")?
        .to_string();
    let a = call(
        &b,
        &c,
        json!({ "space": "work", "view": "register", "limit": 2, "cursor": next }),
    )?;
    let b2 = call(
        &b,
        &c,
        json!({ "space": "work", "view": "register", "limit": 2, "cursor": next }),
    )?;
    assert_eq!(a["items"], b2["items"], "a cursor is stable");
    // A temporal window: the two oldest notes (i = 3, 4).
    let window = call(
        &b,
        &c,
        json!({ "space": "work", "view": "register", "from": util::iso(NOW - 4 * DAY), "to": util::iso(NOW - 3 * DAY) }),
    )?;
    let texts: Vec<&str> = window["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect();
    assert_eq!(texts.len(), 2, "{window}");
    assert!(
        texts
            .iter()
            .all(|t| t.to_lowercase().contains("delta") || t.to_lowercase().contains("epsilon")),
        "{window}"
    );
    Ok(())
}

#[test]
fn types_stable_ids_and_provenance() -> Res {
    let (_t, b, c) = world()?;
    let app = put(
        &b,
        "work",
        "A scanner read the signed quote.",
        &["sources/scanner"],
    )?;
    let file = put(&b, "work", "The quote as a file.", &[])?;
    b.open_space(&sp("work"))?
        .execute("UPDATE facts SET type = 'file' WHERE id = ?1", [&file])?;
    let sign = put(
        &b,
        "health",
        "Mal de tête depuis ce matin, 6/10.",
        &["signe/cephalee", "field/level/6/10"],
    )?;
    let measure = put(
        &b,
        "health",
        "Tension 12/8 en sortant du cabinet.",
        &["mesure/tension", "field/tension/12/8"],
    )?;

    let notes = call(
        &b,
        &c,
        json!({ "space": "work", "view": "register", "type": "note" }),
    )?;
    let app_item = notes["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["id"] == app)
        .ok_or("app note missing")?;
    assert_eq!(app_item["type"], "note");
    assert_eq!(app_item["provenance"]["kind"], "app");
    assert_eq!(app_item["provenance"]["app"], "scanner");
    assert_eq!(app_item["provenance"]["layer"], "sources");

    let files = call(
        &b,
        &c,
        json!({ "space": "work", "view": "register", "type": "file" }),
    )?;
    assert!(
        files["items"]
            .as_array()
            .ok_or("items")?
            .iter()
            .any(|i| i["id"] == file && i["type"] == "file"),
        "{files}"
    );

    let signs = call(
        &b,
        &c,
        json!({ "space": "health", "view": "register", "type": "sign" }),
    )?;
    let s = signs["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["id"] == sign)
        .ok_or("sign missing")?;
    assert_eq!(s["type"], "sign");
    assert_eq!(s["sign"], "cephalee");
    assert_eq!(s["level"], "6/10");
    assert_eq!(s["label"], "Mal de tête");

    let measures = call(
        &b,
        &c,
        json!({ "space": "health", "view": "register", "type": "measure" }),
    )?;
    let m = measures["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["id"] == measure)
        .ok_or("measure missing")?;
    assert_eq!(m["type"], "measure");
    assert_eq!(m["measure"], "tension");
    assert_eq!(m["value"], "12/8");
    Ok(())
}

#[test]
fn targets_are_the_living_board() -> Res {
    let (_t, b, c) = world()?;
    let full = caller(&b, "writer", "all", false)?;
    let added = tools::call(
        &b,
        &full,
        "living_set",
        &json!({ "space": "health", "page": "targets",
                 "add": [{ "text": "Migraine (f_source)", "column": "open", "signs": ["cephalee"],
                           "fields": { "due": "2026-12-31", "target_amount": 10, "realized_amount": 4, "currency": "EUR" } }] }),
    )?;
    let id = added["added"][0].as_str().ok_or("card")?;
    let v = call(&b, &c, json!({ "space": "health", "view": "targets" }))?;
    assert_eq!(v["columns"], json!(["open", "reached"]));
    let target = v["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["fact_id"] == id)
        .ok_or("target missing")?;
    assert_eq!(target["type"], "target");
    assert_eq!(target["column"], "open");
    assert!(target["signs"].to_string().contains("headache"), "{target}");
    assert_eq!(target["metrics"]["due"], "2026-12-31");
    assert_eq!(target["metrics"]["target_amount"], 10.0);
    assert_eq!(target["metrics"]["realized_amount"], 4.0);
    assert_eq!(target["metrics"]["progress_percent"], 40.0);
    Ok(())
}

#[test]
fn home_and_hobbies_tasks_are_checkable() -> Res {
    let (_t, b, c) = world()?;
    let full = caller(&b, "writer", "all", false)?;
    let added = tools::call(
        &b,
        &full,
        "living_set",
        &json!({ "space": "home", "page": "next",
                 "add": [{ "text": "Buy a new fuse", "column": "now" }] }),
    )?;
    let id = added["added"][0].as_str().ok_or("card")?;
    let cap = call(&b, &c, json!({ "space": "home", "view": "capabilities" }))?;
    let ids: Vec<&str> = cap["views"]
        .as_array()
        .ok_or("views")?
        .iter()
        .filter_map(|v| v["id"].as_str())
        .collect();
    assert!(ids.contains(&"tasks"), "{cap}");
    let before = call(&b, &c, json!({ "space": "home", "view": "tasks" }))?;
    let task = before["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["fact_id"] == id)
        .ok_or("task missing")?;
    assert_eq!(task["type"], "task");
    assert_eq!(task["done"], false);
    let card_id = task["id"].as_str().ok_or("card id")?.to_string();
    let fact_id = task["fact_id"].as_str().ok_or("fact id")?.to_string();
    tools::call(
        &b,
        &full,
        "living_set",
        &json!({ "space": "home", "page": "next", "move": [{ "id": id, "column": "done" }] }),
    )?;
    let after = call(&b, &c, json!({ "space": "home", "view": "tasks" }))?;
    let task = after["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["text"] == "Buy a new fuse")
        .ok_or("task missing after check")?;
    assert_eq!(task["done"], true);
    assert_eq!(task["id"], card_id, "the card keeps its stable id");
    assert_ne!(
        task["fact_id"], fact_id,
        "the fact is superseded by the move"
    );
    Ok(())
}

#[test]
fn chronology_stats_and_file_inventory_are_one_contract() -> Res {
    let (_t, b, c) = world()?;
    let note = put(
        &b,
        "work",
        "Alpha: the report is due on Friday.",
        &["work/alpha"],
    )?;
    at(&b, "work", &note, NOW - DAY)?;
    let app = put(
        &b,
        "work",
        "A scanner read the signed quote.",
        &["sources/scanner"],
    )?;
    at(&b, "work", &app, NOW - 2 * DAY)?;
    let file = memcp_core::files::put(
        &b,
        &sp("work"),
        "fixture",
        "quote.txt",
        "The signed quote.",
        b"a quote",
    )?;

    let chrono = call(
        &b,
        &c,
        json!({ "space": "work", "view": "chronology", "limit": 10 }),
    )?;
    let items = chrono["items"].as_array().ok_or("items")?;
    assert!(
        items.iter().any(|i| i["id"] == note && i["type"] == "note"),
        "{chrono}"
    );
    assert!(
        items
            .iter()
            .any(|i| i["id"] == file.fact_id.clone().unwrap_or_default()),
        "{chrono}"
    );
    assert!(
        items
            .iter()
            .all(|i| i["at"].is_string() && i["provenance"].is_object()),
        "{chrono}"
    );
    let searched = call(
        &b,
        &c,
        json!({ "space": "work", "view": "chronology", "q": "scanner" }),
    )?;
    assert_eq!(
        searched["items"].as_array().map(Vec::len),
        Some(1),
        "{searched}"
    );

    let stats = call(&b, &c, json!({ "space": "work", "view": "stats" }))?;
    assert!(
        stats["stats"]["total"].as_i64().unwrap_or(0) >= 2,
        "{stats}"
    );
    assert!(
        stats["stats"]["by_day"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{stats}"
    );

    let files = call(
        &b,
        &c,
        json!({ "space": "work", "view": "files", "limit": 5 }),
    )?;
    let item = files["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .find(|i| i["id"] == file.id)
        .ok_or("file missing")?;
    assert_eq!(item["type"], "file");
    assert_eq!(item["name"], "quote.txt");
    assert_eq!(item["size"], 7);
    Ok(())
}

#[test]
fn the_dashboard_sections_have_stable_ids() -> Res {
    let (_t, b, c) = world()?;
    let doc = json!({
        "version": 1, "title": "Work",
        "sections": [
            { "type": "daily", "title": "Notes day by day", "window_days": 7 },
            { "type": "text", "title": "A word", "body": "Short text." }
        ]
    });
    put(&b, "work", &doc.to_string(), &["dashboard"])?;
    let v = call(&b, &c, json!({ "space": "work", "view": "dashboard" }))?;
    assert_eq!(v["dashboard"]["source"], "stored");
    let sections = v["dashboard"]["sections"].as_array().ok_or("sections")?;
    assert_eq!(sections[0]["id"], "section:0");
    assert_eq!(sections[0]["type"], "daily");
    assert_eq!(sections[1]["id"], "section:1");
    assert_eq!(sections[1]["type"], "text");
    Ok(())
}

#[test]
fn rights_are_the_whole_space_and_nothing_else() -> Res {
    let (_t, b, _) = world()?;
    let work = caller(&b, "work-reader", "work:r", true)?;
    let scoped = caller(&b, "work-app", "work/journal:r", true)?;
    call(
        &b,
        &work,
        json!({ "space": "work", "view": "capabilities" }),
    )?;
    call(&b, &work, json!({ "space": "work", "view": "register" }))?;
    let denied = call(&b, &work, json!({ "space": "health", "view": "register" }))
        .err()
        .ok_or("a space the caller does not read is not denied")?;
    assert!(matches!(denied, ToolError::Denied(_)), "{denied:?}");
    let ghost = call(&b, &work, json!({ "space": "nowhere", "view": "register" }))
        .err()
        .ok_or("a space that does not exist is not denied")?;
    assert!(matches!(ghost, ToolError::Denied(_)), "{ghost:?}");
    assert_eq!(
        format!("{denied:?}").replace("health", "X"),
        format!("{ghost:?}").replace("nowhere", "X"),
        "a forbidden space and an unknown one answer alike"
    );
    assert!(
        matches!(
            call(&b, &scoped, json!({ "space": "work", "view": "register" })),
            Err(ToolError::Unknown)
        ),
        "a key limited to paths does not see the whole-space view"
    );
    // A denied read wrote nothing.
    let conn = b.open_space(&sp("work"))?;
    let n: i64 = conn.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    assert_eq!(n, 0);
    Ok(())
}

#[test]
fn fixtures_are_well_formed_examples_for_generic_health_and_finances() -> Res {
    for (name, kind, health, money) in [
        ("generic", "work", false, false),
        ("health", "health", true, false),
        ("finances", "money", false, true),
    ] {
        let raw = std::fs::read_to_string(format!(
            "{}/tests/fixtures/view/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let doc: Value = serde_json::from_str(&raw)?;
        let cap = &doc["capabilities"];
        assert_eq!(cap["contract"], view::CONTRACT, "{name}");
        assert_eq!(cap["contract_version"], json!(view::CONTRACT_VERSION));
        assert_eq!(cap["kind"], kind, "{name}");
        let views: Vec<&str> = cap["views"]
            .as_array()
            .ok_or("views")?
            .iter()
            .filter_map(|v| v["id"].as_str())
            .collect();
        assert_eq!(views, ["targets", "chronology", "stats", "files"], "{name}");
        assert_eq!(cap["home"], "targets", "{name}");
        assert_eq!(cap["item_types"].get("sign").is_some(), health, "{name}");
        assert_eq!(cap["item_types"].get("movement").is_some(), money, "{name}");
        if money {
            let ext = cap["extensions"].as_array().ok_or("extensions")?;
            assert_eq!(ext[0]["id"], "finances", "{name}");
        }
        let reg = &doc["register"];
        assert_eq!(reg["view"], "register", "{name}");
        for item in reg["items"].as_array().ok_or("items")? {
            assert!(
                item["id"].as_str().is_some_and(|s| !s.is_empty()),
                "{name}: {item}"
            );
            assert!(item["type"].as_str().is_some(), "{name}: {item}");
            assert!(item["provenance"].is_object(), "{name}: {item}");
        }
        if health {
            let types: BTreeSet<&str> = reg["items"]
                .as_array()
                .ok_or("items")?
                .iter()
                .filter_map(|i| i["type"].as_str())
                .collect();
            assert!(
                types.contains("sign") && types.contains("measure"),
                "{name}"
            );
        }
        if money {
            let first = reg["items"]
                .as_array()
                .ok_or("items")?
                .first()
                .ok_or("empty")?;
            assert_eq!(first["type"], "movement");
            assert!(first["amount"].is_number(), "{first}");
        }
    }
    Ok(())
}
