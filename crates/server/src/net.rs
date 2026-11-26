//! The listeners: the main one, the identity one (behind Tailscale serve) and the ingress one
//! (Home Assistant), whatever answers behind them. Shared by the engine and the manager.

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::Failure;
use crate::http::{self, Request, Response};

/// Holds the node's data directory for the life of the server. Keep the open file until shutdown.
pub fn lock_node(data: &std::path::Path) -> Result<std::fs::File, Failure> {
    use std::fs::{OpenOptions, TryLockError};
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join(".node.lock"))
        .map_err(crate::cli::fail)?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err((
            1,
            format!("A node is already running in {}.", data.display()),
        )),
        Err(TryLockError::Error(e)) => Err(crate::cli::fail(e)),
    }
}

/// Whatever turns a request into a response: one engine, or the manager that holds several.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, req: &Request) -> Response;

    /// The biggest body this request may bring, decided **before** it is read, from its line and headers: only a request
    /// that carries a key of this installation may send a recording. `trusted`: it came through the ingress or the
    /// identity listener, whose visitor is known.
    fn body_limit(
        &self,
        _method: &str,
        _path: &str,
        _headers: &HashMap<String, String>,
        _trusted: bool,
    ) -> u64 {
        SMALL_MAX
    }

    /// A request from `ip` declared a body bigger than it may bring. Asking for a large body is what a valid key
    /// earns, so a refusal here is a failed attempt like any other and is counted.
    fn oversize(&self, _ip: &str) {}
}

/// What a request with no key of this installation may send: JSON, a form.
pub const SMALL_MAX: u64 = 256 * 1024;
/// What a request with a key may send: a 24 MB recording in base64.
pub const LARGE_MAX: u64 = READ_MAX;

/// Requests being read and answered by a thread of their own. A client that sends its body slowly holds only its own
/// thread, never a worker; past these numbers (in all, and from one address) a new request is turned away at once.
///
/// Two pools: a request with no key of this installation that still has a body to send (the only kind a stranger can
/// keep waiting at no cost) takes its place in a pool of its own, so that such requests, however many, never turn
/// away an assistant or a peer with its key, nor a request without a body.
static IN_FLIGHT: std::sync::Mutex<[Option<HashMap<String, usize>>; 2]> =
    std::sync::Mutex::new([None, None]);
const IN_FLIGHT_MAX: usize = 64;
const IN_FLIGHT_PER_ADDRESS: usize = 16;

/// Which pool a request takes its place in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pool {
    /// A request with a key, or with no body to wait for.
    Main = 0,
    /// A request with no key that has a body to send.
    Keyless = 1,
}

/// Takes a place for a request from `who`; `false` if there is none.
fn take_place(pool: Pool, who: &str) -> bool {
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    let m = g[pool as usize].get_or_insert_with(HashMap::new);
    let total: usize = m.values().sum();
    let mine = m.get(who).copied().unwrap_or(0);
    if total >= IN_FLIGHT_MAX || mine >= IN_FLIGHT_PER_ADDRESS {
        return false;
    }
    m.insert(who.to_string(), mine + 1);
    true
}

fn give_place(pool: Pool, who: &str) {
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = g[pool as usize].as_mut()
        && let Some(n) = m.get_mut(who)
    {
        *n = n.saturating_sub(1);
        if *n == 0 {
            m.remove(who);
        }
    }
}

/// Where to listen.
pub struct Listeners {
    pub listen: String,
    /// The identity listener (behind Tailscale serve only) and the one address believed besides
    /// the loopback.
    pub identity: Option<(String, Option<IpAddr>)>,
    /// The ingress listener (Home Assistant) and the one proxy address whose headers are believed.
    pub ingress: Option<(String, IpAddr)>,
    pub threads: usize,
}

/// 34 MiB: room for a 24 MB recording in base64 inside a request, and not a byte more: a body is held in memory.
const READ_MAX: u64 = 34 * 1024 * 1024;

/// Which listener a request came in on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Main,
    Ingress,
    Identity,
}

fn respond<H: Handler>(app: &Arc<H>, rq: tiny_http::Request, mode: Mode) {
    let url = rq.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let mut headers = HashMap::new();
    for h in rq.headers() {
        headers.insert(
            h.field.as_str().as_str().to_ascii_lowercase(),
            h.value.as_str().to_string(),
        );
    }
    let remote = rq.remote_addr().map(|a| a.ip());
    // Behind the local proxy, the client address is the **last** entry of X-Forwarded-For: the one the proxy added. The
    // first ones are whatever the client wrote, and believing them would let it pick an address for each guess.
    let ip = match (remote, headers.get("x-forwarded-for")) {
        (Some(r), Some(f)) if r.is_loopback() => f.rsplit(',').next().map(|s| s.trim().to_string()),
        (Some(r), _) => Some(r.to_string()),
        _ => None,
    };
    // Tailscale serve sets the login for tailnet visitors and marks Funnel requests;
    // a Funnel request is never a tailnet login, whatever its headers say.
    let tailnet_login = if headers.contains_key("tailscale-funnel-request") {
        None
    } else {
        headers.get("tailscale-user-login").cloned()
    };
    let method = rq.method().as_str().to_string();
    let limit = app
        .body_limit(&method, path, &headers, mode != Mode::Main)
        .min(READ_MAX);
    // A declared size over the limit is refused before a byte is read.
    if rq.body_length().is_some_and(|n| n as u64 > limit) {
        if limit <= SMALL_MAX {
            app.oversize(ip.as_deref().unwrap_or("?"));
        }
        refuse(
            rq,
            tiny_http::Response::from_string(r#"{"error":"body too large"}"#).with_status_code(413),
        );
        return;
    }
    let path = path.to_string();
    let query = query.to_string();
    let who = ip.clone().unwrap_or_else(|| "?".into());
    // Only a request with a key of this installation (an MCP call, a peer's sync, a file of the page) may bring a
    // large body: below that limit, one that still has a body to send goes to the keyless pool (the page's other
    // actions, small, with it).
    let has_body =
        headers.contains_key("transfer-encoding") || rq.body_length().is_some_and(|n| n > 0);
    let pool = if has_body && limit <= SMALL_MAX {
        Pool::Keyless
    } else {
        Pool::Main
    };
    if !take_place(pool, &who) {
        // Busy, not broken: say when to come back, so that a client waits instead of hammering.
        let mut busy = tiny_http::Response::empty(503);
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Retry-After"[..], &b"1"[..]) {
            busy.add_header(h);
        }
        refuse(rq, busy);
        return;
    }
    let app = Arc::clone(app);
    std::thread::spawn(move || {
        let mut rq = rq;
        let declared = rq.body_length();
        let mut body = Vec::new();
        // A keyless body has a deadline as a whole (twice the timeout of one read): sent a byte at a time, it would
        // otherwise hold its place for ever without ever being silent.
        let deadline = (pool == Pool::Keyless).then(|| Instant::now() + read_timeout() * 2);
        let read = read_body(rq.as_reader(), limit + 1, &mut body, deadline);
        let too_big = body.len() as u64 > limit;
        // A body that stopped before its end (the connection closed or failed, or fewer bytes than announced) is not a
        // request: what came may be whole JSON and still not be what the client meant to send.
        let cut = read.is_err() || declared.is_some_and(|n| body.len() < n);
        let resp = if too_big {
            http::Response::json(413, &serde_json::json!({ "error": "body too large" }))
        } else if cut {
            let late = read.as_ref().is_err_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                )
            });
            http::Response::json(
                if late { 408 } else { 400 },
                &serde_json::json!({ "error": "the body did not come whole" }),
            )
        } else {
            app.handle(&http::Request {
                method,
                path,
                query: http::parse_query(&query),
                headers,
                body,
                tailnet_login: (mode == Mode::Identity).then(|| tailnet_login.unwrap_or_default()),
                ip,
                via_ingress: mode == Mode::Ingress,
            })
        };
        if cut {
            // What is left of a body cut short is drained by the connection after the answer, at the client's pace:
            // the place is not held for that.
            give_place(pool, &who);
            send(rq, resp);
        } else {
            send(rq, resp);
            give_place(pool, &who);
        }
    });
}

/// How long a connection may stay silent before the node closes it: `MEMCP_READ_TIMEOUT_SECS`, 30 by default.
fn read_timeout() -> Duration {
    pensine_core::env::var("MEMCP_READ_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map_or(Duration::from_secs(30), Duration::from_secs)
}

/// Reads a body into `body`, at most `max` bytes, and gives up with `TimedOut` past `deadline`.
fn read_body(
    r: &mut dyn Read,
    max: u64,
    body: &mut Vec<u8>,
    deadline: Option<Instant>,
) -> std::io::Result<()> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        if deadline.is_some_and(|d| Instant::now() > d) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the body is too slow",
            ));
        }
        let room = usize::try_from(max.saturating_sub(body.len() as u64)).unwrap_or(usize::MAX);
        if room == 0 {
            return Ok(());
        }
        let take = room.min(buf.len());
        match r.read(&mut buf[..take]) {
            Ok(0) => return Ok(()),
            Ok(n) => body.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Answers a request turned away before its body is read, from a thread of its own: once answered, the request reads
/// what is left of the body it declared (so that the connection can carry the next one), and a client that never sends
/// it must hold only that thread, never a worker. Two such connections used to stop a node with two workers.
fn refuse<R: std::io::Read + Send + 'static>(
    rq: tiny_http::Request,
    response: tiny_http::Response<R>,
) {
    std::thread::spawn(move || {
        let _ = rq.respond(response);
    });
}

/// Whether a header may be written as it is: no line break or other control character (a value that came from a
/// stored file name or type must never be able to end the header and start another, or a second response).
pub fn header_is_safe(name: &str, value: &str) -> bool {
    let plain = |s: &str| !s.chars().any(|c| c.is_control() && c != '\t');
    !name.is_empty() && plain(name) && plain(value)
}

fn send(rq: tiny_http::Request, resp: http::Response) {
    if let Some(path) = &resp.file {
        // A file is handed over block by block, and gone once it is sent (or could not be).
        match std::fs::File::open(path) {
            Ok(f) => {
                let mut out = tiny_http::Response::from_file(f).with_status_code(resp.status);
                for (k, v) in &resp.headers {
                    if header_is_safe(k, v)
                        && let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes())
                    {
                        out.add_header(h);
                    }
                }
                let _ = rq.respond(out);
            }
            Err(_) => {
                let _ = rq.respond(tiny_http::Response::empty(500));
            }
        }
        let _ = std::fs::remove_file(path);
        return;
    }
    let mut out = tiny_http::Response::from_data(resp.body).with_status_code(resp.status);
    for (k, v) in resp.headers {
        if !header_is_safe(&k, &v) {
            continue;
        }
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            out.add_header(h);
        }
    }
    let _ = rq.respond(out);
}

/// Listens until the process ends. `banner` is what is printed once the listeners are open.
pub fn serve<H: Handler>(app: Arc<H>, l: &Listeners, banner: &str) -> Result<(), Failure> {
    tiny_http::set_read_timeout(Some(read_timeout()));
    let server = tiny_http::Server::http(&l.listen)
        .map_err(|e| (1, format!("cannot listen on {}: {e}", l.listen)))?;
    serve_server(app, server, l, banner)
}

/// Like [`serve`], on a socket that is already open (`l.listen` is only printed). A program that needs a free port binds
/// port 0 itself and hands the socket over, so no other program can take the port between the choice and the listening.
pub fn serve_on<H: Handler>(
    app: Arc<H>,
    listener: std::net::TcpListener,
    l: &Listeners,
    banner: &str,
) -> Result<(), Failure> {
    tiny_http::set_read_timeout(Some(read_timeout()));
    let server = tiny_http::Server::from_listener(listener, None)
        .map_err(|e| (1, format!("cannot listen on {}: {e}", l.listen)))?;
    serve_server(app, server, l, banner)
}

fn serve_server<H: Handler>(
    app: Arc<H>,
    server: tiny_http::Server,
    l: &Listeners,
    banner: &str,
) -> Result<(), Failure> {
    let server = Arc::new(server);
    let mut ingress_workers = vec![];
    // The identity listener (behind Tailscale serve only): the OAuth consent page and the
    // admin page, the visitor known by `Tailscale-User-Login`. Never put it behind Funnel.
    if let Some((identity, trusted)) = &l.identity {
        let srv = Arc::new(
            tiny_http::Server::http(identity.as_str())
                .map_err(|e| (1, format!("cannot listen on {identity}: {e}")))?,
        );
        let trusted = *trusted;
        eprintln!(
            "the consent page and the page of this node for the tailnet on http://{identity}"
        );
        for _ in 0..2 {
            let (srv, app) = (Arc::clone(&srv), Arc::clone(&app));
            ingress_workers.push(std::thread::spawn(move || {
                for rq in srv.incoming_requests() {
                    // Loopback is always believed; behind a proxy on another machine, only that one address.
                    let believed = rq
                        .remote_addr()
                        .is_some_and(|a| a.ip().is_loopback() || Some(a.ip()) == trusted);
                    if believed {
                        respond(&app, rq, Mode::Identity);
                    } else {
                        let _ = rq.respond(tiny_http::Response::empty(403));
                    }
                }
            }));
        }
    }
    // The ingress listener (Home Assistant add-on): only the admin page, and the login headers
    // are believed only from the trusted proxy address.
    if let Some((ingress, trusted)) = &l.ingress {
        let srv = Arc::new(
            tiny_http::Server::http(ingress.as_str())
                .map_err(|e| (1, format!("cannot listen on {ingress}: {e}")))?,
        );
        let trusted = *trusted;
        eprintln!("the page through ingress on http://{ingress} (trusted proxy {trusted})");
        for _ in 0..2 {
            let (srv, app) = (Arc::clone(&srv), Arc::clone(&app));
            ingress_workers.push(std::thread::spawn(move || {
                for rq in srv.incoming_requests() {
                    let from_proxy = rq.remote_addr().is_some_and(|a| a.ip() == trusted);
                    if from_proxy {
                        respond(&app, rq, Mode::Ingress);
                    } else {
                        let _ = rq.respond(tiny_http::Response::empty(403));
                    }
                }
            }));
        }
    }
    // The address really bound: the port is the system's when `listen` asks for port 0.
    eprintln!("{banner} listening on http://{}", server.server_addr());
    let mut workers = vec![];
    for _ in 0..l.threads.clamp(1, 16) {
        let (server, app) = (Arc::clone(&server), Arc::clone(&app));
        workers.push(std::thread::spawn(move || {
            for rq in server.incoming_requests() {
                respond(&app, rq, Mode::Main);
            }
        }));
    }
    for w in workers.into_iter().chain(ingress_workers) {
        let _ = w.join();
    }
    Ok(())
}
