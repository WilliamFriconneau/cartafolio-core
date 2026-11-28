#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::Accounts;
use pensine_server::{Config, Server};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pensine"))
        .args(args)
        .arg("--data")
        .arg(data)
        .output()
        .unwrap()
}

fn success(data: &Path, args: &[&str]) -> String {
    let output = run(data, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn key_create_prints_only_the_key_and_it_authenticates() {
    let dir = tempfile::tempdir().unwrap();
    success(
        dir.path(),
        &["account", "create", "demo", "--alias", "sample"],
    );
    let printed = success(
        dir.path(),
        &[
            "key",
            "create",
            "--account",
            "demo",
            "--name",
            "Owner",
            "--all",
        ],
    );
    assert_eq!(printed.lines().count(), 1);
    let key = printed.strip_suffix('\n').unwrap();
    assert_eq!(key.len(), 67);
    assert!(key.starts_with("pk_"));
    let accounts = Accounts::open(dir.path()).unwrap();
    assert_eq!(accounts.alias_of("demo").unwrap(), "sample");
    let principal = accounts.authenticate(key).unwrap();
    assert!(principal.can_write_root());
    let listing = success(dir.path(), &["key", "list", "--account", "demo"]);
    assert!(listing.contains("Owner"));
    assert!(!listing.contains(key));
    success(
        dir.path(),
        &["key", "revoke", "--account", "demo", "--name", "Owner"],
    );
    assert!(accounts.authenticate(key).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_key_cannot_write_over_http() {
    let dir = tempfile::tempdir().unwrap();
    success(dir.path(), &["account", "create", "demo"]);
    success(
        dir.path(),
        &["space", "create", "--account", "demo", "notes", "Notes"],
    );
    let key = success(
        dir.path(),
        &[
            "key",
            "create",
            "--account",
            "demo",
            "--name",
            "Reader",
            "--read-only",
            "--grant",
            "notes:rw",
        ],
    );
    let server = Server::start(Config {
        data_dir: dir.path().to_path_buf(),
        listen: "127.0.0.1:0".parse().unwrap(),
        ..Config::default()
    })
    .await
    .unwrap();
    let call = |name, arguments| {
        let response = ureq::post(&format!("http://{}/mcp", server.local_addr()))
            .set("Authorization", &format!("Bearer {}", key.trim_end()))
            .set("Content-Type", "application/json")
            .send_string(
                &serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}
                })
                .to_string(),
            )
            .unwrap()
            .into_string()
            .unwrap();
        serde_json::from_str::<serde_json::Value>(&response).unwrap()
    };
    let write = call(
        "remember",
        serde_json::json!({"space": "notes", "text": "Example"}),
    );
    assert_eq!(write["result"]["isError"], true);
    let read = call("list", serde_json::json!({"space": "notes"}));
    assert_eq!(read["result"]["isError"], false);
    server.shutdown();
}

#[test]
fn check_finds_and_fixes_orphan_blobs() {
    let dir = tempfile::tempdir().unwrap();
    for account in ["demo", "empty"] {
        success(dir.path(), &["account", "create", account]);
    }
    let blobs = dir.path().join("accounts/demo/blobs");
    let digest = "b".repeat(64);
    let orphan = blobs.join("bb").join(&digest);
    let temp = blobs.join(".tmpInterrupted");
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, b"unused").unwrap();
    fs::write(&temp, b"incomplete").unwrap();

    let report = success(dir.path(), &["check"]);
    assert!(report.contains(&digest));
    assert!(report.contains(".tmpInterrupted"));
    assert!(report.contains("empty"));
    assert!(orphan.exists() && temp.exists());
    assert!(success(dir.path(), &["check", "--fix"]).contains(&digest));
    assert!(!orphan.exists() && !temp.exists());
    assert!(!success(dir.path(), &["check"]).contains(&digest));
}

#[test]
fn account_alias_changes_the_alias_and_rejects_bad_ones() {
    let data = tempfile::tempdir().unwrap();
    assert!(run(
        data.path(),
        &["account", "create", "main", "--alias", "first"]
    )
    .status
    .success());
    let changed = run(
        data.path(),
        &["account", "alias", "--account", "main", "renamed"],
    );
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let list = String::from_utf8(run(data.path(), &["account", "list"]).stdout).unwrap();
    assert!(list.contains("main\trenamed"), "{list}");
    for bad in ["a", "mcp", "Will", "-x", "x-"] {
        let refused = run(data.path(), &["account", "alias", "--account", "main", bad]);
        assert!(!refused.status.success(), "{bad} should be refused");
    }
    let unknown = run(
        data.path(),
        &["account", "alias", "--account", "nobody", "fine"],
    );
    assert!(!unknown.status.success());
}
