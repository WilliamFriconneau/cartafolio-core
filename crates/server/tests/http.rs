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
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 20);

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

const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

fn status_response(result: Result<ureq::Response, ureq::Error>, expected: u16) -> ureq::Response {
    match result {
        Err(ureq::Error::Status(status, response)) => {
            assert_eq!(status, expected);
            response
        }
        _ => panic!("expected HTTP {expected}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_alias_routes_the_mcp_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let account = accounts
        .account_for_ha_user("user-a", "Sample User")
        .unwrap();
    let (_, key) = accounts
        .connection_create_owner(&account.id, "Owner")
        .unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    let url = format!("{}/sample-user", mcp_url(&handle));
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", key.as_str()))
        .send_string(PING)
        .unwrap();
    assert_eq!(response.status(), 200);
    status_response(ureq::get(&url).call(), 405);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_principal_of_another_account_gets_the_same_404_as_an_unknown_alias() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts
        .account_for_ha_user("user-a", "First User")
        .unwrap();
    let account = accounts
        .account_for_ha_user("user-b", "Second User")
        .unwrap();
    let (_, key) = accounts
        .connection_create_owner(&account.id, "Owner")
        .unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    for alias in ["first-user", "unknown-user"] {
        let response = status_response(
            ureq::post(&format!("{}/{alias}", mcp_url(&handle)))
                .set("Authorization", &format!("Bearer {}", key.as_str()))
                .send_string(PING),
            404,
        );
        assert_eq!(response.into_string().unwrap(), "");
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_protected_resource_document_is_path_specific() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts
        .account_for_ha_user("user-a", "Sample User")
        .unwrap();
    let issuer = "https://pensine.example";
    let handle = Server::start(Config {
        public_url: Some(format!("{issuer}/")),
        ..config(&dir)
    })
    .await
    .unwrap();
    for alias in ["sample-user", "unknown-user"] {
        let path = format!("/.well-known/oauth-protected-resource/mcp/{alias}");
        let challenge = status_response(
            ureq::post(&format!("{}/{alias}", mcp_url(&handle))).send_string(PING),
            401,
        );
        assert_eq!(
            challenge.header("WWW-Authenticate").unwrap(),
            format!("Bearer resource_metadata=\"{issuer}{path}\"")
        );
        let body = ureq::get(&format!("http://{}{path}", handle.local_addr()))
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(metadata["resource"], format!("{issuer}/mcp/{alias}"));
        assert_eq!(
            metadata["authorization_servers"],
            serde_json::json!([issuer])
        );
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_legacy_mcp_path_still_works_with_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let account = accounts
        .account_for_ha_user("user-a", "Sample User")
        .unwrap();
    let (_, key) = accounts
        .connection_create_owner(&account.id, "Owner")
        .unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    let response = ureq::post(&mcp_url(&handle))
        .set("Authorization", &format!("Bearer {}", key.as_str()))
        .send_string(PING)
        .unwrap();
    assert_eq!(response.status(), 200);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oauth_token_routes_by_account_after_an_alias_change() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "Notes", None).unwrap();
    let issuer = "https://pensine.example";
    let handle = Server::start(Config {
        public_url: Some(issuer.into()),
        ..config(&dir)
    })
    .await
    .unwrap();

    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let url = |path| format!("http://{}{path}", handle.local_addr());
    let callback = "https://client.example/callback";
    let registration: Value = serde_json::from_str(
        &agent
            .post(&url("/register"))
            .set("Content-Type", "application/json")
            .send_string(&json!({"redirect_uris": [callback]}).to_string())
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    let client = registration["client_id"].as_str().unwrap();
    let verifier = "abcdefghijklmnopqrstuvwxyz0123456789-._~abcdef";
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    let resource = format!("{issuer}/mcp/demo");
    let invite = handle.oauth_invitation("demo").unwrap();
    let consent = agent
        .post(&url("/authorize"))
        .send_form(&[
            ("response_type", "code"),
            ("client_id", client),
            ("redirect_uri", callback),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("resource", &resource),
            ("invite", &invite),
        ])
        .unwrap()
        .into_string()
        .unwrap();
    let nonce = consent
        .split_once("name=\"nonce\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    let decision = agent
        .post(&url("/authorize"))
        .send_form(&[("nonce", nonce), ("decision", "allow"), ("v_aa", "1")])
        .unwrap();
    let redirect = url::Url::parse(decision.header("Location").unwrap()).unwrap();
    let (_, code) = redirect
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap();
    let tokens: Value = serde_json::from_str(
        &agent
            .post(&url("/token"))
            .send_form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client),
                ("redirect_uri", callback),
                ("code", &code),
                ("code_verifier", verifier),
                ("resource", &resource),
            ])
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    let bearer = format!("Bearer {}", tokens["access_token"].as_str().unwrap());
    let call = |path| agent.post(&url(path)).set("Authorization", &bearer);
    assert_eq!(call("/mcp/demo").send_string(PING).unwrap().status(), 200);
    accounts.set_alias("demo", "new-demo").unwrap();
    assert_eq!(
        status_response(call("/mcp/demo").send_string(PING), 404)
            .into_string()
            .unwrap(),
        ""
    );
    assert_eq!(
        call("/mcp/new-demo").send_string(PING).unwrap().status(),
        200
    );
    handle.shutdown();
}

fn public_url_file_config(dir: &tempfile::TempDir, explicit: &str) -> Config {
    Config::from_toml(&format!(
        "listen = \"127.0.0.1:0\"\ndata_dir = {:?}\npublic_url_file = {:?}\n{explicit}",
        dir.path(),
        dir.path().join("public_url")
    ))
    .unwrap()
}

#[test]
fn the_public_url_file_is_used_when_no_explicit_url_is_set() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("public_url"), " https://funnel.example/ \n").unwrap();
    let config = public_url_file_config(&dir, "");
    assert_eq!(
        config.public_url().as_deref(),
        Some("https://funnel.example/")
    );
    let config = public_url_file_config(&dir, "public_url = \"https://proxy.example\"");
    assert_eq!(
        config.public_url().as_deref(),
        Some("https://proxy.example")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_written_after_startup_is_picked_up() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(Config {
        admin_listen: Some("127.0.0.1:0".parse().unwrap()),
        admin_trusted_proxy: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ..public_url_file_config(&dir, "")
    })
    .await
    .unwrap();
    let local = format!("http://{}", handle.local_addr());
    let read_metadata = |path: &str| -> serde_json::Value {
        let body = ureq::get(&format!("{local}/.well-known/{path}"))
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        serde_json::from_str(&body).unwrap()
    };
    let connections_page = || {
        ureq::get(&format!("http://{}/", handle.admin_addr().unwrap()))
            .set("X-Remote-User-Id", "user-a")
            .set("X-Remote-User-Name", "Demo")
            .call()
            .unwrap()
            .into_string()
            .unwrap()
    };
    assert_eq!(read_metadata("oauth-authorization-server")["issuer"], local);
    assert!(connections_page().contains("Set the public address"));
    std::fs::write(dir.path().join("public_url"), "https://funnel.example\n").unwrap();
    assert_eq!(
        read_metadata("oauth-authorization-server")["issuer"],
        "https://funnel.example"
    );
    assert_eq!(
        read_metadata("oauth-protected-resource/mcp/demo")["resource"],
        "https://funnel.example/mcp/demo"
    );
    assert!(connections_page().contains("https://funnel.example/mcp/demo"));
    for (path, suffix) in [("/mcp", ""), ("/mcp/demo", "/mcp/demo")] {
        let Err(ureq::Error::Status(401, response)) =
            ureq::post(&format!("{local}{path}")).send_string("{}")
        else {
            panic!("expected 401");
        };
        assert_eq!(
            response.header("WWW-Authenticate").unwrap(),
            format!("Bearer resource_metadata=\"https://funnel.example/.well-known/oauth-protected-resource{suffix}\"")
        );
    }
    handle.shutdown();
}

#[test]
fn an_invalid_file_content_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let config = public_url_file_config(&dir, "");
    assert!(config.public_url().is_none());
    for value in [
        "",
        "not a URL",
        "http://localhost",
        "https://",
        "https:///example.test",
        "https://example.test/.",
        "https://example.test/a",
        "https://example.test with spaces",
        "https://user@example.test",
        "https://example.test#fragment",
    ] {
        std::fs::write(dir.path().join("public_url"), value).unwrap();
        assert!(config.public_url().is_none(), "{value:?}");
    }
}
