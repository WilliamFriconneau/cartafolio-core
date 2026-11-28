#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::{Accounts, Grant, Principal};
use pensine_mcp::Mcp;
use pensine_store::{NewFact, NewFile, Store};
use serde_json::{json, Value};
use std::sync::Arc;

fn note(store: &Store, space: &str, text: &str, origin: Option<(&str, &str)>) -> String {
    store
        .remember(NewFact {
            space: space.into(),
            text: text.into(),
            tags: vec![],
            author: "fixture".into(),
            source: "test".into(),
            at: None,
            origin: origin.map(|(app, id)| (app.into(), id.into())),
        })
        .unwrap()
        .fact
        .id
}

fn call(mcp: &Mcp, principal: &Principal, tool: &str, arguments: Value) -> Value {
    mcp.handle(
        principal,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":tool,"arguments":arguments}}),
    )["result"]
        .clone()
}

fn answer(result: Value) -> Value {
    assert_ne!(result["isError"], true, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn permission_matrix_matches_missing_resources() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    let store = &account.store;
    store.space_create("bb", "Hidden parent", None).unwrap();
    store.space_create("aa", "Visible", Some("bb")).unwrap();
    store
        .space_create("ac", "Visible child", Some("aa"))
        .unwrap();
    let public = note(store, "aa", "Visible sample", None);
    let private = note(store, "bb", "Hidden sample", Some(("capture", "hidden")));
    let file = store
        .file_put(NewFile {
            space: "bb".into(),
            name: "fixture.txt".into(),
            mime: "text/plain".into(),
            reader: &b"sample"[..],
            max_size: 100,
            note: None,
        })
        .unwrap();
    let (_, key) = accounts
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

    for (tool, field, forbidden, mut args) in [
        ("remember", "space", "bb", json!({"text":"Example"})),
        (
            "remember_many",
            "space",
            "bb",
            json!({"notes":[{"text":"Example"}]}),
        ),
        ("recall", "space", "bb", json!({"query":"sample"})),
        ("list", "space", "bb", json!({})),
        ("read", "id", private.as_str(), json!({})),
        ("history", "id", private.as_str(), json!({})),
        (
            "supersede",
            "id",
            private.as_str(),
            json!({"text":"Changed","reason":"Correction"}),
        ),
        (
            "retire",
            "id",
            private.as_str(),
            json!({"reason":"Correction"}),
        ),
        ("restore", "id", private.as_str(), json!({})),
        ("tags_set", "id", private.as_str(), json!({"tags":[]})),
        ("move", "ids", private.as_str(), json!({"to_space":"aa"})),
        ("move", "to_space", "bb", json!({"ids":[public]})),
        (
            "space_create",
            "parent",
            "bb",
            json!({"name":"fresh","label":"New"}),
        ),
        ("space_move", "name", "bb", json!({"label":"Changed"})),
        ("space_move", "parent", "bb", json!({"name":"aa"})),
        (
            "file_put",
            "space",
            "bb",
            json!({"name":"test.txt","base64":"aGk="}),
        ),
        ("file_get", "id", file.id.as_str(), json!({})),
        ("file_list", "space", "bb", json!({})),
        (
            "file_retire",
            "id",
            file.id.as_str(),
            json!({"reason":"Correction"}),
        ),
    ] {
        let argument = |value| {
            if field == "ids" {
                json!([public, value])
            } else {
                json!(value)
            }
        };
        args[field] = argument(forbidden);
        let hidden = call(&mcp, &principal, tool, args.clone());
        args[field] = argument("missing");
        let missing = call(&mcp, &principal, tool, args);
        assert_eq!(hidden, missing, "{tool}.{field}");
        assert_eq!(hidden["isError"], true, "{tool}.{field}");
        assert_eq!(hidden["content"][0]["text"], "not found", "{tool}.{field}");
    }

    let spaces = answer(call(&mcp, &principal, "spaces", json!({})));
    assert_eq!(spaces.as_array().unwrap().len(), 2);
    assert_eq!(spaces[0]["name"], "aa");
    assert!(spaces[0]["parent"].is_null());
    assert_eq!(spaces[1]["parent"], "aa");
    let recall = answer(call(&mcp, &principal, "recall", json!({"query":"sample"})));
    assert_eq!(recall.as_array().unwrap().len(), 1);
    assert_eq!(recall[0]["fact"]["id"], public);
    let files = answer(call(&mcp, &principal, "file_list", json!({})));
    assert_eq!(files["items"], json!([]));

    let replay = call(
        &mcp,
        &principal,
        "remember_many",
        json!({"space":"aa","notes":[
            {"text":"Rolled back"}, {"text":"Retry","origin":["capture","hidden"]}
        ]}),
    );
    assert_eq!(replay["content"][0]["text"], "not found");
    let notes = answer(call(&mcp, &principal, "list", json!({"space":"aa"})));
    assert_eq!(notes["items"].as_array().unwrap().len(), 1);

    let moved = store.move_facts(&[private], "aa").unwrap().remove(0);
    let history = answer(call(&mcp, &principal, "history", json!({"id":moved.id})));
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert!(history[0]["replaces"].is_null());
}

#[test]
fn scoped_writes_require_a_writable_source_and_destination() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    for space in ["aa", "bb"] {
        account.store.space_create(space, space, None).unwrap();
    }
    let id = note(&account.store, "aa", "Written note", None);
    let other = note(&account.store, "bb", "Read-only note", None);
    let (_, key) = accounts
        .connection_create(
            "demo",
            "Scoped",
            vec![
                Grant {
                    space: "aa".into(),
                    write: true,
                },
                Grant {
                    space: "bb".into(),
                    write: false,
                },
            ],
            false,
        )
        .unwrap();
    let principal = accounts.authenticate(key.as_str()).unwrap();
    let mcp = Mcp::new(accounts);
    for (tool, args) in [
        ("move", json!({"ids":[id], "to_space":"bb"})),
        ("move", json!({"ids":[other], "to_space":"aa"})),
        ("space_create", json!({"name":"cc","label":"Root"})),
        (
            "space_create",
            json!({"name":"cc","label":"Child","parent":"bb"}),
        ),
        ("space_move", json!({"name":"aa","parent":"bb"})),
        ("space_move", json!({"name":"aa","parent":null})),
    ] {
        let response = call(&mcp, &principal, tool, args);
        assert_eq!(response["content"][0]["text"], "Write access required");
    }
    answer(call(
        &mcp,
        &principal,
        "space_create",
        json!({"name":"ac","label":"Child","parent":"aa"}),
    ));
    let moved = answer(call(
        &mcp,
        &principal,
        "move",
        json!({"ids":[id],"to_space":"ac"}),
    ));
    assert_eq!(moved[0]["space"], "ac");
}
