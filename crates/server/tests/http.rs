#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::Accounts;
use pensine_server::{Config, Handle, Server};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn config(dir: &tempfile::TempDir) -> Config {
    Config {
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    }
}

fn mcp_url(handle: &Handle) -> String {
    format!("http://{}/mcp", handle.local_addr())
}

#[tokio::test(flavor = "multi_thread")]
async fn healthz_needs_no_key() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    let url = format!("http://{}/healthz", handle.local_addr());
    let body = ureq::get(&url).call().unwrap().into_string().unwrap();
    assert!(body.starts_with("ok pensine "));
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unauthenticated_call_is_401() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    let url = mcp_url(&handle);
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let response = match ureq::post(&url).send_string(body) {
        Err(ureq::Error::Status(code, response)) => {
            assert_eq!(code, 401);
            response
        }
        _ => panic!("expected 401"),
    };
    assert_eq!(response.header("WWW-Authenticate").unwrap(), "Bearer");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_granted_call_works_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "A", None).unwrap();
    let (_connection, key) = accounts.connection_create_owner("demo", "Owner").unwrap();

    let handle = Server::start(config(&dir)).await.unwrap();
    let url = mcp_url(&handle);
    let auth = format!("Bearer {}", key.as_str());

    let list = ureq::post(&url)
        .set("Authorization", &auth)
        .set("Content-Type", "application/json")
        .send_string(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
        .unwrap()
        .into_string()
        .unwrap();
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 19);

    let remember = ureq::post(&url)
        .set("Authorization", &auth)
        .set("Content-Type", "application/json")
        .send_string(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"remember","arguments":{"space":"aa","text":"hello from http"}}}"#,
        )
        .unwrap()
        .into_string()
        .unwrap();
    let remember: serde_json::Value = serde_json::from_str(&remember).unwrap();
    assert_eq!(remember["id"], 2);
    assert_eq!(remember["result"]["isError"], false);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_the_limit_is_413() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts.account_create("demo").unwrap();
    let (_connection, key) = accounts.connection_create_owner("demo", "Owner").unwrap();

    let cfg = Config {
        max_body: 128,
        ..config(&dir)
    };
    let handle = Server::start(cfg).await.unwrap();
    let url = mcp_url(&handle);
    let padding = "x".repeat(512);
    let body =
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{padding}"}}}}"#);
    let result = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", key.as_str()))
        .send_string(&body);
    match result {
        Err(ureq::Error::Status(code, _)) => assert_eq!(code, 413),
        _ => panic!("expected 413"),
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn wildcard_bind_refuses_when_loopback_is_taken() {
    let loopback = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = loopback.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();

    let cfg = Config {
        listen: format!("0.0.0.0:{port}").parse().unwrap(),
        allow_remote: true,
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };

    let result = Server::start(cfg).await;
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_notification_is_accepted_with_202() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts.account_create("demo").unwrap();
    let (_connection, key) = accounts.connection_create_owner("demo", "Owner").unwrap();

    let handle = Server::start(config(&dir)).await.unwrap();
    let url = mcp_url(&handle);
    let body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", key.as_str()))
        .send_string(body)
        .unwrap();
    assert_eq!(response.status(), 202);
    assert!(response.into_string().unwrap().is_empty());
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn get_on_mcp_is_405() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    let url = mcp_url(&handle);
    let response = ureq::get(&url).call();
    match response {
        Err(ureq::Error::Status(code, _)) => assert_eq!(code, 405),
        _ => panic!("expected 405"),
    }
    handle.shutdown();
}
