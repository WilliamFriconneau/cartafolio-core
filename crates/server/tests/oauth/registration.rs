use super::{body, Fixture, CALLBACK};

#[tokio::test(flavor = "multi_thread")]
async fn registrations_are_capped_and_expire() {
    let mut env = Fixture::start().await;
    env.handle.shutdown();
    let config = pensine_server::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: env.dir.path().into(),
        oauth_registration_cap: 2,
        ..Default::default()
    };
    env.handle = pensine_server::Server::start(config.clone()).await.unwrap();
    let first = env.client();
    let second = env.client();
    assert_eq!(env.register(CALLBACK).status(), 503);

    let db = rusqlite::Connection::open(env.dir.path().join("oauth.db")).unwrap();
    db.execute(
        "UPDATE oauth_clients SET last_used = unixepoch() - 31 * 86400 WHERE id = ?1",
        [&first],
    )
    .unwrap();
    assert_eq!(env.register(CALLBACK).status(), 201);
    let remaining: Vec<String> = db
        .prepare("SELECT id FROM oauth_clients")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(!remaining.contains(&first));
    assert!(remaining.contains(&second));

    for _ in 0..6 {
        assert_eq!(env.register(CALLBACK).status(), 503);
    }
    let limited = env.register(CALLBACK);
    assert_eq!(limited.status(), 429);
    assert!(limited.header("Retry-After").is_some());
    assert_eq!(body(limited)["error"], "registration_rate_limit");
    env.handle.shutdown();
    let smaller = pensine_server::Config {
        oauth_registration_cap: 1,
        ..config
    };
    env.handle = pensine_server::Server::start(smaller).await.unwrap();
    let count: usize = db
        .query_row("SELECT COUNT(*) FROM oauth_clients", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    env.handle.shutdown();
}
