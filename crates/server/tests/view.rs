//! The space view contract through the node: the finance extension of a money space, its
//! movements with stable ids and a cursor, and the rights that keep a space whole.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine as _;
use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::{Brain, Space};
use memcp_server::app::App;
use memcp_server::http::Request;
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);
const WORKBOOK: &[u8] = include_bytes!("../../finances/tests/fixtures/statements.xlsx");

fn sp(n: &str) -> Space {
    match Space::new(n) {
        Ok(s) => s,
        Err(e) => panic!("{n}: {e}"),
    }
}

fn temp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-view-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

struct Env {
    dir: PathBuf,
    app: App,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Result<Env, Box<dyn Error>> {
    let dir = temp();
    let brain = Brain::init(
        &dir,
        &[sp("health"), sp("finances"), sp("work")],
        Language::English,
    )?;
    brain.set_space_kind(&sp("health"), "health")?;
    brain.set_space_kind(&sp("finances"), "money")?;
    let app = App::new(brain);
    Ok(Env { dir, app })
}

fn connect(e: &Env, agent: &str, rights: &str, read_only: bool) -> Result<String, Box<dyn Error>> {
    let ctl = e.app.brain.open_control()?;
    let tok = access::connect(&ctl, agent, &Rights::parse(rights)?, None, read_only, false)?;
    Ok(tok)
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        query: Default::default(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            .collect(),
        body: body.as_bytes().to_vec(),
        ip: Some("10.0.0.7".into()),
        via_ingress: false,
        tailnet_login: None,
    }
}

fn address_of(e: &Env, token: &str) -> String {
    let name = e
        .app
        .brain
        .open_control()
        .ok()
        .and_then(|ctl| {
            access::authenticate(&ctl, token, 1_900_000_000)
                .ok()
                .flatten()
        })
        .map_or_else(|| "nobody".to_string(), |c| c.agent);
    format!("/mcp/{name}")
}

fn call(e: &Env, token: &str, name: &str, args: Value) -> Value {
    let auth = format!("Bearer {token}");
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
    e.app
        .handle(&request(
            "POST",
            &address_of(e, token),
            &[("authorization", &auth)],
            &body.to_string(),
        ))
        .body_json()
}

fn text_of(v: &Value) -> Value {
    serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap_or("null"))
        .unwrap_or(Value::Null)
}

fn imported(e: &Env, token: &str) -> Result<Value, Box<dyn Error>> {
    let data = base64::engine::general_purpose::STANDARD.encode(WORKBOOK);
    let kept = text_of(&call(
        e,
        token,
        "file_put",
        json!({ "space": "finances", "name": "accounts.xlsx", "note": "A fictitious bank export.", "data_base64": data }),
    ));
    let id = kept["file"]["id"]
        .as_str()
        .ok_or_else(|| format!("no file id in {kept}"))?;
    Ok(text_of(&call(
        e,
        token,
        "finances_import",
        json!({ "file_id": id }),
    )))
}

#[test]
fn a_money_space_publishes_the_finance_extension_and_pages_its_movements() -> Res {
    let e = env()?;
    let fin = connect(&e, "finance-reader", "finances:rw,health:r", false)?;
    let report = imported(&e, &fin)?;
    assert!(
        report["new_movements"].as_i64().unwrap_or(0) > 0,
        "{report}"
    );

    let cap = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "capabilities" }),
    ));
    assert_eq!(cap["contract"], memcp_core::view::CONTRACT);
    assert_eq!(cap["kind"], "money");
    assert_eq!(cap["sensitive"], true);
    let ext = cap["extensions"].as_array().ok_or("no extension")?;
    assert_eq!(ext[0]["id"], "finances");
    let tools: Vec<&str> = ext[0]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tools.contains(&"finances_movements") && tools.contains(&"finances_balances"));
    assert!(cap["item_types"]["movement"].is_object());

    let stats = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "stats" }),
    ));
    let months = stats["stats"]["finances"]["months"]
        .as_array()
        .ok_or("no monthly totals in the finance stats")?;
    assert_eq!(months.len(), 6, "{stats}");
    assert!(
        months
            .iter()
            .all(|m| m["income"].is_number() && m["outflow"].is_number() && m["net"].is_number()),
        "{stats}"
    );

    let first = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "register", "type": "movement", "limit": 2 }),
    ));
    let items = first["items"].as_array().ok_or("items")?;
    assert_eq!(items.len(), 2, "{first}");
    assert!(items.iter().all(|i| i["type"] == "movement"));
    assert!(
        items
            .iter()
            .all(|i| i["id"].as_str().is_some_and(|s| s.starts_with("mv_")))
    );
    assert!(
        items
            .iter()
            .all(|i| i["amount"].is_number() && i["account"].is_string())
    );
    assert!(items.iter().all(|i| i["provenance"]["kind"] == "statement"));
    // A stable id and a stable page.
    let again = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "register", "type": "movement", "limit": 2 }),
    ));
    assert_eq!(first["items"], again["items"]);

    // Follow the cursor to the end: every movement once.
    let mut seen = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let mut args =
            json!({ "space": "finances", "view": "register", "type": "movement", "limit": 3 });
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = text_of(&call(&e, &fin, "space_view", args));
        for item in page["items"].as_array().ok_or("items")? {
            let id = item["id"].as_str().ok_or("id")?.to_string();
            assert!(!seen.contains(&id), "movement {id} came twice");
            seen.push(id);
        }
        match page["page"]["next_cursor"].as_str() {
            Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
            _ => break,
        }
    }
    assert!(!seen.is_empty(), "no movement came out");
    Ok(())
}

#[test]
fn the_money_chronology_merges_notes_and_movements() -> Res {
    let e = env()?;
    let fin = connect(&e, "finance-reader", "finances:rw,health:r", false)?;
    imported(&e, &fin)?;
    text_of(&call(
        &e,
        &fin,
        "remember",
        json!({ "space": "finances", "text": "2026-09-30, I checked the September statement.", "tags": ["journal"] }),
    ));
    let cap = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "capabilities" }),
    ));
    assert_eq!(cap["home"], "targets");
    let ids: Vec<&str> = cap["views"]
        .as_array()
        .ok_or("views")?
        .iter()
        .filter_map(|v| v["id"].as_str())
        .collect();
    assert_eq!(ids, ["targets", "chronology", "stats", "files"], "{cap}");
    assert!(
        cap["tools"].to_string().contains("finances_movements"),
        "{cap}"
    );

    let mut seen = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let mut args = json!({ "space": "finances", "view": "chronology", "limit": 3 });
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = text_of(&call(&e, &fin, "space_view", args));
        for item in page["items"].as_array().ok_or("items")? {
            let id = item["id"].as_str().ok_or("id")?.to_string();
            assert!(!seen.contains(&id), "{id} came twice");
            seen.push(id);
        }
        match page["page"]["next_cursor"].as_str() {
            Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
            _ => break,
        }
    }
    assert!(
        seen.iter().any(|id| id.starts_with("mv_")),
        "no movement in the chronology"
    );
    assert!(
        seen.iter().any(|id| id.starts_with("f_")),
        "no note in the chronology"
    );
    Ok(())
}

#[test]
fn rights_keep_the_money_space_whole() -> Res {
    let e = env()?;
    let fin = connect(&e, "finance-reader", "finances:rw,health:r", false)?;
    imported(&e, &fin)?;
    let work = connect(&e, "work-reader", "work:r", true)?;
    let denied = call(
        &e,
        &work,
        "space_view",
        json!({ "space": "finances", "view": "register" }),
    );
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    assert!(denied.to_string().contains("no access"), "{denied}");

    let scoped = connect(&e, "money-app", "finances/bank:r", true)?;
    let hidden = call(
        &e,
        &scoped,
        "space_view",
        json!({ "space": "finances", "view": "register" }),
    );
    assert!(
        hidden["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Unknown tool")),
        "a key limited to paths does not see the whole-space view: {hidden}"
    );

    // The reader may not write through the view: it has no write tool in the contract.
    let cap = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "capabilities" }),
    ));
    assert_eq!(cap["writable"], true);
    let stats = text_of(&call(
        &e,
        &fin,
        "space_view",
        json!({ "space": "finances", "view": "stats" }),
    ));
    assert!(
        stats["stats"]["finances"]["accounts"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{stats}"
    );
    Ok(())
}
