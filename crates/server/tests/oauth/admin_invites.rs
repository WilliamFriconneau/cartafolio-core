use super::{body, Fixture, Url, VERIFIER};

async fn admin_invitation(scope: &str) -> (Fixture, String) {
    let env = Fixture::start().await;
    pensine_access::Accounts::open(env.dir.path())
        .unwrap()
        .link_ha_user("sample-user", "demo", "Sample User")
        .unwrap();
    let base = format!("http://{}", env.handle.admin_addr().unwrap());
    let page = ureq::get(&base)
        .set("X-Remote-User-Id", "sample-user")
        .call()
        .unwrap();
    let cookie = page
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let csrf = cookie.split_once('=').unwrap().1;
    let html = ureq::post(&format!("{base}/oauth"))
        .set("X-Remote-User-Id", "sample-user")
        .set("Cookie", cookie)
        .send_form(&[
            ("csrf", csrf),
            ("scope", scope),
            ("s_aa", "1"),
            ("read_only", "1"),
        ])
        .unwrap()
        .into_string()
        .unwrap();
    (env, html)
}

async fn invitation(scope: &str) -> (Fixture, String, String) {
    let (env, html) = admin_invitation(scope).await;
    assert!(html.contains("expires in 10 minutes") && html.contains("Connections and keys"));
    let invite = html
        .split("id=\"invitation-code\" readonly value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(invite.len(), 14);
    let client = env.client();
    let mut authorize = Url::parse(&env.url("/authorize")).unwrap();
    authorize
        .query_pairs_mut()
        .extend_pairs(env.authorize_fields(&client, "S256"));
    assert_eq!(
        env.agent.get(authorize.as_str()).call().unwrap().status(),
        200
    );
    let mut fields = env.authorize_fields(&client, "S256");
    fields.push(("invite", invite.into()));
    let consent = env.post("/authorize", &fields);
    assert_eq!(consent.status(), 200);
    (env, client, consent.into_string().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_consent_page_shows_the_rights_without_asking_again() {
    let (_, _, html) = invitation("selected").await;
    assert!(html.contains("Notes") && html.contains("Read only"));
    assert!(!html.contains("Other"));
    assert!(!html.contains("type=\"checkbox\""));
    let (_, _, html) = invitation("all").await;
    assert!(html.contains("All your spaces") && html.contains("Read only"));
    assert!(!html.contains("type=\"checkbox\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_full_flow_with_the_short_code_issues_a_token_and_calls_mcp() {
    let (env, client, html) = invitation("selected").await;
    let nonce = html
        .split("name=\"nonce\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let response = env.post(
        "/authorize",
        &[
            ("nonce", nonce.into()),
            ("decision", "allow".into()),
            ("v_bb", "1".into()),
        ],
    );
    assert_eq!(response.status(), 302);
    let callback = Url::parse(response.header("location").unwrap()).unwrap();
    let code = callback
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1;
    let tokens = body(env.exchange(&client, &code, VERIFIER));
    let token = tokens["access_token"].as_str().unwrap();
    let allowed = body(env.mcp(token, "aa"));
    let denied = body(env.mcp(token, "bb"));
    assert_ne!(allowed["result"]["isError"], true);
    assert_eq!(denied["result"]["isError"], true);
    let write = env
        .agent
        .post(&env.url("/mcp"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_string(
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "remember", "arguments": {"space": "aa", "text": "Sample"}}
            })
            .to_string(),
        )
        .unwrap();
    assert_eq!(body(write)["result"]["isError"], true);

    let base = format!("http://{}", env.handle.admin_addr().unwrap());
    let dashboard = ureq::get(&base)
        .set("X-Remote-User-Id", "sample-user")
        .call()
        .unwrap();
    let cookie = dashboard
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = cookie.split_once('=').unwrap().1;
    let html = dashboard.into_string().unwrap();
    assert!(html.contains("Assistants signed in with a code (1)"));
    let id = html
        .split("name=\"id\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let revoked = ureq::post(&format!("{base}/oauth/revoke"))
        .set("X-Remote-User-Id", "sample-user")
        .set("Cookie", &cookie)
        .send_form(&[("id", id), ("csrf", csrf)])
        .unwrap()
        .into_string()
        .unwrap();
    assert!(revoked.contains("Assistants signed in with a code (0)"));
    assert_eq!(env.mcp(token, "aa").status(), 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_admin_page_shows_the_code_with_a_copy_button_and_expiry() {
    let (_, html) = admin_invitation("all").await;
    assert!(html.contains("id=\"invitation-code\""));
    assert!(html.contains("user-select:all"));
    assert_eq!(html.matches(">Copy code</button>").count(), 1);
    assert!(html.contains("expires in 10 minutes"));
    assert!(html.contains("Type it on the page that opens in your assistant"));
    assert_eq!(html.matches("<script nonce=").count(), 1);
    assert!(html.contains("input.select()"));
}
