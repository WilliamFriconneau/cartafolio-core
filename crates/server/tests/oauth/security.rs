use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_wrong_codes_lock_the_invite() {
    let mut env = Fixture::start().await;
    let client = env.client();
    let invite = env.handle.oauth_invitation("demo").unwrap();
    let other_invite = env.handle.oauth_invitation("demo").unwrap();
    let mut fields = env.authorize_fields(&client, "S256");
    for attempt in 0..5 {
        let wrong = format!("{}wrong-{attempt}", &invite[..12]);
        fields.push(("invite", wrong));
        assert_eq!(env.post("/authorize", &fields).status(), 403);
        fields.pop();
        if attempt == 3 {
            env.restart().await;
        }
    }
    let mut other_fields = fields.clone();
    other_fields.push(("invite", other_invite));
    assert_eq!(env.post("/authorize", &other_fields).status(), 200);
    fields.push(("invite", invite));
    let locked_before_restart = env.post("/authorize", &fields).status();
    env.restart().await;
    let locked_after_restart = env.post("/authorize", &fields).status();
    assert_eq!([locked_before_restart, locked_after_restart], [403, 403]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_consent_page_escapes_every_value() {
    let env = Fixture::start().await;
    let client = "client<script>\"'&";
    let name = "name<script>\"'&";
    let redirect = "https://example.test/callback?next=<script>\"'&other=yes";
    let state = "state<script>\"'&";
    // Registration generates URL-safe IDs, so seed a client to exercise escaping of its ID.
    let db = rusqlite::Connection::open(env.dir.path().join("oauth.db")).unwrap();
    db.execute(
        "INSERT INTO oauth_clients (id, name, redirects) VALUES (?1, ?2, ?3)",
        (client, name, json!([redirect]).to_string()),
    )
    .unwrap();
    let mut fields = env.authorize_fields(client, "S256");
    fields[2].1 = redirect.into();
    fields[6].1 = state.into();
    let mut url = Url::parse(&env.url("/authorize")).unwrap();
    url.query_pairs_mut().extend_pairs(fields.clone());
    let get = env.agent.get(url.as_str()).call().unwrap();
    fields.push(("invite", env.handle.oauth_invitation("demo").unwrap()));
    let post = env.post("/authorize", &fields);
    for response in [get, post] {
        assert_eq!(response.status(), 200);
        let html = response.into_string().unwrap();
        for (raw, escaped) in [
            (client, "client&lt;script&gt;&quot;&#39;&amp;"),
            (name, "name&lt;script&gt;&quot;&#39;&amp;"),
            (
                redirect,
                "https://example.test/callback?next=&lt;script&gt;&quot;&#39;&amp;other=yes",
            ),
            (state, "state&lt;script&gt;&quot;&#39;&amp;"),
        ] {
            assert!(!html.contains(raw));
            assert!(html.contains(escaped), "missing {escaped}");
        }
        assert!(!html.contains("<script>"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_protected_resource_and_server_metadata_agree_on_the_issuer() {
    let env = Fixture::start().await;
    let get = |path| body(env.agent.get(&env.url(path)).call().unwrap());
    let resource = get("/.well-known/oauth-protected-resource");
    let metadata = get("/.well-known/oauth-authorization-server");
    assert_eq!(metadata["issuer"], ISSUER);
    assert_eq!(
        resource["authorization_servers"],
        json!([metadata["issuer"]])
    );
    assert_eq!(resource["resource"], format!("{ISSUER}/mcp"));
    for (key, path) in [
        ("authorization_endpoint", "/authorize"),
        ("token_endpoint", "/token"),
        ("registration_endpoint", "/register"),
    ] {
        assert_eq!(metadata[key], format!("{ISSUER}{path}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_and_register_answers_are_never_cached() {
    let env = Fixture::start().await;
    let client = env.client();
    let code = env.code(&client);
    let post_raw = |path, content_type, payload| {
        env.agent
            .post(&env.url(path))
            .set("Content-Type", content_type)
            .send_string(payload)
            .unwrap_or_else(status_response)
    };
    let invalid_query = env
        .agent
        .get(&env.url("/authorize?state=%FF"))
        .call()
        .unwrap_or_else(status_response);
    for (case, status, response) in [
        ("token success", 200, env.exchange(&client, &code, VERIFIER)),
        ("token error", 400, env.exchange(&client, &code, VERIFIER)),
        (
            "token extractor rejection",
            415,
            post_raw("/token", "text/plain", ""),
        ),
        ("register success", 201, env.register(CALLBACK)),
        (
            "register error",
            400,
            env.register("http://example.test/callback"),
        ),
        (
            "register extractor rejection",
            400,
            post_raw("/register", "application/json", "{"),
        ),
        (
            "authorize GET validation error",
            400,
            env.agent
                .get(&env.url("/authorize"))
                .call()
                .unwrap_or_else(status_response),
        ),
        ("authorize GET extractor rejection", 400, invalid_query),
        (
            "authorize POST validation error",
            400,
            env.post("/authorize", &[]),
        ),
        (
            "authorize POST extractor rejection",
            415,
            post_raw("/authorize", "text/plain", ""),
        ),
    ] {
        assert_eq!(response.status(), status, "{case}");
        assert!(response.header("Location").is_none(), "{case}");
        assert_eq!(response.header("Cache-Control"), Some("no-store"), "{case}");
    }
}
