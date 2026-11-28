use super::{config, Accounts, Server};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

fn request(
    addr: std::net::SocketAddr,
    path: &str,
    key: Option<&str>,
    body: Option<&str>,
) -> TcpStream {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.map_or(PING.len(), str::len)
    )
    .unwrap();
    if let Some(key) = key {
        write!(stream, "Authorization: Bearer {key}\r\n").unwrap();
    }
    if body.is_none() {
        write!(stream, "Expect: 100-continue\r\n").unwrap();
    }
    write!(stream, "\r\n{}", body.unwrap_or("")).unwrap();
    stream
}

fn status(stream: TcpStream) -> u16 {
    read_status(&mut BufReader::new(stream))
}

fn read_status(reader: &mut impl BufRead) -> u16 {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line.split_whitespace()
        .nth(1)
        .and_then(|word| word.parse().ok())
        .unwrap_or(0)
}

#[tokio::test]
async fn authentication_happens_before_the_body_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let handle = Server::start(config(&dir)).await.unwrap();
    for path in ["/mcp", "/mcp/demo"] {
        let stream = request(handle.local_addr(), path, None, None);
        assert_eq!(
            tokio::task::spawn_blocking(|| status(stream))
                .await
                .unwrap(),
            401
        );
    }
    handle.shutdown();
}

#[tokio::test]
async fn a_slow_body_is_cut_at_the_read_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts.account_create("demo").unwrap();
    let (_, key) = accounts.connection_create_owner("demo", "Owner").unwrap();
    let mut cfg = config(&dir);
    cfg.read_timeout = Duration::from_millis(150);
    let handle = Server::start(cfg).await.unwrap();
    for path in ["/mcp", "/mcp/demo"] {
        let stream = request(handle.local_addr(), path, Some(key.as_str()), None);
        tokio::task::spawn_blocking(move || {
            let mut reader = BufReader::new(stream);
            assert_eq!(read_status(&mut reader), 100);
            let mut end_of_headers = String::new();
            reader.read_line(&mut end_of_headers).unwrap();
            assert_eq!(read_status(&mut reader), 408);
        })
        .await
        .unwrap();
    }
    let stream = request(handle.local_addr(), "/mcp", Some(key.as_str()), Some(PING));
    assert_eq!(
        tokio::task::spawn_blocking(|| status(stream))
            .await
            .unwrap(),
        200
    );
    handle.shutdown();
}

#[tokio::test]
async fn admission_control_answers_503_beyond_the_cap_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    accounts.account_create("demo").unwrap();
    let (_, key) = accounts.connection_create_owner("demo", "Owner").unwrap();
    accounts.account_create("other").unwrap();
    let (_, other_key) = accounts.connection_create_owner("other", "Owner").unwrap();
    let cfg = pensine_server::Config {
        account_in_flight: 1,
        account_queue: 0,
        global_in_flight: 1,
        global_queue: 0,
        ..config(&dir)
    };
    let handle = Server::start(cfg).await.unwrap();
    let first = request(handle.local_addr(), "/mcp", Some(key.as_str()), None);
    let mut first = tokio::task::spawn_blocking(move || {
        let mut reader = BufReader::new(first);
        assert_eq!(read_status(&mut reader), 100);
        let mut end_of_headers = String::new();
        reader.read_line(&mut end_of_headers).unwrap();
        reader
    })
    .await
    .unwrap();
    for key in [&key, &other_key] {
        let second = request(handle.local_addr(), "/mcp", Some(key.as_str()), Some(PING));
        tokio::task::spawn_blocking(move || {
            let mut reader = BufReader::new(second);
            assert_eq!(read_status(&mut reader), 503);
            assert!(reader
                .lines()
                .map(Result::unwrap)
                .take_while(|line| !line.is_empty())
                .any(|line| line.eq_ignore_ascii_case("retry-after: 1")));
        })
        .await
        .unwrap();
    }
    first.get_mut().write_all(PING.as_bytes()).unwrap();
    assert_eq!(
        tokio::task::spawn_blocking(move || read_status(&mut first))
            .await
            .unwrap(),
        200
    );
    let next = request(handle.local_addr(), "/mcp", Some(key.as_str()), Some(PING));
    assert_eq!(
        tokio::task::spawn_blocking(|| status(next)).await.unwrap(),
        200
    );
    handle.shutdown();
}

#[tokio::test]
async fn a_timed_out_call_does_not_starve_the_next_one_for_the_account() {
    use pensine_mcp::Mcp;
    use pensine_server::KeysAuthenticator;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let accounts = Arc::new(Accounts::open(dir.path()).unwrap());
    let account = accounts.account_create("demo").unwrap();
    account.store.space_create("aa", "A", None).unwrap();
    let (_, key) = accounts.connection_create_owner("demo", "Owner").unwrap();
    let mut cfg = config(&dir);
    cfg.request_timeout = Duration::from_millis(200);
    let handle = Server::start_with(
        cfg,
        Arc::new(Mcp::new(accounts.clone())),
        vec![Arc::new(KeysAuthenticator::new(accounts))],
    )
    .await
    .unwrap();
    let locked = account.lock_operations().unwrap();
    let call = request(
        handle.local_addr(),
        "/mcp",
        Some(key.as_str()),
        Some(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"remember","arguments":{"space":"aa","text":"Expired capture"}}}"#,
        ),
    );
    let response = tokio::task::spawn_blocking(|| status(call)).await.unwrap();
    drop(locked);
    let next = request(handle.local_addr(), "/mcp", Some(key.as_str()), Some(PING));
    assert_eq!(
        tokio::task::spawn_blocking(|| status(next)).await.unwrap(),
        200
    );
    assert_eq!(response, 503);
    let notes = account
        .store
        .list(pensine_store::ListQuery {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert!(!notes
        .items
        .iter()
        .any(|note| note.text == "Expired capture"));
    handle.shutdown();
}
