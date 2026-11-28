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
async fn a_cross_origin_admin_post_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(admin_config(&dir, Ipv4Addr::LOCALHOST.into()))
        .await
        .unwrap();
    // An origin mismatch is rejected even before form extraction.
    let result = ureq::post(&admin_url(&handle, "/connections"))
        .set("X-Remote-User-Id", "aaaa")
        .set("Origin", "https://other.example")
        .call();
    assert!(matches!(result, Err(ureq::Error::Status(403, _))));
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
