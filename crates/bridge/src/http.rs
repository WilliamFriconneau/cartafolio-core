//! The gateway over HTTP, for a program on this computer that only speaks HTTP (an assistant's app, an internal
//! tool): `POST /mcp` with one JSON-RPC message (or a batch), answered as JSON. It listens **on this machine only**
//! (a loopback address is the one thing accepted), and refuses a request that a web page could have made: the
//! `Host` and the `Origin` must be this machine. There is no key to give: the bridge holds it, which is the point,
//! and the reason it never listens anywhere else.

use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;

use serde_json::{Value, json};

use crate::{Error, Gateway};

/// The biggest request taken: a file sent as text through a tool, at most.
const BODY_MAX: u64 = 40 * 1024 * 1024;

fn loopback_host(host: &str) -> bool {
    // `host` may carry a port: `127.0.0.1:8792`, `[::1]:8792`, `localhost:8792`.
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

fn loopback_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    rest.is_some_and(loopback_host)
}

/// The address to listen on, if it is one of this machine's.
fn loopback_addr(addr: &str) -> Result<SocketAddr, Error> {
    let fixed = addr.replacen("localhost", "127.0.0.1", 1);
    match fixed.parse::<SocketAddr>() {
        Ok(a) if a.ip().is_loopback() => Ok(a),
        _ => Err(Error::Invalid(format!(
            "{addr} is refused: the bridge listens on this machine only (127.0.0.1 or [::1])"
        ))),
    }
}

fn reply(request: tiny_http::Request, status: u16, body: &str) {
    let json = tiny_http::Header::from_bytes("Content-Type", "application/json");
    let mut r = tiny_http::Response::from_string(body).with_status_code(status);
    if let Ok(h) = json {
        r = r.with_header(h);
    }
    let _ = request.respond(r);
}

/// Serves `gateway` on `addr` until the process stops. One request at a time.
pub fn serve_http(gateway: &mut Gateway, addr: &str) -> Result<(), Error> {
    let server = bind(addr)?;
    serve_on(&server, gateway);
    Ok(())
}

fn bind(addr: &str) -> Result<tiny_http::Server, Error> {
    let addr = loopback_addr(addr)?;
    tiny_http::Server::http(addr).map_err(|e| Error::Io(format!("{addr}: {e}")))
}

/// The gateway served over HTTP on a thread of its own, for an app that keeps running: an assistant on this machine
/// reaches the memory through it and never holds the address or the key. Stops when dropped.
pub struct HttpServer {
    server: Option<Arc<tiny_http::Server>>,
    thread: Option<JoinHandle<()>>,
}

impl HttpServer {
    /// Listens on `addr` (this machine only) before returning, so a port already taken is said at once.
    pub fn start(mut gateway: Gateway, addr: &str) -> Result<HttpServer, Error> {
        let server = Arc::new(bind(addr)?);
        let serving = Arc::clone(&server);
        let thread = std::thread::Builder::new()
            .name("memcp-bridge-http".into())
            .spawn(move || serve_on(&serving, &mut gateway))
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(HttpServer {
            server: Some(server),
            thread: Some(thread),
        })
    }

    /// Stops listening, after the request being answered, and lets the port go.
    pub fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.unblock();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve_on(server: &tiny_http::Server, gateway: &mut Gateway) {
    for mut request in server.incoming_requests() {
        let header = |name: &str| {
            request
                .headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                .map(|h| h.value.as_str().to_string())
        };
        let (host, origin) = (header("host"), header("origin"));
        if host.as_deref().is_some_and(|h| !loopback_host(h))
            || origin.as_deref().is_some_and(|o| !loopback_origin(o))
        {
            reply(
                request,
                403,
                &json!({ "error": "this address answers this machine only" }).to_string(),
            );
            continue;
        }
        let path = request.url().split('?').next().unwrap_or("").to_string();
        if request.method() != &tiny_http::Method::Post
            || !(path == "/mcp" || path.starts_with("/mcp/"))
        {
            reply(request, 405, &json!({ "error": "POST /mcp" }).to_string());
            continue;
        }
        let mut body = String::new();
        if request
            .as_reader()
            .take(BODY_MAX + 1)
            .read_to_string(&mut body)
            .is_err()
            || body.len() as u64 > BODY_MAX
        {
            reply(
                request,
                413,
                &json!({ "error": "body unreadable or too big" }).to_string(),
            );
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&body) else {
            reply(
                request,
                400,
                &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})
                    .to_string(),
            );
            continue;
        };
        let answers: Vec<Value> = match &message {
            Value::Array(batch) => batch.iter().filter_map(|m| gateway.handle(m)).collect(),
            one => gateway.handle(one).into_iter().collect(),
        };
        match (&message, answers.as_slice()) {
            (_, []) => {
                let _ = request.respond(tiny_http::Response::empty(202));
            }
            (Value::Array(_), _) => reply(request, 200, &Value::Array(answers).to_string()),
            (_, [one]) => reply(request, 200, &one.to_string()),
            _ => reply(request, 200, &Value::Array(answers).to_string()),
        }
        gateway.after_reply();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{loopback_addr, loopback_host, loopback_origin};

    #[test]
    fn only_this_machine_is_accepted() {
        for ok in [
            "127.0.0.1:8792",
            "localhost:8792",
            "[::1]:8792",
            "127.0.0.1",
            "LOCALHOST",
        ] {
            assert!(loopback_host(ok), "{ok}");
        }
        for no in [
            "example.com",
            "10.0.0.20:8792",
            "evil.localhost.example.com",
            "0.0.0.0:80",
            "127.0.0.1.evil.com",
        ] {
            assert!(!loopback_host(no), "{no}");
        }
        assert!(loopback_origin("http://localhost:3000"));
        assert!(!loopback_origin("https://evil.example"));
        assert!(!loopback_origin("null"));
        assert!(
            loopback_addr("127.0.0.1:8792").is_ok()
                && loopback_addr("localhost:8792").is_ok()
                && loopback_addr("[::1]:8792").is_ok()
        );
        assert!(loopback_addr("0.0.0.0:8792").is_err());
        assert!(loopback_addr("10.0.0.20:8792").is_err());
    }

    /// A server started by an app answers on its port, says at once when the port is taken, and lets the port go
    /// when stopped.
    #[test]
    fn a_started_server_answers_and_stops() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("memcp-http-{}", std::process::id()));
        let gateway = || {
            crate::Gateway::new(crate::Config {
                url: "http://127.0.0.1:9/mcp/x".into(),
                key: "k".into(),
                outbox: dir.clone(),
                utc_offset_minutes: 0,
            })
            .expect("fixture")
        };
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture");
        let addr = probe.local_addr().expect("fixture").to_string();
        drop(probe);
        let mut server = super::HttpServer::start(gateway(), &addr).expect("fixture");
        assert!(super::HttpServer::start(gateway(), &addr).is_err());
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut tcp = std::net::TcpStream::connect(&addr).expect("fixture");
        write!(
            tcp,
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("fixture");
        let mut answer = String::new();
        tcp.read_to_string(&mut answer).expect("fixture");
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.contains(r#""result":{}"#), "{answer}");
        server.stop();
        let again = super::HttpServer::start(gateway(), &addr);
        assert!(again.is_ok());
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
