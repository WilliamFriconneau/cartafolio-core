#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::Accounts;
use pensine_mcp::Mcp;
use serde_json::{json, Value};
use std::sync::Arc;

#[test]
fn the_access_tool_reports_usage_and_quota() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "Notes", None).unwrap();
    let (_, key) = accounts.connection_create_owner("demo", "Owner").unwrap();
    let principal = accounts.authenticate(key.as_str()).unwrap();
    let mcp = Mcp::new(accounts);
    let call = |name, arguments| {
        let result = mcp.handle(&principal, json!({
            "jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name, "arguments":arguments}
        }));
        assert_eq!(result["result"]["isError"], false);
        serde_json::from_str::<Value>(result["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap()
    };
    call(
        "file_put",
        json!({"space":"aa", "name":"example.txt", "base64":"YWJj"}),
    );
    assert_eq!(
        call("access", json!({}))["file_storage"],
        json!({
            "bytes": 3, "files": 1, "max_bytes": 20_000_000_000u64, "max_files": 100_000
        })
    );
}
