#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::{Accounts, Principal};
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
            "file_list"
        ]
    );
    let list = tools.iter().find(|tool| tool["name"] == "list").unwrap();
    assert_eq!(list["inputSchema"]["required"], json!(["space"]));
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
    assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 18);
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
