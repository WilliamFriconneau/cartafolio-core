use super::*;

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
    assert!(connections_page().contains("Turn on Tailscale in the add-on options, then reload"));
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
