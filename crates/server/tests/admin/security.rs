use super::{admin_config, admin_url, csrf_token, Ipv4Addr, Server};

#[tokio::test(flavor = "multi_thread")]
async fn admin_responses_are_never_cached() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    let url = admin_url(&handle, "/status");
    let ok = ureq::get(&url)
        .set("X-Remote-User-Id", "aaaa")
        .call()
        .unwrap();
    let Err(ureq::Error::Status(403, denied)) = ureq::get(&url).call() else {
        panic!("expected forbidden");
    };
    for response in [ok, denied] {
        assert_eq!(response.header("Cache-Control"), Some("no-store"));
        assert_eq!(response.header("Pragma"), Some("no-cache"));
        assert_eq!(response.header("X-Frame-Options"), Some("SAMEORIGIN"));
        assert!(response
            .header("Content-Security-Policy")
            .unwrap()
            .contains("frame-ancestors 'self'"));
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_csrf_token_is_bound_to_the_user() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    let first = csrf_token(&handle, "aaaa");
    assert_ne!(first, csrf_token(&handle, "bbbb"));
    assert_eq!(first, csrf_token(&handle, "aaaa"));
    let url = admin_url(&handle, "/connections");
    let cookie = format!("pensine_csrf={first}");
    let borrowed = ureq::post(&url)
        .set("X-Remote-User-Id", "bbbb")
        .set("Cookie", &cookie)
        .send_form(&[("csrf", &first), ("name", "Sample connection")]);
    assert!(matches!(borrowed, Err(ureq::Error::Status(403, _))));
    let own = ureq::post(&url)
        .set("X-Remote-User-Id", "aaaa")
        .set(
            "Origin",
            &format!("http://{}", handle.admin_addr().unwrap()),
        )
        .set("Cookie", &cookie)
        .send_form(&[("csrf", &first), ("name", "Sample connection")]);
    assert_eq!(own.unwrap().status(), 200);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_site_admin_post_is_refused_with_a_readable_body() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    // Even with a valid token and cookie, a post the browser marks cross-site is refused...
    let token = csrf_token(&handle, "aaaa");
    let result = ureq::post(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Cookie", &format!("pensine_csrf={token}"))
        .set("Sec-Fetch-Site", "cross-site")
        .send_form(&[("name", "Owner"), ("csrf", &token)]);
    // ...with a text a person can read: an empty body would be downloaded as a file by the browser.
    match result {
        Err(ureq::Error::Status(403, response)) => {
            assert!(response.content_type().starts_with("text/plain"));
            assert!(response.into_string().unwrap().contains("Refused"));
        }
        other => panic!("expected a readable 403, got {other:?}"),
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_through_the_ingress_with_the_home_assistant_origin_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    // Behind the ingress the browser's Origin and Referer are Home Assistant's own address, never this server's.
    let token = csrf_token(&handle, "aaaa");
    let response = ureq::post(&admin_url(&handle, "/oauth"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Cookie", &format!("pensine_csrf={token}"))
        .set("Origin", "http://homeassistant.local:8123")
        .set(
            "Referer",
            "http://homeassistant.local:8123/hassio/ingress/sample/",
        )
        .set("Sec-Fetch-Site", "same-origin")
        .send_form(&[("csrf", &token), ("assistant", "claude"), ("scope", "all")])
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.content_type().starts_with("text/html"));
    assert!(response.into_string().unwrap().len() > 500);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_without_the_token_is_refused_with_a_readable_body() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    let result = ureq::post(&admin_url(&handle, "/oauth"))
        .set("X-Remote-User-Id", "aaaa")
        .send_form(&[("assistant", "claude"), ("scope", "all")]);
    match result {
        Err(ureq::Error::Status(403, response)) => {
            assert!(response.content_type().starts_with("text/plain"));
        }
        other => panic!("expected a readable 403, got {other:?}"),
    }
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_page_counts_only_the_signed_in_users_files() {
    use pensine_access::Accounts;
    use pensine_store::NewFile;

    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    for (user, bytes) in [("aaaa", &b"own"[..]), ("bbbb", &b"someone else's file"[..])] {
        let account = accounts.account_for_ha_user(user, "").unwrap();
        account.store.space_create("aa", "Notes", None).unwrap();
        account
            .store
            .file_put(NewFile {
                space: "aa".into(),
                name: "sample.txt".into(),
                mime: "text/plain".into(),
                reader: bytes,
                max_size: bytes.len() as u64,
                note: None,
            })
            .unwrap();
    }
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    let page = ureq::get(&admin_url(&handle, "/status"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Accept-Language", "en")
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(page.contains("Disk used: 3 bytes"), "{page}");
    handle.shutdown();
}
