#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_access::Accounts;
use pensine_server::{Config, Handle, Server};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn admin_config(dir: &tempfile::TempDir, trusted_proxy: IpAddr) -> Config {
    Config {
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        data_dir: dir.path().to_path_buf(),
        admin_listen: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        admin_trusted_proxy: trusted_proxy,
        ..Config::default()
    }
}

fn admin_url(handle: &Handle, path: &str) -> String {
    format!("http://{}{path}", handle.admin_addr().unwrap())
}

fn csrf_cookie(response: &ureq::Response) -> String {
    let header = response.header("set-cookie").unwrap();
    header
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_string()
}

fn csrf_token(handle: &Handle, ha_id: &str) -> String {
    let response = ureq::get(&admin_url(handle, "/status"))
        .set("X-Remote-User-Id", ha_id)
        .call()
        .unwrap();
    csrf_cookie(&response)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admin_refuses_any_address_but_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(
        &dir,
        IpAddr::V4(Ipv4Addr::new(172, 30, 32, 2)),
    ))
    .await
    .unwrap();
    let refused = ureq::get(&admin_url(&handle, "/status"))
        .set("X-Remote-User-Id", "aaaa")
        .call();
    match refused {
        Err(ureq::Error::Status(code, response)) => {
            assert_eq!(code, 403);
            assert!(response.into_string().unwrap().is_empty());
        }
        _ => panic!("expected 403 from a non-proxy address"),
    }
    handle.shutdown();

    let handle = Server::start(admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .await
        .unwrap();
    let allowed = ureq::get(&admin_url(&handle, "/status"))
        .set("X-Remote-User-Id", "aaaa")
        .call()
        .unwrap();
    assert_eq!(allowed.status(), 200);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_ha_user_gets_an_account() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .await
        .unwrap();
    let response = ureq::get(&admin_url(&handle, "/status"))
        .set("X-Remote-User-Id", "a1b2c3d4e5f6")
        .set("X-Remote-User-Name", "Alice")
        .call()
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(dir
        .path()
        .join("accounts")
        .join("u-a1b2c3d4e5f6")
        .join("store.db")
        .exists());
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_appears_once() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .await
        .unwrap();
    let token = csrf_token(&handle, "aaaa");
    let created = ureq::post(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Cookie", &format!("pensine_csrf={token}"))
        .send_form(&[("name", "Owner"), ("csrf", &token)])
        .unwrap();
    assert_eq!(created.status(), 200);
    assert!(created.into_string().unwrap().contains("pk_"));

    let listed = ureq::get(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(!listed.contains("pk_"));
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_escapes_every_value() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let account = accounts.account_for_ha_user("aaaa", "Alice").unwrap();
    account
        .store
        .space_create("aa", "<script>\"'&label", None)
        .unwrap();
    drop(accounts);

    let handle = Server::start(admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .await
        .unwrap();
    let token = csrf_token(&handle, "aaaa");
    let name = "<script>\"'&name";
    let created = ureq::post(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Cookie", &format!("pensine_csrf={token}"))
        .send_form(&[("name", name), ("csrf", &token)])
        .unwrap();
    assert_eq!(created.status(), 200);
    let created = created.into_string().unwrap();
    assert!(created.contains("&lt;script&gt;&quot;&#39;&amp;name"));
    assert!(!created.contains("<script>"));

    let listed = ureq::get(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(listed.contains("&lt;script&gt;&quot;&#39;&amp;name"));
    assert!(listed.contains("&lt;script&gt;&quot;&#39;&amp;label"));
    assert!(!listed.contains("<script>"));
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_without_the_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .await
        .unwrap();
    let response = ureq::post(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .send_form(&[("name", "Owner")]);
    match response {
        Err(ureq::Error::Status(code, _)) => assert_eq!(code, 403),
        _ => panic!("expected 403 without a CSRF token"),
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_cannot_see_or_change_another_users_connections() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .await
        .unwrap();
    let token_a = csrf_token(&handle, "aaaa");
    ureq::post(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Cookie", &format!("pensine_csrf={token_a}"))
        .send_form(&[("name", "A's secret connection"), ("csrf", &token_a)])
        .unwrap();

    let db = rusqlite::Connection::open(dir.path().join("access.db")).unwrap();
    let connection_id: String = db
        .query_row(
            "SELECT id FROM connections WHERE account = 'u-aaaa'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let other = ureq::get(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "bbbb")
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(!other.contains("A's secret connection"));

    let token_b = csrf_token(&handle, "bbbb");
    ureq::post(&admin_url(&handle, "/connections/revoke"))
        .set("X-Remote-User-Id", "bbbb")
        .set("Cookie", &format!("pensine_csrf={token_b}"))
        .send_form(&[("id", &connection_id), ("csrf", &token_b)])
        .unwrap();

    let still_there: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM connections WHERE id = ?1)",
            [&connection_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(still_there);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_admin_shows_the_mcp_address_and_changes_the_alias() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(Config {
        public_url: Some("https://pensine.example/".into()),
        ..admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST))
    })
    .await
    .unwrap();
    let response = ureq::get(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "user-a")
        .set("X-Remote-User-Name", "Sample User")
        .call()
        .unwrap();
    let token = csrf_cookie(&response);
    let page = response.into_string().unwrap();
    assert!(page.contains("readonly value=\"https://pensine.example/mcp/sample-user\""));

    let post = |alias| {
        ureq::post(&admin_url(&handle, "/alias"))
            .set("X-Remote-User-Id", "user-a")
            .set("Cookie", &format!("pensine_csrf={token}"))
            .send_form(&[("alias", alias), ("csrf", &token)])
            .unwrap_or_else(|error| match error {
                ureq::Error::Status(_, response) => response,
                other => panic!("{other}"),
            })
    };
    assert!(matches!(
        ureq::post(&admin_url(&handle, "/alias"))
            .set("X-Remote-User-Id", "user-a")
            .send_form(&[("alias", "new-alias")]),
        Err(ureq::Error::Status(403, _))
    ));
    let invalid = post("mcp");
    assert_eq!(invalid.status(), 400);
    assert!(invalid.into_string().unwrap().contains("reserved"));
    let changed = post("new-alias");
    assert_eq!(changed.status(), 200);
    let changed = changed.into_string().unwrap();
    assert!(changed.contains("readonly value=\"https://pensine.example/mcp/new-alias\""));
    handle.shutdown();
}

async fn get_dashboard(public_url: Option<&str>, language: &str) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(Config {
        public_url: public_url.map(str::to_owned),
        ..admin_config(&dir, IpAddr::V4(Ipv4Addr::LOCALHOST))
    })
    .await
    .unwrap();
    let response = ureq::get(&admin_url(&handle, "/"))
        .set("X-Remote-User-Id", "aaaa")
        .set("X-Remote-User-Name", "Sample User")
        .set("Accept-Language", language)
        .call()
        .unwrap();
    let csp = response
        .header("Content-Security-Policy")
        .unwrap()
        .to_owned();
    let html = response.into_string().unwrap();
    handle.shutdown();
    (html, csp)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_page_closes_every_section_but_the_connect_guide() {
    let (html, _) = get_dashboard(None, "en").await;
    assert_eq!(html.matches("<details open>").count(), 1);
    assert!(html.contains("<details open><summary>Connect an assistant</summary>"));
    for summary in [
        "Connections and keys (0)",
        "Assistants signed in with a code (0)",
        "Spaces (0)",
        "Status:",
    ] {
        assert!(html.contains(&format!("<details><summary>{summary}")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_address_line_shows_the_public_address_alias_and_a_copy_button() {
    let (html, _) = get_dashboard(Some("https://pensine.example/"), "en").await;
    let header = html.split("</header>").next().unwrap();
    assert!(header.contains("Sample User"));
    assert!(header.contains("readonly value=\"https://pensine.example/mcp/sample-user\""));
    assert!(header.contains(">Copy</button>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_page_is_french_when_the_browser_asks_for_french_and_english_otherwise() {
    let (french, _) = get_dashboard(None, "fr-CA,en;q=0.8").await;
    assert!(french.contains("lang=\"fr\"") && french.contains("Que peut-il atteindre ?"));
    let (english, _) = get_dashboard(None, "en,fr;q=0.8").await;
    assert!(english.contains("lang=\"en\"") && english.contains("What can it reach?"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_public_address_shows_the_hint_instead_of_a_broken_address() {
    let (html, _) = get_dashboard(None, "en").await;
    assert!(html.contains("Turn on Tailscale in the add-on options, then reload"));
    assert!(!html.contains("/mcp/"));
    assert!(!html.contains(">Copy</button>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_csp_header_allows_only_the_nonce() {
    let (html, csp) = get_dashboard(Some("https://pensine.example"), "en").await;
    let nonce = html
        .split("<script nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(!nonce.is_empty());
    assert_eq!(html.matches("<script").count(), 1);
    assert!(csp
        .split(';')
        .any(|part| part.trim() == format!("script-src 'nonce-{nonce}'")));
    assert!(csp.contains("default-src 'none'"));
}
