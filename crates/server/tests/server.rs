//! The server end to end: MCP over HTTP, device routes, and a real socket.

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use pensine_core::access::{self, Rights};
use pensine_core::guide;
use pensine_core::text::Language;
use pensine_core::{Brain, Space};
use pensine_server::app::App;
use pensine_server::http::{Request, Response};
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);

fn sp(n: &str) -> Space {
    match Space::new(n) {
        Ok(s) => s,
        Err(e) => panic!("{n}: {e}"),
    }
}

fn temp(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("pensine-server-{tag}-{}-{n}", std::process::id()));
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
    let dir = temp("app");
    let brain = Brain::init(
        &dir,
        &[sp("health"), sp("work"), sp("home"), sp("personal")],
        Language::English,
    )?;
    brain.set_space_kind(&sp("health"), "health")?;
    let app = App::new(brain);
    Ok(Env { dir, app })
}

fn connect(e: &Env, agent: &str, rights: &str, read_only: bool) -> Result<String, Box<dyn Error>> {
    let ctl = e.app.brain.open_control()?;
    let r = Rights::parse(rights)?;
    Ok(access::connect(&ctl, agent, &r, None, read_only, false)?)
}

fn rq(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
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

/// The address of the connection a key belongs to: `/mcp/<its name>`. A key that is no key goes to an
/// address nobody has.
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

fn rpc(e: &Env, token: &str, extra: &[(&str, &str)], body: Value) -> Response {
    let auth = format!("Bearer {token}");
    let mut h = vec![("authorization", auth.as_str())];
    h.extend_from_slice(extra);
    e.app
        .handle(&rq("POST", &address_of(e, token), &h, &body.to_string()))
}

fn call(e: &Env, token: &str, name: &str, args: Value) -> Value {
    rpc(
        e,
        token,
        &[],
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } }),
    )
    .body_json()
}

fn text_of(v: &Value) -> Value {
    serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap_or("null"))
        .unwrap_or(Value::Null)
}

#[test]
fn read_before_write_requires_a_recent_read_from_the_same_connection() -> Res {
    let mut e = env()?;
    e.app.strict_write_agents = vec!["writer".into(), "other-writer".into()];
    let token = connect(&e, "writer", "work:rw", false)?;
    let other_token = connect(&e, "other-writer", "work:rw", false)?;
    let ctl = e.app.brain.open_control()?;
    let now = pensine_core::util::now();
    let writer = access::authenticate(&ctl, &token, now)?.ok_or("missing writer")?;
    let other = access::authenticate(&ctl, &other_token, now)?.ok_or("missing other writer")?;
    let call_at = |caller: &access::Caller, name, args, at| {
        e.app.dispatch(
            caller,
            &json!(1),
            "tools/call",
            &json!({ "name": name, "arguments": args }),
            at,
        )
    };
    let note = json!({
        "space": "work", "text": "The observatory opens in spring.", "tags": ["schedule"]
    });
    let (status, refused) = call_at(&writer, "remember", note.clone(), now);
    assert_eq!(status, 200);
    assert_eq!(refused["result"]["isError"], true);
    assert_eq!(
        refused["result"]["content"][0]["text"],
        "Read first. Call `recall` with the topic of what you are about to write, read what it returns, then call this tool again: write only what the memory does not already say, and correct a fact with `supersede`, never with a second note."
    );
    let event: String = ctl.query_row(
        "SELECT detail FROM auth_audit WHERE event = 'write_refused_read_first' AND agent = 'writer'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(event, "remember");

    let (_, read) = call_at(
        &writer,
        "recall",
        json!({ "space": "work", "query": "observatory" }),
        now,
    );
    assert_eq!(read["result"]["isError"], false);
    assert_eq!(text_of(&read)["results"], json!([]));
    assert_eq!(
        call_at(&other, "remember", note.clone(), now).1["result"]["isError"],
        true
    );
    let (_, saved) = call_at(&writer, "remember", note.clone(), now + 30 * 60);
    assert_eq!(saved["result"]["isError"], false);
    assert!(text_of(&saved)["fact"]["id"].is_string());
    let (_, expired) = call_at(&writer, "remember", note, now + 30 * 60 + 1);
    assert_eq!(expired, refused);
    Ok(())
}

#[test]
fn read_before_write_is_off_by_default_and_wildcard_exempts_peers_and_apps() -> Res {
    let mut e = env()?;
    let normal = connect(&e, "writer", "work:rw", false)?;
    let note = |text| json!({ "space": "work", "text": text, "tags": ["test"] });
    let saved = call(
        &e,
        &normal,
        "remember",
        note("The bakery opens before dawn."),
    );
    assert_eq!(saved["result"]["isError"], false);
    e.app.strict_write_agents = vec!["*".into()];
    let refused = call(
        &e,
        &normal,
        "remember",
        note("The bakery opens before dawn."),
    );
    assert_eq!(refused["result"]["isError"], true);
    let ctl = e.app.brain.open_control()?;
    let peer = access::connect_peer(&ctl, "owner", "peer-laptop", &Rights::owner(), false)?;
    let saved = call(&e, &peer, "remember", note("A ferry crosses the bay."));
    assert_eq!(saved["result"]["isError"], false);
    for (name, text) in [
        ("weather-app", "Rain fell overnight."),
        ("app-weather", "Fog delayed the morning flight."),
        ("janitor", "The gardener planted tulips."),
    ] {
        let token = connect(&e, name, "work:rw", false)?;
        assert_eq!(
            call(&e, &token, "remember", note(text))["result"]["isError"],
            false
        );
    }
    Ok(())
}

#[test]
fn read_before_write_exempts_deposits_files_and_inbox_notes() -> Res {
    let mut e = env()?;
    e.app.strict_write_agents = vec!["writer".into()];
    let token = connect(&e, "writer", "work:rw", false)?;
    for (name, args) in [
        (
            "deposit",
            json!({
                "space": "work", "records": [{
                    "text": "A sensor recorded rain.",
                    "origin": { "app": "weather", "id": "rain-1" }
                }]
            }),
        ),
        (
            "file_put",
            json!({
                "space": "work", "name": "sample.txt", "data_base64": "aGVsbG8="
            }),
        ),
        (
            "remember",
            json!({
                "space": "work", "text": "A quick note for later.", "tags": ["inbox"]
            }),
        ),
    ] {
        let reply = call(&e, &token, name, args);
        assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
    }
    let reply = call(
        &e,
        &token,
        "remember",
        json!({
            "space": "work", "text": "The festival opens on Saturday.", "tags": ["schedule"]
        }),
    );
    assert_eq!(reply["result"]["isError"], true);
    Ok(())
}

#[test]
fn space_reads_repeat_the_journal_handoff_without_initializing() -> Res {
    let e = env()?;
    let writer = connect(&e, "health-writer", "health:rw,work:rw", false)?;
    let reader = connect(&e, "health-reader", "health:rw,work:rw", true)?;
    let scoped = connect(&e, "health-app", "health/journal:rw", false)?;
    let work = connect(&e, "work-reader", "work:rw", true)?;
    let saved = text_of(&call(
        &e,
        &writer,
        "remember",
        json!({ "space": "health", "text": "The person mentioned a walk.", "tags": ["journal"] }),
    ));
    assert!(saved.get("assistant_instructions").is_none());
    let id = saved["fact"]["id"]
        .as_str()
        .ok_or("missing saved health fact")?;

    let lookup = json!({ "query": "walk" });
    let unrestricted = text_of(&call(&e, &writer, "recall", lookup.clone()));
    let reminder = unrestricted["assistant_instructions"]
        .as_str()
        .ok_or("missing handoff reminder")?;
    assert!(reminder.contains("`remember`"));
    assert!(reminder.contains("advice actually given"));
    assert!(reminder.contains("Combine several messages on the same topic in one handoff"));
    assert!(reminder.contains("important reasoning"));
    assert!(reminder.contains("`health` (writable)"));
    assert_eq!(unrestricted["notice"], pensine_core::tools::NOTICE);
    assert_eq!(unrestricted["results"][0]["id"], id);

    let empty = text_of(&call(
        &e,
        &writer,
        "recall",
        json!({ "space": "health", "query": "zzzz" }),
    ));
    assert_eq!(empty["results"], json!([]));
    assert_eq!(empty["assistant_instructions"], reminder);
    for name in ["read_fact", "history"] {
        let read = text_of(&call(&e, &writer, name, json!({ "id": id })));
        assert_eq!(read["assistant_instructions"], reminder);
    }
    let read_only = text_of(&call(&e, &reader, "recall", lookup.clone()));
    assert!(
        read_only["assistant_instructions"]
            .as_str()
            .unwrap_or_default()
            .contains("`health` (read-only)")
    );
    let scoped_read = text_of(&call(&e, &scoped, "recall", lookup));
    assert_eq!(scoped_read["results"][0]["id"], id);
    assert!(scoped_read.get("assistant_instructions").is_none());
    let work_read = text_of(&call(
        &e,
        &writer,
        "recall",
        json!({ "space": "work", "query": "walk" }),
    ));
    let work_reminder = work_read["assistant_instructions"]
        .as_str()
        .ok_or("missing work handoff reminder")?;
    assert!(work_reminder.contains("`work` (writable)"));
    assert!(!work_reminder.contains("`health`"));
    let denied = call(
        &e,
        &work,
        "recall",
        json!({ "space": "health", "query": "walk" }),
    );
    assert_eq!(denied["result"]["isError"], true);
    assert!(!denied.to_string().contains("handoff"));

    let named = sp("person-project");
    e.app.brain.add_space(&named)?;
    let named_writer = connect(&e, "named-writer", "person-project:rw", false)?;
    let named_read = text_of(&call(
        &e,
        &named_writer,
        "recall",
        json!({ "query": "zzzz" }),
    ));
    assert!(
        named_read["assistant_instructions"]
            .as_str()
            .unwrap_or_default()
            .contains("`person-project` (writable)")
    );
    Ok(())
}

fn tool_names(e: &Env, token: &str) -> Vec<String> {
    rpc(
        e,
        token,
        &[],
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .body_json()["result"]["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn no_token_is_401_with_www_authenticate() -> Res {
    let e = env()?;
    let r = e.app.handle(&rq("POST", "/mcp/claudeai", &[], "{}"));
    assert_eq!(r.status, 401);
    assert!(
        r.headers
            .iter()
            .any(|(k, v)| k == "WWW-Authenticate" && v.starts_with("Bearer"))
    );
    assert_eq!(
        e.app
            .handle(&rq(
                "POST",
                "/mcp/claudeai",
                &[("authorization", "Bearer nope")],
                "{}"
            ))
            .status,
        401
    );
    Ok(())
}

#[test]
fn get_on_mcp_is_405_and_unknown_paths_404() -> Res {
    let e = env()?;
    assert_eq!(
        e.app.handle(&rq("GET", "/mcp/claudeai", &[], "")).status,
        405
    );
    // There is no address for the whole memory: a connection has its own.
    let r = e.app.handle(&rq("POST", "/mcp", &[], "{}"));
    assert_eq!(r.status, 404);
    assert!(
        r.body_json()["error"]
            .as_str()
            .is_some_and(|m| m.contains("/mcp/"))
    );
    assert_eq!(e.app.handle(&rq("GET", "/nothing", &[], "")).status, 404);
    assert_eq!(e.app.handle(&rq("GET", "/health", &[], "")).status, 200);
    Ok(())
}

#[test]
fn a_foreign_origin_is_refused_a_local_one_accepted() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    assert_eq!(
        rpc(&e, &t, &[("origin", "https://evil.example")], list.clone()).status,
        403
    );
    assert_eq!(
        rpc(&e, &t, &[("origin", "http://localhost:5173")], list).status,
        200
    );
    Ok(())
}

#[test]
fn initialize_for_old_clients_and_discover_for_new_ones() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    let r = rpc(
        &e,
        &t,
        &[],
        json!({ "jsonrpc": "2.0", "id": "x", "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "old", "version": "1" } } }),
    );
    let b = r.body_json();
    assert_eq!(
        (b["id"].clone(), b["result"]["protocolVersion"].clone()),
        (json!("x"), json!("2025-06-18"))
    );
    assert_eq!(b["result"]["serverInfo"]["name"], "memcp");
    assert!(
        b["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.contains("recall"))
    );
    assert!(
        !r.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id")),
        "no session"
    );
    let b = rpc(
        &e,
        &t,
        &[],
        json!({ "jsonrpc": "2.0", "id": 2, "method": "server/discover" }),
    )
    .body_json();
    assert!(
        b["result"]["supportedVersions"]
            .as_array()
            .is_some_and(|v| v.iter().any(|x| x == "2026-07-28"))
    );
    assert_eq!(
        (
            b["result"]["cacheScope"].clone(),
            b["result"]["ttlMs"].clone()
        ),
        (json!("private"), json!(0))
    );
    let b = rpc(&e, &t, &[], json!({ "jsonrpc": "2.0", "id": 3, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } })).body_json();
    assert!(
        b["result"]["protocolVersion"].is_string(),
        "an unknown revision gets a supported one"
    );
    Ok(())
}

#[test]
fn headers_of_revision_2026_07_28() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let v = ("mcp-protocol-version", "2026-07-28");
    assert_eq!(
        rpc(&e, &t, &[v, ("mcp-method", "tools/list")], list.clone()).status,
        200
    );
    for h in [vec![v], vec![v, ("mcp-method", "ping")]] {
        let r = rpc(&e, &t, &h, list.clone());
        assert_eq!(
            (r.status, r.body_json()["error"]["code"].clone()),
            (400, json!(-32020)),
            "{h:?}"
        );
    }
    let c = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "stats", "arguments": {} } });
    let r = rpc(
        &e,
        &t,
        &[v, ("mcp-method", "tools/call"), ("mcp-name", "recall")],
        c.clone(),
    );
    assert_eq!(r.body_json()["error"]["code"], -32020);
    assert_eq!(
        rpc(
            &e,
            &t,
            &[v, ("mcp-method", "tools/call"), ("mcp-name", "stats")],
            c
        )
        .status,
        200
    );
    let r = rpc(&e, &t, &[("mcp-protocol-version", "2030-01-01")], list);
    let b = r.body_json();
    assert_eq!((r.status, b["error"]["code"].clone()), (400, json!(-32022)));
    assert!(
        b["error"]["data"]["supported"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    Ok(())
}

#[test]
fn notification_202_unknown_method_404_bad_json_400() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    let r = rpc(
        &e,
        &t,
        &[],
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert_eq!((r.status, r.body.len()), (202, 0));
    let r = rpc(
        &e,
        &t,
        &[],
        json!({ "jsonrpc": "2.0", "id": 1, "method": "no/such/thing" }),
    );
    assert_eq!(
        (r.status, r.body_json()["error"]["code"].clone()),
        (404, json!(-32601))
    );
    let auth = format!("Bearer {t}");
    let at = address_of(&e, &t);
    let r = e
        .app
        .handle(&rq("POST", &at, &[("authorization", &auth)], "not json"));
    assert_eq!(
        (r.status, r.body_json()["error"]["code"].clone()),
        (400, json!(-32700))
    );
    let r = e
        .app
        .handle(&rq("POST", &at, &[("authorization", &auth)], "{\"id\":1}"));
    assert_eq!(
        (r.status, r.body_json()["error"]["code"].clone()),
        (400, json!(-32600))
    );
    Ok(())
}

#[test]
fn visible_tools_follow_rights_and_read_only() -> Res {
    let e = env()?;
    let owner = tool_names(&e, &connect(&e, "owner", "all", false)?);
    for n in [
        "remember",
        "supersede",
        "retire",
        "recall",
        "stats",
        "file_put",
    ] {
        assert!(
            owner.contains(&n.to_string()),
            "{n} for the owner: {owner:?}"
        );
    }
    // The memory knows no app: nothing of any app's is offered.
    assert!(
        !owner.iter().any(|n| n.starts_with("healthapp")
            || (n.starts_with("health_") && n != "health_referential")),
        "{owner:?}"
    );
    let reader = tool_names(&e, &connect(&e, "reader", "health:rw", true)?);
    assert!(!reader.contains(&"remember".into()) && !reader.contains(&"retire".into()));
    assert!(reader.contains(&"recall".into()));
    Ok(())
}

#[test]
fn write_search_and_replace_through_the_protocol() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    let r = text_of(&call(
        &e,
        &t,
        "remember",
        json!({ "text": "The bakery opens at nine." }),
    ));
    assert_eq!(r["result"], "created");
    let id = r["fact"]["id"].as_str().ok_or("id")?.to_string();
    let r = text_of(&call(
        &e,
        &t,
        "supersede",
        json!({ "id": id, "text": "The bakery opens at ten.", "reason": "winter" }),
    ));
    assert_eq!(r["result"], "superseded");
    let r = text_of(&call(&e, &t, "recall", json!({ "query": "bakery" })));
    assert_eq!(r["results"][0]["text"], "The bakery opens at ten.");
    let r = call(&e, &t, "nonexistent_tool", json!({}));
    assert_eq!(r["error"]["code"], -32602);
    let r = call(&e, &t, "remember", json!({ "text": "" }));
    assert_eq!(r["result"]["isError"], true);
    Ok(())
}

#[test]
fn isolation_between_agents_through_the_protocol() -> Res {
    let e = env()?;
    let a = connect(&e, "health-agent", "health:rw", false)?;
    let b = connect(&e, "home-agent", "home:rw", false)?;
    call(
        &e,
        &a,
        "remember",
        json!({ "text": "Sam's ferritin is low." }),
    );
    let r = text_of(&call(&e, &b, "recall", json!({ "query": "ferritin" })));
    assert_eq!(r["results"].as_array().map(Vec::len), Some(0));
    Ok(())
}

#[test]
fn live_revocation_and_read_only_over_http() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    let ctl = e.app.brain.open_control()?;
    assert!(access::set_read_only(&ctl, "a", true)?);
    assert!(!tool_names(&e, &t).contains(&"remember".into()));
    assert!(access::revoke(&ctl, "a")?);
    assert_eq!(
        rpc(
            &e,
            &t,
            &[],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })
        )
        .status,
        401
    );
    Ok(())
}

#[test]
fn rate_limit_per_connection() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    assert!(access::set_limit(
        &e.app.brain.open_control()?,
        "a",
        "rate",
        3
    )?);
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    for _ in 0..3 {
        assert_eq!(rpc(&e, &t, &[], ping.clone()).status, 200);
    }
    assert_eq!(rpc(&e, &t, &[], ping).status, 429);
    Ok(())
}

#[test]
fn the_default_ceiling_is_an_anti_flood_one_that_no_feed_meets() -> Res {
    let e = env()?;
    let t = connect(&e, "feed", "personal:rw", false)?;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    // Four hundred calls in a burst: a feed sending one record per call stays far below the ceiling.
    for _ in 0..400 {
        assert_eq!(rpc(&e, &t, &[], ping.clone()).status, 200);
    }
    assert_eq!(pensine_core::access::DEFAULT_RATE_PER_MIN, 6000);
    Ok(())
}

#[test]
fn too_many_refusals_from_one_address_block_even_the_right_token() -> Res {
    let e = env()?;
    let t = connect(&e, "a", "personal:rw", false)?;
    for _ in 0..10 {
        e.app.handle(&rq(
            "POST",
            "/mcp/a",
            &[("authorization", "Bearer wrong")],
            "{}",
        ));
    }
    assert_eq!(
        rpc(
            &e,
            &t,
            &[],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })
        )
        .status,
        429
    );
    Ok(())
}

#[test]
fn the_bank_statement_tools_follow_the_money_vault_and_its_rights() -> Res {
    let dir = temp("finances");
    let brain = Brain::init(&dir, &[sp("finances"), sp("home")], Language::English)?;
    let e = Env {
        dir,
        app: App::new(brain),
    };
    // No vault of the money kind (the space is made custom here): the tools do not exist, whatever the rights.
    e.app.brain.set_space_kind(&sp("finances"), "custom")?;
    let all = connect(&e, "owner", "all", false)?;
    assert!(
        !tool_names(&e, &all)
            .iter()
            .any(|n| n.starts_with("finances_"))
    );
    e.app.brain.set_space_kind(&sp("finances"), "money")?;
    let writer = connect(&e, "writer", "finances:rw", false)?;
    let reader = connect(&e, "reader", "finances:r", false)?;
    let other = connect(&e, "other", "home:rw", false)?;
    let has = |t: &str, n: &str| tool_names(&e, t).contains(&n.to_string());
    assert!(has(&writer, "finances_import") && has(&writer, "finances_balances"));
    assert!(has(&reader, "finances_monthly") && !has(&reader, "finances_import"));
    assert!(
        !tool_names(&e, &other)
            .iter()
            .any(|n| n.starts_with("finances_"))
    );
    // A call goes through the protocol and answers on an empty ledger.
    let b = text_of(&call(&e, &reader, "finances_balances", json!({})));
    assert_eq!(b["accounts"], json!([]));
    let r = text_of(&call(
        &e,
        &writer,
        "finances_rules",
        json!({ "rules": { "fees": ["agios"] } }),
    ));
    assert_eq!(r["rules"]["fees"], json!(["AGIOS"]));
    // A key limited to a path of the money vault (its inbox) gets no statement tool, and a call is refused.
    let inbox = connect(&e, "inbox", "finances/inbox:rw", false)?;
    assert!(
        !tool_names(&e, &inbox)
            .iter()
            .any(|n| n.starts_with("finances_")),
        "{:?}",
        tool_names(&e, &inbox)
    );
    let refused = call(&e, &inbox, "finances_movements", json!({}));
    assert!(
        refused.to_string().contains("isError\":true")
            || refused.get("error").is_some()
            || refused["result"]["isError"] == true,
        "{refused}"
    );
    Ok(())
}

// --- the binary on a real socket -----------------------------------------------------------

/// Starts `memcp serve` on port 0 and returns the child and the port it was given (read from its first line on the
/// standard error), so no port is chosen here and left free for another program to take.
fn serve_on_a_free_port(
    bin: &str,
    dir: &std::path::Path,
    extra: &[&str],
) -> Result<(std::process::Child, u16), Box<dyn Error>> {
    serve_with_env(bin, dir, extra, &[])
}

fn serve_with_env(
    bin: &str,
    dir: &std::path::Path,
    extra: &[&str],
    env: &[(&str, &str)],
) -> Result<(std::process::Child, u16), Box<dyn Error>> {
    use std::io::{BufRead, BufReader};
    let mut child = std::process::Command::new(bin)
        .arg("--data")
        .arg(dir)
        .args(["serve", "--listen", "127.0.0.1:0"])
        .args(extra)
        .envs(env.iter().copied())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut reader = BufReader::new(child.stderr.take().ok_or("no stderr")?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    // Keep reading: a closed pipe would make the server's next message fail.
    std::thread::spawn(move || for _ in reader.lines() {});
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| format!("no port in {line:?}"))?;
    Ok((child, port))
}

struct Child(std::process::Child, PathBuf);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = fs::remove_dir_all(&self.1);
    }
}

fn http(port: u16, raw: &str) -> Result<(String, String), Box<dyn Error>> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.write_all(raw.as_bytes())?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    let (h, b) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    Ok((h.to_string(), b.to_string()))
}

#[test]
fn a_second_server_refuses_the_same_data_directory() -> Res {
    let dir = temp("node-instance");
    Brain::init(&dir, &[], pensine_core::text::Language::English)?;
    let bin = env!("CARGO_BIN_EXE_memcp");
    let (first, port) = serve_on_a_free_port(bin, &dir, &[])?;
    let _first = Child(first, dir.clone());
    // Use the first server's address so a regression still exits on bind, with a different error.
    let second = std::process::Command::new(bin)
        .arg("--data")
        .arg(&dir)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .output()?;
    let error = String::from_utf8_lossy(&second.stderr);
    assert!(!second.status.success());
    assert!(error.contains("A node is already running"), "{error}");
    Ok(())
}

#[test]
fn the_real_server_on_a_real_socket() -> Res {
    let dir = temp("socket");
    let bin = env!("CARGO_BIN_EXE_memcp");
    let run = |args: &[&str]| -> Result<String, Box<dyn Error>> {
        let o = std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(args)
            .output()?;
        if !o.status.success() {
            return Err(format!("{args:?}: {}", String::from_utf8_lossy(&o.stderr)).into());
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    };
    run(&["init", "--spaces", "personal"])?;
    run(&["space", "add", "household"])?;
    assert_eq!(run(&["space", "list"])?, "household\npersonal\n");
    let out = run(&[
        "connect",
        "test-socket",
        "--grant",
        "personal:rw",
        "--public-url",
        "https://example.ts.net",
    ])?;
    let token = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("hcp_"))
        .ok_or("token not shown")?
        .to_string();
    assert!(out.contains(
        "claude mcp add --transport http test-socket https://example.ts.net/mcp/test-socket"
    ));
    assert!(
        run(&["connect", "x", "--grant", "garden:rw"]).is_err(),
        "unknown space"
    );

    let (child, port) = serve_on_a_free_port(bin, &dir, &[])?;
    let srv = Child(child, dir.clone());
    let (head, body) = http(
        port,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )?;
    assert!(
        head.starts_with("HTTP/1.1 200") && body.contains("\"ok\":true"),
        "{head}"
    );
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "remember", "arguments": { "text": "A fact written over a real socket." } } }).to_string();
    let req = format!(
        "POST /mcp/test-socket HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let (head, body) = http(port, &req)?;
    assert!(
        head.starts_with("HTTP/1.1 200") && body.contains("created"),
        "{head}\n{body}"
    );
    let (head, _) = http(
        port,
        "POST /mcp/test-socket HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    )?;
    assert!(
        head.starts_with("HTTP/1.1 401")
            && head.to_lowercase().contains("www-authenticate: bearer")
    );

    let n: i64 = Brain::open(&srv.1)?
        .open_space(&sp("personal"))?
        .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    assert_eq!(n, 1, "the fact is on disk");
    let backups = temp("socket-backups");
    let out = run(&["backup", "--to", backups.to_str().ok_or("path")?])?;
    assert!(out.starts_with("backup: "), "{out}");
    let _ = fs::remove_dir_all(&backups);
    Ok(())
}

#[test]
fn refuses_to_listen_on_a_non_local_address_without_consent() -> Res {
    let dir = temp("remote");
    let bin = env!("CARGO_BIN_EXE_memcp");
    assert!(
        std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .arg("init")
            .output()?
            .status
            .success()
    );
    let o = std::process::Command::new(bin)
        .arg("--data")
        .arg(&dir)
        .args(["serve", "--listen", "0.0.0.0:0"])
        .output()?;
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("in clear"));
    Ok(())
}

#[test]
fn a_kept_photo_comes_back_as_an_image() -> Res {
    let e = env()?;
    let t = connect(&e, "home-agent", "home:rw", false)?;
    // A 1×1 PNG header is enough for the type check.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let put = text_of(&call(
        &e,
        &t,
        "file_put",
        json!({ "name": "fuse-box.png", "note": "Fuse box", "data_base64": png }),
    ));
    let id = put["file"]["id"].as_str().ok_or("id")?.to_string();
    let r = call(&e, &t, "file_get", json!({ "id": id }));
    let content = r["result"]["content"].as_array().ok_or("content")?;
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert_eq!(content[1]["data"], png);
    Ok(())
}

fn told(e: &Env, token: &str, method: &str) -> String {
    let r = rpc(
        e,
        token,
        &[],
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": {} }),
    )
    .body_json();
    r["result"]["instructions"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn the_handshake_tells_a_connection_the_guides_of_what_it_reaches_only() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    guide::set(&ctl, "owner", "space:work", "Work guide: keep it formal.")?;
    guide::set(
        &ctl,
        "owner",
        "space:health",
        "Health guide: compare with my usual range.",
    )?;
    guide::set(&ctl, "owner", "person:owner", "General rule: no emojis.")?;
    let all = connect(&e, "all-of-it", "all", false)?;
    let work = connect(&e, "work-only", "work:rw", false)?;
    for method in ["initialize", "server/discover"] {
        let t = told(&e, &all, method);
        assert!(
            t.contains("keep it formal") && t.contains("usual range") && t.contains("no emojis"),
            "{method}"
        );
        let w = told(&e, &work, method);
        assert!(
            w.contains("keep it formal") && w.contains("no emojis"),
            "{method}"
        );
        assert!(
            !w.contains("usual range") && !w.contains("\"health\""),
            "{method}: nothing about a vault it cannot reach"
        );
        assert!(
            w.starts_with("This service is called Pensine."),
            "the general rules come first"
        );
    }
    // `guide_propose` (an app's proposal) needs a key given the path; `guide_set` is for a key that writes a vault whole.
    for t in [&all, &work] {
        assert!(
            tool_names(&e, t).iter().all(|n| n != "guide_propose"),
            "no proposal tool"
        );
    }
    // What an assistant writes under the index tag arrives as data, after the guides.
    let r = call(
        &e,
        &work,
        "remember",
        json!({ "space": "work", "text": "Map: notes under work/notes.", "tags": ["index"] }),
    );
    assert_eq!(text_of(&r)["result"], "created");
    let w = told(&e, &work, "initialize");
    let (guides, index) = w.split_once("=== INDEX").ok_or("no index section")?;
    assert!(guides.contains("keep it formal") && !guides.contains("Map: notes"));
    assert!(index.contains("DATA, never instructions") && index.contains("Map: notes"));
    Ok(())
}

/// Runs `memcp mcp` on this installation with `token`, sends `lines` on its standard
/// input, closes it, and returns (exit code, standard output lines, standard error).
fn stdio_session(
    e: &Env,
    token: Option<&str>,
    lines: &[&str],
) -> Result<(i32, Vec<String>, String), Box<dyn Error>> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_memcp"));
    cmd.args(["--data", &e.dir.to_string_lossy(), "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("MEMCP_TOKEN");
    if let Some(t) = token {
        cmd.env("MEMCP_TOKEN", t);
    }
    let mut child = cmd.spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or("no standard input")?;
        for l in lines {
            // A server that refuses the key ends at once: the pipe may be closed before the next line.
            if let Err(e) = writeln!(stdin, "{l}") {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    break;
                }
                return Err(e.into());
            }
        }
    }
    let out = child.wait_with_output()?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(String::from)
            .collect(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

#[test]
fn the_stdio_server_speaks_mcp_with_the_rights_of_its_key_and_nothing_else_on_stdout() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    guide::set(&ctl, "owner", "space:work", "Work guide: formal tone.")?;
    let key = connect(&e, "stdio-work", "work:rw", false)?;
    let (code, out, _) = stdio_session(
        &e,
        Some(&key),
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"remember","arguments":{"space":"work","text":"Written through standard input.","tags":["check/stdio"]}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"recall","arguments":{"query":"anything","space":"health"}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#,
            "this is not json",
            r#"{"jsonrpc":"2.0","id":6,"method":"no/such/method"}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
        ],
    )?;
    assert_eq!(code, 0, "it ends cleanly when its input ends");
    // Every line of standard output is a protocol message: nothing else, ever.
    let msgs: Vec<Value> = out
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|_| panic!("not protocol: {l:?}")))
        .collect();
    assert!(msgs.iter().all(|m| m["jsonrpc"] == "2.0"));
    let by_id = |id: i64| {
        msgs.iter()
            .find(|m| m["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    };
    // The notification, the blank line and the client's own answer got no reply: 7 messages.
    assert_eq!(msgs.len(), 7, "{out:?}");
    let init = by_id(1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let told = init["result"]["instructions"].as_str().unwrap_or("");
    assert!(
        told.contains("Work guide: formal tone."),
        "the guide arrives through standard input too"
    );
    let names: Vec<String> = by_id(2)["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect();
    assert!(
        names.contains(&"remember".to_string())
            && names
                .iter()
                .all(|n| (!n.starts_with("health_") || n == "health_referential")
                    && !n.starts_with("healthapp_"))
    );
    assert_eq!(text_of(&by_id(3))["result"], "created");
    assert_eq!(
        by_id(4)["result"]["isError"],
        true,
        "the vault it was not opened on stays closed"
    );
    assert_eq!(by_id(5)["result"], json!({}));
    assert_eq!(
        msgs.iter()
            .find(|m| m["id"].is_null())
            .map(|m| m["error"]["code"].clone()),
        Some(json!(-32700))
    );
    assert_eq!(by_id(6)["error"]["code"], -32601);
    // What it wrote is the same memory the HTTP side reads.
    let http = connect(&e, "http-work", "work:r", false)?;
    let r = text_of(&call(
        &e,
        &http,
        "recall",
        json!({ "query": "standard input", "tags": ["check/stdio"] }),
    ));
    assert_eq!(r["results"][0]["text"], "Written through standard input.");
    Ok(())
}

#[test]
fn the_stdio_server_stops_when_its_key_is_revoked_and_refuses_to_start_without_one() -> Res {
    let e = env()?;
    let key = connect(&e, "stdio-key", "work:rw", false)?;
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let (code, out, _) = stdio_session(&e, Some(&key), &[ping])?;
    assert_eq!((code, out.len()), (0, 1));
    // Revoked: the next request is answered with an error and the server ends, non-zero.
    let ctl = e.app.brain.open_control()?;
    assert!(access::revoke(&ctl, "stdio-key")?);
    let (code, out, err) = stdio_session(&e, Some(&key), &[ping, ping])?;
    assert_ne!(code, 0);
    assert_eq!(out.len(), 1, "one error, then it stops: {out:?}");
    let m: Value = serde_json::from_str(&out[0])?;
    assert_eq!(m["error"]["code"], -32001);
    assert!(err.contains("refused or revoked"));
    // A key that never existed, an MCP-less console key, and no key at all.
    let (code, out, _) = stdio_session(&e, Some("hcp_00000000_nope"), &[ping])?;
    assert_ne!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out[0])?["error"]["code"],
        -32001
    );
    let (code, out, err) = stdio_session(&e, None, &[ping])?;
    assert_eq!(code, 2);
    assert!(out.is_empty(), "nothing on standard output: {out:?}");
    assert!(err.contains("MEMCP_TOKEN"), "{err}");
    // The refusals are in the log of the installation.
    let refused: i64 = ctl.query_row(
        "SELECT count(*) FROM auth_audit WHERE event = 'mcp_refused'",
        [],
        |r| r.get(0),
    )?;
    assert!(refused >= 2);
    Ok(())
}

fn rpc_at(e: &Env, path: &str, token: &str, body: Value) -> Response {
    let auth = format!("Bearer {token}");
    e.app.handle(&rq(
        "POST",
        path,
        &[("authorization", auth.as_str())],
        &body.to_string(),
    ))
}

#[test]
fn a_key_used_at_the_address_of_a_vault_reaches_that_vault_and_no_other() -> Res {
    let e = env()?;
    let ctl = e.app.brain.open_control()?;
    guide::set(&ctl, "owner", "space:work", "Work guide: formal tone.")?;
    guide::set(&ctl, "owner", "space:health", "Health guide: usual range.")?;
    let all = connect(&e, "everything", "all", false)?;
    let stats = |path: &str| -> Value {
        let r = rpc_at(
            &e,
            path,
            &all,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "stats", "arguments": {} } }),
        );
        text_of(&r.body_json())
    };
    // At its own address a key with every right sees every vault...
    assert!(
        stats("/mcp/everything")
            .as_object()
            .is_some_and(|m| m.contains_key("health") && m.contains_key("work"))
    );
    // ...at the address of a vault it sees that vault, and its instructions, and nothing else.
    let only_work = stats("/mcp/work");
    assert_eq!(
        only_work
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["work".to_string()])
    );
    let init = rpc_at(
        &e,
        "/mcp/work",
        &all,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {} }),
    )
    .body_json();
    let told = init["result"]["instructions"].as_str().unwrap_or("");
    assert!(
        told.contains("Work guide: formal tone.")
            && !told.contains("usual range")
            && !told.contains("health")
    );
    let names: Vec<String> = rpc_at(
        &e,
        "/mcp/work",
        &all,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .body_json()["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect();
    assert!(
        names
            .iter()
            .all(|n| (!n.starts_with("health_") || n == "health_referential")
                && !n.starts_with("healthapp_")),
        "no health tool at the address of the work vault"
    );
    let denied = rpc_at(&e, "/mcp/work", &all, json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "recall", "arguments": { "query": "x", "space": "health" } } })).body_json();
    assert_eq!(denied["result"]["isError"], true);
    // A key that does not reach the vault of the address is told so, whether or not it exists.
    let narrow = connect(&e, "narrow", "work:rw", false)?;
    let r = rpc_at(
        &e,
        "/mcp/health",
        &narrow,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
    );
    assert_eq!(r.status, 403);
    assert_eq!(
        r.body_json()["error"],
        "this key does not reach this address"
    );
    // An address that is no vault's names a connection: only its own key is let in.
    let r = rpc_at(
        &e,
        "/mcp/ghost",
        &narrow,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
    );
    assert_eq!(r.status, 403);
    assert_eq!(
        r.body_json()["error"],
        "this key does not reach this address"
    );
    assert_eq!(
        rpc_at(
            &e,
            "/mcp/work",
            &narrow,
            json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" })
        )
        .status,
        200
    );
    // Not a name: not found. No key: the refusal points at the metadata of that very address.
    assert_eq!(rpc_at(&e, "/mcp/Bad%20Name", &all, json!({})).status, 404);
    let r = e.app.handle(&rq("POST", "/mcp/work", &[], "{}"));
    assert_eq!(r.status, 401);
    let www = r
        .headers
        .iter()
        .find(|(k, _)| k == "WWW-Authenticate")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        www.contains("/.well-known/oauth-protected-resource/mcp/work\""),
        "{www}"
    );
    Ok(())
}

#[test]
fn the_metadata_of_an_address_that_names_a_vault_says_that_resource() -> Res {
    let mut e = env()?;
    e.app.public_url = "https://memory.example.net".into();
    let get = |p: &str| e.app.handle(&rq("GET", p, &[], ""));
    let r = get("/.well-known/oauth-protected-resource/mcp/work");
    assert_eq!(r.status, 200);
    assert_eq!(
        r.body_json()["resource"],
        "https://memory.example.net/mcp/work"
    );
    assert_eq!(
        r.body_json()["authorization_servers"],
        json!(["https://memory.example.net"])
    );
    // No metadata for a general address: there is none.
    assert_eq!(get("/.well-known/oauth-protected-resource").status, 404);
    assert_eq!(get("/.well-known/oauth-protected-resource/mcp").status, 404);
    assert_eq!(
        get("/.well-known/oauth-protected-resource/mcp/Bad%20Name").status,
        404
    );
    Ok(())
}

#[test]
fn an_app_deposits_under_its_own_ids_and_gets_its_cursor_back() -> Res {
    let e = env()?;
    let t = connect(&e, "bevel-sync", "health:rw", false)?;
    let send = |id: &str, at: &str, text: &str| {
        text_of(&call(
            &e,
            &t,
            "remember",
            json!({ "text": text, "origin": { "app": "bevel", "id": id, "at": at } }),
        ))
    };
    let a = send(
        "2026-11-01/steps",
        "2026-11-01",
        "Steps 2026-11-01: 4200 so far",
    );
    assert_eq!(a["result"], "created");
    assert_eq!(a["cursor"]["last_at"], "2026-11-01");
    assert!(
        a["fact"]["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x == "sources/bevel")),
        "filed under its source: {a}"
    );
    let again = send(
        "2026-11-01/steps",
        "2026-11-01",
        "Steps 2026-11-01: 4200 so far",
    );
    assert_eq!(again["result"], "unchanged");
    let up = send("2026-11-01/steps", "2026-11-01", "Steps 2026-11-01: 9100");
    assert_eq!(up["result"], "updated");
    assert_eq!(up["cursor"]["facts"], 1);
    let stats = text_of(&call(&e, &t, "stats", json!({})));
    assert_eq!(stats["health"]["sources"]["bevel"]["facts"], 1, "{stats}");
    // A malformed origin is refused, not half applied.
    let bad = call(
        &e,
        &t,
        "remember",
        json!({ "text": "x", "origin": { "app": "Bad App", "id": "1" } }),
    );
    assert_eq!(bad["result"]["isError"], true, "{bad}");
    Ok(())
}

// --- doctor ---------------------------------------------------------------------------------

#[test]
fn doctor_reads_an_installation_and_changes_nothing() -> Res {
    use pensine_server::doctor;
    let e = env()?;
    let dir = e.app.brain.dir().to_path_buf();
    let t = connect(&e, "wide-key", "all", false)?;
    let _ = t;
    let before: Vec<(String, Vec<u8>)> = ["control.sqlite"]
        .iter()
        .map(|f| Ok((f.to_string(), std::fs::read(dir.join(f))?)))
        .collect::<Result<_, Box<dyn Error>>>()?;
    let r = doctor::inspect(&dir, Some("http://192.168.1.10:8791/"), Some("127.0.0.1:1"));
    let text = r.render();
    for want in [
        "control database: version",
        "the public address ends with a slash",
        "the public address is not HTTPS",
        "only works on your own network",
        "connection wide-key reaches every space",
        "nothing answers on 127.0.0.1:1",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert_eq!(r.problems(), 0, "{text}");
    let after = std::fs::read(dir.join("control.sqlite"))?;
    assert_eq!(before[0].1, after, "the control database was not touched");
    Ok(())
}

#[test]
fn doctor_fails_on_a_missing_vault_file_and_on_a_data_directory_that_is_not_there() -> Res {
    use pensine_server::doctor;
    let e = env()?;
    let dir = e.app.brain.dir().to_path_buf();
    let first = e.app.brain.spaces()?.into_iter().next().ok_or("no vault")?;
    std::fs::remove_file(dir.join(format!("{}.sqlite", first.as_str())))?;
    let r = doctor::inspect(&dir, None, None);
    assert!(r.problems() >= 1, "{}", r.render());
    assert!(r.render().contains("its file is missing"), "{}", r.render());
    let gone = doctor::inspect(&dir.join("nowhere"), None, None);
    assert_eq!(gone.problems(), 1);
    Ok(())
}

#[test]
fn the_address_of_a_connection_is_its_name_and_lets_in_only_its_own_key() -> Res {
    let e = env()?;
    let mine = connect(&e, "claudeai", "work:rw", false)?;
    let other = connect(&e, "healthapp", "health:rw", false)?;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    // Its own address and the address of a vault it reaches take it; the bare /mcp is nobody's.
    for path in ["/mcp/claudeai", "/mcp/work"] {
        assert_eq!(rpc_at(&e, path, &mine, ping.clone()).status, 200, "{path}");
    }
    assert_eq!(rpc_at(&e, "/mcp", &mine, ping.clone()).status, 404);
    // Another connection's address does not, nor an address that is nobody's.
    for path in ["/mcp/healthapp", "/mcp/nobody"] {
        let r = rpc_at(&e, path, &mine, ping.clone());
        assert_eq!(r.status, 403, "{path}");
        assert_eq!(
            r.body_json()["error"],
            "this key does not reach this address"
        );
    }
    assert_eq!(
        rpc_at(&e, "/mcp/healthapp", &other, ping.clone()).status,
        200
    );
    // A key that is not one is sent to discover the address it was given, not the general one.
    let r = rpc_at(&e, "/mcp/claudeai", "wrong", ping);
    assert_eq!(r.status, 401);
    let challenge = r
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("www-authenticate"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        challenge.contains("oauth-protected-resource/mcp/claudeai"),
        "{challenge}"
    );
    let meta = e.app.handle(&rq(
        "GET",
        "/.well-known/oauth-protected-resource/mcp/claudeai",
        &[],
        "",
    ));
    let resource = meta.body_json()["resource"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(resource.ends_with("/mcp/claudeai"), "{resource}");
    Ok(())
}

#[test]
fn a_slow_or_keyless_big_body_holds_no_worker() -> Res {
    let dir = temp("slow");
    let bin = env!("CARGO_BIN_EXE_memcp");
    assert!(
        std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(["init", "--spaces", "personal"])
            .output()?
            .status
            .success()
    );
    let (child, port) = serve_on_a_free_port(bin, &dir, &["--threads", "1"])?;
    let _srv = Child(child, dir.clone());
    // A big body with no key: refused from its declared size, before it is sent.
    let (head, _) = http(
        port,
        "POST /mcp/x HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 5000000\r\nConnection: close\r\n\r\n",
    )?;
    assert!(head.starts_with("HTTP/1.1 413"), "{head}");
    // Three clients that announce a body and send nothing: the one worker still answers.
    let mut slow = vec![];
    for _ in 0..3 {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(b"POST /oauth/token HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\nab")?;
        slow.push(s);
    }
    std::thread::sleep(Duration::from_millis(300));
    let started = std::time::Instant::now();
    let (head, _) = http(
        port,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    drop(slow);
    Ok(())
}

#[test]
fn a_refused_body_holds_no_worker_and_no_memory() -> Res {
    let dir = temp("refused");
    let bin = env!("CARGO_BIN_EXE_memcp");
    assert!(
        std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(["init", "--spaces", "personal"])
            .output()?
            .status
            .success()
    );
    let (child, port) = serve_on_a_free_port(bin, &dir, &["--threads", "1"])?;
    let mut srv = Child(child, dir.clone());
    // A body declared bigger than any memory: refused, and the node is still there (it used to ask the allocator for
    // the declared size while throwing the unread body away, and abort).
    let (head, _) = http(
        port,
        "POST /mcp/x HTTP/1.1\r\nHost: x\r\nContent-Length: 999999999999999\r\nConnection: close\r\n\r\nab",
    )?;
    assert!(head.starts_with("HTTP/1.1 413"), "{head}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(srv.0.try_wait()?.is_none(), "the node stopped");
    // Clients refused for their declared size that then send nothing: the one worker still answers.
    let mut held = vec![];
    for _ in 0..3 {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(b"POST /mcp/x HTTP/1.1\r\nHost: x\r\nContent-Length: 5000000\r\n\r\nab")?;
        held.push(s);
    }
    std::thread::sleep(Duration::from_millis(300));
    let started = std::time::Instant::now();
    let (head, _) = http(
        port,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    drop(held);
    Ok(())
}

#[test]
fn a_burst_of_open_connections_leaves_none_unread() -> Res {
    let dir = temp("burst");
    let bin = env!("CARGO_BIN_EXE_memcp");
    assert!(
        std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(["init", "--spaces", "personal"])
            .output()?
            .status
            .success()
    );
    let (child, port) = serve_on_a_free_port(bin, &dir, &[])?;
    let _srv = Child(child, dir.clone());
    // Many connections opened at once and kept open, each starting a request it does not finish (a keep-alive client,
    // a slow body). Each is read by a thread of its own: a request on a connection opened in the same burst is answered.
    let mut idle = vec![];
    for _ in 0..48 {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")?;
        idle.push(s);
    }
    let mut last = TcpStream::connect(("127.0.0.1", port))?;
    last.set_read_timeout(Some(Duration::from_secs(5)))?;
    last.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")?;
    let mut out = String::new();
    last.read_to_string(&mut out)?;
    assert!(out.starts_with("HTTP/1.1 200"), "{out:?}");
    drop(idle);
    Ok(())
}

#[test]
fn the_key_can_ride_in_the_address_so_one_url_is_enough() -> Res {
    let e = env()?;
    let token = connect(&e, "one-url", "health:rw", false)?;
    let mut r = rq(
        "POST",
        &address_of(&e, &token),
        &[],
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
    );
    r.query.insert("key".into(), token.clone());
    assert_eq!(
        e.app.handle(&r).status,
        200,
        "the key in the address opens it"
    );
    r.query.insert("key".into(), "wrong".into());
    assert_eq!(e.app.handle(&r).status, 401, "a wrong key does not");
    Ok(())
}

/// A request to `tools/call` with the key given as the test says: in the header, in the address, in both.
fn keyed(
    e: &Env,
    token: &str,
    header: Option<&str>,
    query: Option<&str>,
    tool: &str,
    args: Value,
) -> Response {
    let auth = header.map(|h| format!("Bearer {h}"));
    let hs: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } });
    let mut r = rq("POST", &address_of(e, token), &hs, &body.to_string());
    if let Some(q) = query {
        r.query.insert("key".into(), q.into());
    }
    e.app.handle(&r)
}

#[test]
fn the_key_in_the_address_and_the_key_in_the_header_are_the_same_key() -> Res {
    let e = env()?;
    let token = connect(&e, "both-ways", "work:rw", false)?;
    let args = json!({"space": "work", "text": "Written by the address key."});
    // Header only, address only, both: the same answer.
    let by_header = keyed(
        &e,
        &token,
        Some(&token),
        None,
        "recall",
        json!({"query": "x", "space": "work"}),
    );
    let by_query = keyed(
        &e,
        &token,
        None,
        Some(&token),
        "recall",
        json!({"query": "x", "space": "work"}),
    );
    let by_both = keyed(
        &e,
        &token,
        Some(&token),
        Some(&token),
        "recall",
        json!({"query": "x", "space": "work"}),
    );
    for r in [&by_header, &by_query, &by_both] {
        assert_eq!(r.status, 200);
        assert_eq!(
            r.body_json()["result"]["content"][0]["text"],
            by_header.body_json()["result"]["content"][0]["text"]
        );
    }
    // A write by the address key is attributed to the same connection.
    let w = keyed(&e, &token, None, Some(&token), "remember", args);
    assert_eq!(
        text_of(&w.body_json())["fact"]["author"],
        "both-ways",
        "{:?}",
        w.body_json()
    );
    // Whitespace around the key in the address does not matter (a pasted address often carries it).
    assert_eq!(
        keyed(
            &e,
            &token,
            None,
            Some(&format!(" {token} ")),
            "recall",
            json!({"query": "x", "space": "work"})
        )
        .status,
        200
    );
    Ok(())
}

#[test]
fn a_wrong_key_is_refused_wherever_it_is_and_the_right_one_does_not_save_it() -> Res {
    let e = env()?;
    let token = connect(&e, "strict", "work:rw", false)?;
    let other = connect(&e, "stranger", "health:rw", false)?;
    let q = json!({"query": "x", "space": "work"});
    let cases: [(&str, Option<&str>, Option<&str>); 6] = [
        ("wrong in the header", Some("hcp_wrong"), None),
        ("wrong in the address", None, Some("hcp_wrong")),
        ("none at all", None, None),
        ("empty in the address", None, Some("")),
        // The header is read first: a wrong one is not rescued by a right key in the address.
        (
            "wrong header, right address",
            Some("hcp_wrong"),
            Some(token.as_str()),
        ),
        // Another connection's key does not open this address.
        ("another key", None, Some(other.as_str())),
    ];
    for (what, header, query) in cases {
        let r = keyed(&e, &token, header, query, "recall", q.clone());
        assert!(matches!(r.status, 401 | 403), "{what}: {}", r.status);
        assert!(r.body_json().get("result").is_none(), "{what}: no result");
    }
    // A right header with a wrong address key is served: the header is the key.
    assert_eq!(
        keyed(&e, &token, Some(&token), Some("hcp_wrong"), "recall", q).status,
        200
    );
    // Revoked: the key in the address no longer opens anything.
    let ctl = e.app.brain.open_control()?;
    access::revoke(&ctl, "strict")?;
    let r = keyed(
        &e,
        &token,
        None,
        Some(&token),
        "recall",
        json!({"query": "x", "space": "work"}),
    );
    assert_eq!(r.status, 401);
    Ok(())
}

#[test]
fn no_key_is_written_in_the_logs_the_audit_or_the_answers() -> Res {
    let e = env()?;
    let token = connect(&e, "quiet", "work:rw", false)?;
    let secret_try = "hcp_SECRETMARK_0123456789abcdef";
    let mut answers = vec![];
    for (h, q) in [
        (Some(token.as_str()), None),
        (None, Some(token.as_str())),
        (None, Some(secret_try)),
        (Some(secret_try), None),
    ] {
        let r = keyed(
            &e,
            &token,
            h,
            q,
            "remember",
            json!({"space": "work", "text": "A note about the weather today."}),
        );
        answers.push(r);
    }
    answers.push(keyed(
        &e,
        &token,
        None,
        Some(&token),
        "recall",
        json!({"query": "weather", "space": "work"}),
    ));
    for r in &answers {
        let body = String::from_utf8_lossy(&r.body).to_string();
        let heads = format!("{:?}", r.headers);
        for key in [token.as_str(), secret_try] {
            assert!(
                !body.contains(key) && !heads.contains(key),
                "a key is echoed: {body} {heads}"
            );
        }
    }
    // Neither the keys that opened the door nor the one that did not are kept anywhere in the data directory (the
    // control database and the audit of the spaces hold hashes and names only).
    drop(e.app.brain.open_control()?);
    let mut checked = 0;
    for entry in fs::read_dir(&e.dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        for key in [token.as_str(), secret_try] {
            assert!(
                !bytes.windows(key.len()).any(|w| w == key.as_bytes()),
                "{} holds a key",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(checked >= 5, "the databases were looked at ({checked})");
    Ok(())
}

#[test]
fn bodies_that_never_come_without_a_key_do_not_turn_away_a_keyed_assistant() -> Res {
    let dir = temp("anon-bodies");
    let bin = env!("CARGO_BIN_EXE_memcp");
    let run = |args: &[&str]| -> Result<String, Box<dyn Error>> {
        let o = std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(args)
            .output()?;
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    };
    run(&["init", "--spaces", "personal"])?;
    let out = run(&["connect", "keyed", "--grant", "personal:rw"])?;
    let token = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("hcp_"))
        .ok_or("token not shown")?
        .to_string();
    let (child, port) = serve_on_a_free_port(bin, &dir, &[])?;
    let _srv = Child(child, dir.clone());
    // 64 connections from this address, without a key, each announcing a body (bigger than what the server reads
    // before handing the request on) and sending none of it.
    let mut held = vec![];
    for _ in 0..64 {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(b"POST /oauth/token HTTP/1.1\r\nHost: x\r\nContent-Length: 200000\r\n\r\nab")?;
        held.push(s);
    }
    std::thread::sleep(Duration::from_millis(500));
    // The person's assistant, from the same address, with its key: answered, not told to come back.
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "recall", "arguments": { "query": "x" } } }).to_string();
    let (head, _) = http(
        port,
        &format!(
            "POST /mcp/keyed HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
    )?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    drop(held);
    Ok(())
}

#[test]
fn a_body_cut_short_is_not_taken_as_a_request() -> Res {
    let dir = temp("cut-body");
    let bin = env!("CARGO_BIN_EXE_memcp");
    let run = |args: &[&str]| -> Result<String, Box<dyn Error>> {
        let o = std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(args)
            .output()?;
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    };
    run(&["init", "--spaces", "personal"])?;
    let out = run(&["connect", "keyed", "--grant", "personal:rw"])?;
    let token = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("hcp_"))
        .ok_or("token not shown")?
        .to_string();
    let (child, port) = serve_on_a_free_port(bin, &dir, &[])?;
    let _srv = Child(child, dir.clone());
    // A whole JSON request, but the connection ends before the body it announced: the message is not complete.
    let payload =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "remember",
        "arguments": { "space": "personal", "text": "Written by a request that never ended." } } })
        .to_string();
    let padded = format!("{payload:<1400}");
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.write_all(
        format!(
            "POST /mcp/keyed HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 1500\r\n\r\n{padded}"
        )
        .as_bytes(),
    )?;
    s.shutdown(std::net::Shutdown::Write)?;
    let mut answer = String::new();
    let _ = std::io::Read::read_to_string(&mut s, &mut answer);
    assert!(!answer.contains("created"), "{answer}");
    std::thread::sleep(Duration::from_millis(300));
    let listed = run(&["note", "list", "personal"])?;
    assert!(!listed.contains("never ended"), "{listed}");
    Ok(())
}

#[test]
fn a_connection_that_falls_silent_or_drips_gives_its_place_back() -> Res {
    let dir = temp("silent");
    let bin = env!("CARGO_BIN_EXE_memcp");
    assert!(
        std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(["init", "--spaces", "personal"])
            .output()?
            .status
            .success()
    );
    // One second of silence for the test (30 by default).
    let (child, port) = serve_with_env(bin, &dir, &[], &[("MEMCP_READ_TIMEOUT_SECS", "1")])?;
    let _srv = Child(child, dir.clone());
    let pair = "POST /pair HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
    let answered_within = |secs: u64| -> Result<bool, Box<dyn Error>> {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(secs) {
            std::thread::sleep(Duration::from_millis(250));
            let (head, _) = http(port, pair)?;
            if !head.starts_with("HTTP/1.1 503") {
                return Ok(true);
            }
        }
        Ok(false)
    };
    // Every keyless place of this address taken by a body that never comes.
    let mut held = vec![];
    for _ in 0..24 {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(b"POST /oauth/token HTTP/1.1\r\nHost: x\r\nContent-Length: 200000\r\n\r\nab")?;
        held.push(s);
    }
    std::thread::sleep(Duration::from_millis(300));
    let (head, _) = http(port, pair)?;
    assert!(
        head.starts_with("HTTP/1.1 503"),
        "the places are taken: {head}"
    );
    assert!(
        answered_within(10)?,
        "still turned away 10 s after they fell silent"
    );
    drop(held);
    // Every place taken by bodies sent a byte at a time: never silent long enough for the timeout of one read, they
    // meet the deadline of the whole body.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut drips = vec![];
    for _ in 0..16 {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(b"POST /oauth/token HTTP/1.1\r\nHost: x\r\nContent-Length: 200000\r\n\r\n")?;
        let stop = stop.clone();
        drips.push(std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if s.write_all(b"a").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }));
    }
    std::thread::sleep(Duration::from_millis(500));
    let (head, _) = http(port, pair)?;
    assert!(
        head.starts_with("HTTP/1.1 503"),
        "the places are taken: {head}"
    );
    let freed = answered_within(10)?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for d in drips {
        let _ = d.join();
    }
    assert!(
        freed,
        "still turned away 10 s after the bodies began to drip"
    );
    Ok(())
}

#[test]
fn an_endless_request_line_or_header_is_cut_short() -> Res {
    let dir = temp("endless-line");
    let bin = env!("CARGO_BIN_EXE_memcp");
    assert!(
        std::process::Command::new(bin)
            .arg("--data")
            .arg(&dir)
            .args(["init", "--spaces", "personal"])
            .output()?
            .status
            .success()
    );
    let (child, port) = serve_on_a_free_port(bin, &dir, &[])?;
    let _srv = Child(child, dir.clone());
    // Without a line break the server used to keep every byte, until the memory ran out.
    for start in [&b"GET /"[..], &b"GET / HTTP/1.1\r\nX-Long: "[..]] {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.write_all(start)?;
        let chunk = vec![b'a'; 1 << 20];
        let mut sent = 0usize;
        for _ in 0..64 {
            if s.write_all(&chunk).is_err() {
                break;
            }
            sent += 1;
        }
        assert!(sent < 64, "64 MiB taken in one line");
    }
    let (head, _) = http(
        port,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )?;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    Ok(())
}

/// Served on a port of its own without `--public-url`, the node names that port in what it hands out: the address of a
/// new connection and the server a device pairs with (they named port 8791 whatever the listener, and the app that
/// made the connection threw it away).
#[test]
fn without_a_public_address_the_node_names_the_port_it_listens_on() -> Res {
    use std::io::{BufRead, BufReader};
    let dir = temp("listened");
    let bin = env!("CARGO_BIN_EXE_memcp");
    let init = std::process::Command::new(bin)
        .arg("--data")
        .arg(&dir)
        .args(["init", "--spaces", "personal"])
        .output()?;
    assert!(init.status.success(), "{init:?}");
    let console = access::console_token(&Brain::open(&dir)?.open_control()?, "owner")?;
    // A port of its own, not the default one.
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let listen = format!("127.0.0.1:{port}");
    let mut child = std::process::Command::new(bin)
        .arg("--data")
        .arg(&dir)
        .args(["serve", "--listen", &listen])
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut reader = BufReader::new(child.stderr.take().ok_or("no stderr")?);
    let mut line = String::new();
    while !line.contains("listening on") {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err("the server stopped".into());
        }
    }
    std::thread::spawn(move || for _ in reader.lines() {});
    let _srv = Child(child, dir.clone());
    let post = |path: &str, body: &str| -> Result<Value, Box<dyn Error>> {
        let (head, b) = http(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {console}\r\nX-Requested-With: memcp\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        )?;
        assert!(head.starts_with("HTTP/1.1 200"), "{path}: {head}\n{b}");
        Ok(serde_json::from_str(&b)?)
    };
    let made = post(
        "/admin/api/connections",
        &json!({ "agent": "mac-app", "rights": "all" }).to_string(),
    )?;
    let url = format!("http://127.0.0.1:{port}");
    assert_eq!(made["mcp_url"], format!("{url}/mcp/mac-app"), "{made}");
    let pairing = post("/admin/api/pairing", "{}")?;
    assert_eq!(pairing["server"], url, "{pairing}");
    Ok(())
}
