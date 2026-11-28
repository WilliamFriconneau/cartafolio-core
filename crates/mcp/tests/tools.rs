#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::{Accounts, Grant, Principal};
use pensine_mcp::Mcp;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

fn setup(read_only: bool) -> (TempDir, Mcp, Principal) {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "A", None).unwrap();
    let (mut connection, _) = accounts.connection_create_owner("demo", "Owner").unwrap();
    connection.read_only = read_only;
    let principal = Principal::new(account, connection).unwrap();
    (dir, Mcp::new(accounts), principal)
}

fn request(mcp: &Mcp, principal: &Principal, method: &str, params: Value) -> Value {
    mcp.handle(
        principal,
        json!({"jsonrpc":"2.0","id":7,"method":method,"params":params}),
    )
}

fn call(mcp: &Mcp, principal: &Principal, name: &str, arguments: Value) -> Value {
    request(
        mcp,
        principal,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )["result"]
        .clone()
}

fn content(result: &Value) -> Value {
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(result["content"][0]["type"], "text");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn tools_list_hides_write_tools_from_a_read_only_connection() {
    let (_dir, mcp, principal) = setup(true);
    let response = request(&mcp, &principal, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "recall",
            "list",
            "read",
            "history",
            "spaces",
            "file_get",
            "file_list",
            "access"
        ]
    );
    let list = tools.iter().find(|tool| tool["name"] == "list").unwrap();
    assert_eq!(list["inputSchema"]["required"], json!([]));
    let denied = call(
        &mcp,
        &principal,
        "remember",
        json!({"space":"aa","text":"Blocked"}),
    );
    assert_eq!(denied["isError"], true);
}

#[test]
fn remember_many_answers_per_note() {
    let (_dir, mcp, principal) = setup(false);
    let args =
        json!({"space":"aa","notes":[{"text":"First","origin":["demo","one"]},{"text":"Second"}]});
    let first = content(&call(&mcp, &principal, "remember_many", args.clone()));
    let second = content(&call(&mcp, &principal, "remember_many", args));
    assert_eq!(first.as_array().unwrap().len(), 2);
    assert_eq!(first[0]["outcome"], "created");
    assert_eq!(second[0]["outcome"], "replayed");
    assert_eq!(second[0]["fact"]["id"], first[0]["fact"]["id"]);
    assert_eq!(second[1]["outcome"], "duplicate");
}

#[test]
fn an_unknown_tool_is_a_clean_error() {
    let (_dir, mcp, principal) = setup(false);
    let response = call(&mcp, &principal, "missing_tool", json!({}));
    assert_eq!(response["isError"], true);
    assert_eq!(response["content"][0]["text"], "Unknown tool");
}

#[test]
fn a_malformed_request_is_a_json_rpc_error_not_a_crash() {
    let (_dir, mcp, principal) = setup(false);
    for invalid in [
        json!([]),
        json!({"jsonrpc":"2.0","id":8,"method":5}),
        json!({"id":1,"method":"ping"}),
    ] {
        let response = mcp.handle(&principal, invalid);
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["error"]["code"], -32600);
    }
    assert_eq!(
        mcp.handle(
            &principal,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        ),
        Value::Null
    );
}

#[test]
fn initialization_and_file_round_trip() {
    let (_dir, mcp, principal) = setup(false);
    let init = request(&mcp, &principal, "initialize", json!({}));
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "pensine");
    assert_eq!(init["result"]["capabilities"], json!({"tools":{}}));
    let response = request(&mcp, &principal, "tools/list", json!({}));
    assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 20);
    let file = content(&call(
        &mcp,
        &principal,
        "file_put",
        json!({"space":"aa","name":"example.txt","base64":"aGk="}),
    ));
    let fetched = content(&call(
        &mcp,
        &principal,
        "file_get",
        json!({"id":file["id"]}),
    ));
    assert_eq!(fetched["base64"], "aGk=");
    assert_eq!(fetched["size"], 2);
}

#[test]
fn file_get_reports_the_size_above_the_download_limit() {
    use pensine_store::NewFile;
    use std::io::{self, Read};

    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "A", None).unwrap();
    let (connection, _) = accounts.connection_create_owner("demo", "Owner").unwrap();
    let bytes = 8_000_001;
    let file = account
        .store
        .file_put(NewFile {
            space: "aa".into(),
            name: "example.bin".into(),
            mime: "application/octet-stream".into(),
            reader: io::repeat(0).take(bytes),
            max_size: bytes,
            note: None,
        })
        .unwrap();
    let principal = Principal::new(account, connection).unwrap();
    let response = call(
        &Mcp::new(accounts),
        &principal,
        "file_get",
        json!({"id":file.id}),
    );
    assert_eq!(response["isError"], true);
    assert!(response["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("8000001 bytes"));
}

#[test]
fn tools_list_has_20_tools_and_hides_pin_from_read_only() {
    let (_dir, mcp, owner) = setup(false);
    let response = request(&mcp, &owner, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 20);
    for name in ["pin", "list", "recall"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(
            tool["inputSchema"]["properties"]["pinned"]["type"],
            "boolean"
        );
        assert!(tool["description"].is_string());
    }
    let (_dir, mcp, reader) = setup(true);
    let response = request(&mcp, &reader, "tools/list", json!({}));
    assert!(response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["name"] != "pin"));
}

#[test]
fn initialize_instructions_describe_the_targets_note() {
    let (_dir, mcp, owner) = setup(false);
    let response = request(&mcp, &owner, "initialize", json!({}));
    let instructions = response["result"]["instructions"].as_str().unwrap();
    for phrase in [
        "targets",
        "supersede",
        "pinned",
        "30 days",
        "since",
        "recall",
        "tags_set",
        "move",
        "diagnosis",
    ] {
        assert!(instructions.contains(phrase), "{instructions}");
    }
    assert!(instructions.split_whitespace().count() <= 140);
}

#[test]
fn targets_note_keeps_its_pin_through_supersede_over_mcp() {
    let (_dir, mcp, owner) = setup(false);
    let saved = content(&call(
        &mcp,
        &owner,
        "remember",
        json!({"space":"aa","text":"Water the orchard","tags":["targets"]}),
    ));
    let old = &saved["fact"]["id"];
    let pinned = content(&call(&mcp, &owner, "pin", json!({"id":old})));
    assert_eq!(pinned["pinned"], true);

    let updated = content(&call(
        &mcp,
        &owner,
        "supersede",
        json!({"id":old,"text":"Orchard watered","tags":["targets"],"reason":"New evidence"}),
    ));
    assert_ne!(updated["id"], *old);

    let page = content(&call(
        &mcp,
        &owner,
        "list",
        json!({"space":"aa","pinned":true}),
    ));
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], updated["id"]);
    assert_eq!(items[0]["tags"], json!(["targets"]));
}

#[test]
fn pin_filters_list_and_recall_and_counts_active_pins() {
    let (_dir, mcp, owner) = setup(false);
    let saved = content(&call(
        &mcp,
        &owner,
        "remember",
        json!({"space":"aa","text":"Orchard east"}),
    ));
    let id = &saved["fact"]["id"];
    content(&call(
        &mcp,
        &owner,
        "remember",
        json!({"space":"aa","text":"Orchard west"}),
    ));
    assert_eq!(
        content(&call(&mcp, &owner, "pin", json!({"id":id})))["pinned"],
        true
    );
    let page = content(&call(&mcp, &owner, "list", json!({"pinned":true})));
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], *id);
    let hits = content(&call(
        &mcp,
        &owner,
        "recall",
        json!({"query":"orchard","pinned":true}),
    ));
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["fact"]["id"], *id);
    assert_eq!(
        content(&call(&mcp, &owner, "spaces", json!({})))[0]["pinned"],
        1
    );
    assert_eq!(
        content(&call(&mcp, &owner, "pin", json!({"id":id,"pinned":false})))["pinned"],
        false
    );
    assert_eq!(
        content(&call(&mcp, &owner, "spaces", json!({})))[0]["pinned"],
        0
    );
}

#[test]
fn tools_list_annotations_match_the_table() {
    let (_dir, mcp, owner) = setup(false);
    let response = request(&mcp, &owner, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().unwrap();
    let expected = [
        ("remember", false, false, true),
        ("remember_many", false, false, true),
        ("recall", true, false, false),
        ("list", true, false, false),
        ("pin", false, false, true),
        ("read", true, false, false),
        ("history", true, false, false),
        ("supersede", false, true, false),
        ("retire", false, true, false),
        ("restore", false, false, true),
        ("tags_set", false, false, true),
        ("move", false, true, false),
        ("spaces", true, false, false),
        ("space_create", false, false, true),
        ("space_move", false, true, false),
        ("file_put", false, false, false),
        ("file_get", true, false, false),
        ("file_list", true, false, false),
        ("file_retire", false, true, false),
        ("access", true, false, false),
    ];
    assert_eq!(tools.len(), expected.len());
    for (tool, (name, read_only, destructive, idempotent)) in tools.iter().zip(expected) {
        assert_eq!(tool["name"], name);
        let annotations = &tool["annotations"];
        assert!(
            annotations["title"]
                .as_str()
                .is_some_and(|title| !title.is_empty()),
            "{name}"
        );
        assert_eq!(annotations["readOnlyHint"], read_only, "{name}");
        assert_eq!(annotations["destructiveHint"], destructive, "{name}");
        assert_eq!(annotations["idempotentHint"], idempotent, "{name}");
        assert_eq!(annotations["openWorldHint"], false, "{name}");
    }
}

#[test]
fn access_reports_the_scope_of_a_scoped_connection() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "A", None).unwrap();
    account.store.space_create("bb", "B", None).unwrap();
    let (connection, key) = accounts
        .connection_create(
            "demo",
            "Scoped",
            vec![Grant {
                space: "aa".into(),
                write: true,
            }],
            false,
        )
        .unwrap();
    let principal = accounts.authenticate(key.as_str()).unwrap();
    let mcp = Mcp::new(accounts);
    let scope = content(&call(&mcp, &principal, "access", json!({})));
    assert_eq!(scope["account"], "demo");
    assert_eq!(scope["alias"], "demo");
    assert_eq!(scope["connection"], connection.id);
    assert_eq!(scope["read_only"], false);
    assert_eq!(scope["grants"], json!([{"space":"aa","write":true}]));
    assert_eq!(scope["visible_spaces"], 1);
}

#[test]
fn access_works_for_a_read_only_connection() {
    let (_dir, mcp, principal) = setup(true);
    let scope = content(&call(&mcp, &principal, "access", json!({})));
    assert_eq!(scope["read_only"], true);
    assert_eq!(scope["visible_spaces"], 1);
    let response = request(&mcp, &principal, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "access"));
}

#[test]
fn spaces_reports_writable_per_space() {
    let (_dir, mcp, owner) = setup(false);
    let spaces = content(&call(&mcp, &owner, "spaces", json!({})));
    assert_eq!(spaces[0]["name"], "aa");
    assert_eq!(spaces[0]["writable"], true);

    let (_dir, mcp, reader) = setup(true);
    let spaces = content(&call(&mcp, &reader, "spaces", json!({})));
    assert_eq!(spaces[0]["writable"], false);
}
