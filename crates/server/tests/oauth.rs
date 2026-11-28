use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use pensine_access::Accounts;
use pensine_server::{Config, Handle, Server};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

const ISSUER: &str = "https://memory.example.test";
const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyz0123456789-._~abcdef";

struct Fixture {
    handle: Handle,
    dir: tempfile::TempDir,
    agent: ureq::Agent,
}

impl Fixture {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(dir.path()).unwrap();
        let account = accounts.account_create("demo").unwrap();
        account.store.space_create("aa", "Notes", None).unwrap();
        account.store.space_create("bb", "Other", None).unwrap();
        let handle = Self::start_server(dir.path()).await;
        Self {
            handle,
            dir,
            agent: ureq::AgentBuilder::new().redirects(0).build(),
        }
    }

    async fn start_server(dir: &std::path::Path) -> Handle {
        Server::start(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.to_path_buf(),
            public_url: Some(ISSUER.into()),
            ..Config::default()
        })
        .await
        .unwrap()
    }

    async fn restart(&mut self) {
        self.handle.shutdown();
        self.handle = Self::start_server(self.dir.path()).await;
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.handle.local_addr())
    }

    fn register(&self, callback: &str) -> ureq::Response {
        self.agent
            .post(&self.url("/register"))
            .set("Content-Type", "application/json")
            .send_string(
                &json!({"client_name": "Connector", "redirect_uris": [callback]}).to_string(),
            )
            .unwrap_or_else(status_response)
    }

    fn client(&self) -> String {
        let response = self.register(CALLBACK);
        assert_eq!(response.status(), 201);
        let registration = body(response);
        assert_eq!(registration["token_endpoint_auth_method"], "none");
        registration["client_id"].as_str().unwrap().into()
    }

    fn authorize_fields<'a>(&self, client: &'a str, method: &'a str) -> Vec<(&'a str, String)> {
        vec![
            ("response_type", "code".into()),
            ("client_id", client.into()),
            ("redirect_uri", CALLBACK.into()),
            (
                "code_challenge",
                URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER)),
            ),
            ("code_challenge_method", method.into()),
            ("resource", format!("{ISSUER}/mcp")),
            ("state", "one & two".into()),
        ]
    }

    fn post(&self, path: &str, fields: &[(&str, String)]) -> ureq::Response {
        let fields: Vec<_> = fields
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        self.agent
            .post(&self.url(path))
            .send_form(&fields)
            .unwrap_or_else(status_response)
    }

    fn code(&self, client: &str) -> String {
        let invite = self.handle.oauth_invitation("demo").unwrap();
        let mut fields = self.authorize_fields(client, "S256");
        fields.push(("invite", invite.clone()));
        let response = self.post("/authorize", &fields);
        assert_eq!(response.status(), 200);
        assert_eq!(self.post("/authorize", &fields).status(), 403);
        let html = response.into_string().unwrap();
        assert!(html.contains("Notes") && html.contains("Other"));
        assert!(!html.contains("checked"));
        let nonce = html
            .split("name=\"nonce\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let response = self.post(
            "/authorize",
            &[
                ("nonce", nonce.into()),
                ("decision", "allow".into()),
                ("v_aa", "1".into()),
            ],
        );
        assert_eq!(response.status(), 302);
        let callback = Url::parse(response.header("Location").unwrap()).unwrap();
        let params: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
        assert_eq!(params["state"], "one & two");
        assert_eq!(params["iss"], ISSUER);
        params["code"].clone()
    }

    fn exchange(&self, client: &str, code: &str, verifier: &str) -> ureq::Response {
        self.post(
            "/token",
            &[
                ("grant_type", "authorization_code".into()),
                ("client_id", client.into()),
                ("redirect_uri", CALLBACK.into()),
                ("code", code.into()),
                ("code_verifier", verifier.into()),
            ],
        )
    }

    fn refresh(&self, client: &str, token: &str) -> ureq::Response {
        self.post(
            "/token",
            &[
                ("grant_type", "refresh_token".into()),
                ("client_id", client.into()),
                ("refresh_token", token.into()),
            ],
        )
    }

    fn mcp(&self, token: &str, space: &str) -> ureq::Response {
        self.agent
            .post(&self.url("/mcp"))
            .set("Authorization", &format!("Bearer {token}"))
            .send_string(
                &json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "recall", "arguments": {"query": "sample", "space": space}}
                })
                .to_string(),
            )
            .unwrap_or_else(status_response)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

fn status_response(error: ureq::Error) -> ureq::Response {
    match error {
        ureq::Error::Status(_, response) => response,
        other => panic!("HTTP transport failed: {other}"),
    }
}

fn body(response: ureq::Response) -> Value {
    serde_json::from_str(&response.into_string().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_full_oauth_flow_calls_mcp_with_the_selected_grants() {
    let env = Fixture::start().await;
    let unauthorized = env.mcp("unrecognized", "aa");
    assert_eq!(unauthorized.status(), 401);
    assert_eq!(
        unauthorized.header("WWW-Authenticate"),
        Some(
            format!("Bearer resource_metadata=\"{ISSUER}/.well-known/oauth-protected-resource\"")
                .as_str()
        ),
    );
    let protected = body(
        env.agent
            .get(&env.url("/.well-known/oauth-protected-resource"))
            .call()
            .unwrap(),
    );
    assert_eq!(protected["resource"], format!("{ISSUER}/mcp"));
    assert_eq!(protected["authorization_servers"], json!([ISSUER]));
    let metadata = body(
        env.agent
            .get(&env.url("/.well-known/oauth-authorization-server"))
            .call()
            .unwrap(),
    );
    assert_eq!(
        metadata["registration_endpoint"],
        format!("{ISSUER}/register")
    );
    assert_eq!(
        metadata["authorization_endpoint"],
        format!("{ISSUER}/authorize")
    );
    assert_eq!(metadata["token_endpoint"], format!("{ISSUER}/token"));
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );
    let client = env.client();
    let mut authorize = Url::parse(&env.url("/authorize")).unwrap();
    authorize
        .query_pairs_mut()
        .extend_pairs(env.authorize_fields(&client, "S256"));
    let page = env
        .agent
        .get(authorize.as_str())
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(page.contains("One-time code"));
    assert!(!page.contains("Notes"));
    let tokens = body(env.exchange(&client, &env.code(&client), VERIFIER));
    assert_eq!(tokens["token_type"], "Bearer");
    let db = rusqlite::Connection::open(env.dir.path().join("oauth.db")).unwrap();
    let (access_hash, refresh_hash, access_ttl, refresh_ttl): (Vec<u8>, Vec<u8>, i64, i64) = db.query_row(
        "SELECT access_hash, refresh_hash, access_until - unixepoch(), refresh_until - unixepoch() FROM oauth_sessions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(
        access_hash,
        Sha256::digest(tokens["access_token"].as_str().unwrap()).to_vec()
    );
    assert_eq!(
        refresh_hash,
        Sha256::digest(tokens["refresh_token"].as_str().unwrap()).to_vec()
    );
    assert!((3595..=3600).contains(&access_ttl));
    assert!((7_775_995..=7_776_000).contains(&refresh_ttl));
    assert_eq!(tokens["expires_in"], 3600);
    let access = tokens["access_token"].as_str().unwrap();
    assert_eq!(body(env.mcp(access, "aa"))["result"]["isError"], false);
    assert_eq!(body(env.mcp(access, "bb"))["result"]["isError"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_code_works_once() {
    let env = Fixture::start().await;
    let client = env.client();
    let code = env.code(&client);
    assert_eq!(env.exchange(&client, &code, VERIFIER).status(), 200);
    assert_eq!(
        body(env.exchange(&client, &code, VERIFIER))["error"],
        "invalid_grant"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pkce_is_required() {
    let env = Fixture::start().await;
    let client = env.client();
    assert_eq!(
        env.post("/authorize", &env.authorize_fields(&client, "plain"))
            .status(),
        400
    );
    let mut fields = env.authorize_fields(&client, "S256");
    fields.retain(|(name, _)| *name != "code_challenge");
    assert_eq!(env.post("/authorize", &fields).status(), 400);
    let code = env.code(&client);
    assert_eq!(
        body(env.exchange(&client, &code, ""))["error"],
        "invalid_grant"
    );
    assert_eq!(
        body(env.exchange(&client, &code, &"x".repeat(43)))["error"],
        "invalid_grant"
    );
    assert_eq!(env.exchange(&client, &code, VERIFIER).status(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_token_rotates() {
    let env = Fixture::start().await;
    let client = env.client();
    let first = body(env.exchange(&client, &env.code(&client), VERIFIER));
    let refresh = first["refresh_token"].as_str().unwrap();
    let second = body(env.refresh(&client, refresh));
    assert_ne!(first["refresh_token"], second["refresh_token"]);
    assert_eq!(
        env.mcp(first["access_token"].as_str().unwrap(), "aa")
            .status(),
        401
    );
    let access = second["access_token"].as_str().unwrap();
    assert_eq!(body(env.mcp(access, "aa"))["result"]["isError"], false);
    assert_eq!(body(env.mcp(access, "bb"))["result"]["isError"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_uri_not_registered_is_refused() {
    let env = Fixture::start().await;
    assert_eq!(env.register("http://example.test/callback").status(), 400);
    assert_eq!(env.register("http://127.0.0.1:8080/callback").status(), 201);
    let client = env.client();
    let mut fields = env.authorize_fields(&client, "S256");
    fields[2].1 = "https://unregistered.example.test/callback".into();
    let response = env.post("/authorize", &fields);
    assert_eq!(response.status(), 400);
    assert!(response.header("Location").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_code_is_refused() {
    let env = Fixture::start().await;
    let client = env.client();
    let code = env.code(&client);
    let invite = env.handle.oauth_invitation("demo").unwrap();
    let db = rusqlite::Connection::open(env.dir.path().join("oauth.db")).unwrap();
    db.execute("UPDATE oauth_codes SET expires_at = 0", [])
        .unwrap();
    db.execute("UPDATE oauth_invites SET expires_at = 0", [])
        .unwrap();
    assert_eq!(
        body(env.exchange(&client, &code, VERIFIER))["error"],
        "invalid_grant"
    );
    let mut fields = env.authorize_fields(&client, "S256");
    fields.push(("invite", invite));
    assert_eq!(env.post("/authorize", &fields).status(), 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reuse_of_a_rotated_refresh_token_revokes_the_family() {
    let env = Fixture::start().await;
    let client = env.client();
    let first = body(env.exchange(&client, &env.code(&client), VERIFIER));
    let unrelated = body(env.exchange(&client, &env.code(&client), VERIFIER));
    let old_refresh = first["refresh_token"].as_str().unwrap();
    let latest = body(env.refresh(&client, old_refresh));
    let latest_access = latest["access_token"].as_str().unwrap();
    let latest_refresh = latest["refresh_token"].as_str().unwrap();
    assert_eq!(env.mcp(latest_access, "aa").status(), 200);

    assert_eq!(
        body(env.refresh(&client, old_refresh))["error"],
        "invalid_grant"
    );
    assert_eq!(env.mcp(latest_access, "aa").status(), 401);
    assert_eq!(
        body(env.refresh(&client, latest_refresh))["error"],
        "invalid_grant"
    );
    assert_eq!(
        env.mcp(first["access_token"].as_str().unwrap(), "aa")
            .status(),
        401
    );
    assert_eq!(
        env.refresh(&client, unrelated["refresh_token"].as_str().unwrap())
            .status(),
        200
    );
}

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
