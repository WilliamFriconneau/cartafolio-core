//! The listeners: the main one, the identity one (behind Tailscale serve) and the ingress one
//! (Home Assistant), whatever answers behind them. Shared by the engine and the manager.

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::sync::Arc;

use crate::cli::Failure;
use crate::http::{self, Request, Response};

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
}

/// What a request with no key of this installation may send: JSON, a form.
pub const SMALL_MAX: u64 = 256 * 1024;
/// What a request with a key may send: a 24 MB recording in base64.
pub const LARGE_MAX: u64 = READ_MAX;

/// Requests being read and answered by a thread of their own. A client that sends its body slowly holds only its own
/// thread, never a worker; past these numbers (in all, and from one address) a new request is turned away at once.
static IN_FLIGHT: std::sync::Mutex<Option<HashMap<String, usize>>> = std::sync::Mutex::new(None);
const IN_FLIGHT_MAX: usize = 64;
const IN_FLIGHT_PER_ADDRESS: usize = 16;

/// Takes a place for a request from `who`; `false` if there is none.
fn take_place(who: &str) -> bool {
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let total: usize = m.values().sum();
    let mine = m.get(who).copied().unwrap_or(0);
    if total >= IN_FLIGHT_MAX || mine >= IN_FLIGHT_PER_ADDRESS {
        return false;
    }
    m.insert(who.to_string(), mine + 1);
    true
}

fn give_place(who: &str) {
    let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = g.as_mut()
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

/// 34 MB: room for a 24 MB recording in base64 inside a request.
const READ_MAX: u64 = 1024 * 1024 * 1024;

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
        let _ = rq.respond(
            tiny_http::Response::from_string(r#"{"error":"body too large"}"#).with_status_code(413),
        );
        return;
    }
    let path = path.to_string();
    let query = query.to_string();
    let who = ip.clone().unwrap_or_else(|| "?".into());
    if !take_place(&who) {
        let _ = rq.respond(tiny_http::Response::empty(503));
        return;
    }
    let app = Arc::clone(app);
    std::thread::spawn(move || {
        let mut rq = rq;
        let mut body = Vec::new();
        let _ = rq.as_reader().take(limit + 1).read_to_end(&mut body);
        let too_big = body.len() as u64 > limit;
        let resp = if too_big {
            http::Response::json(413, &serde_json::json!({ "error": "body too large" }))
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
        send(rq, resp);
        give_place(&who);
    });
}

fn send(rq: tiny_http::Request, resp: http::Response) {
    let mut out = tiny_http::Response::from_data(resp.body).with_status_code(resp.status);
    for (k, v) in resp.headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            out.add_header(h);
        }
    }
    let _ = rq.respond(out);
}

/// Listens until the process ends. `banner` is what is printed once the listeners are open.
pub fn serve<H: Handler>(app: Arc<H>, l: &Listeners, banner: &str) -> Result<(), Failure> {
    let server = Arc::new(
        tiny_http::Server::http(&l.listen)
            .map_err(|e| (1, format!("cannot listen on {}: {e}", l.listen)))?,
    );
    let mut ingress_workers = vec![];
    // The identity listener (behind Tailscale serve only): the OAuth consent page and the
    // admin page, the visitor known by `Tailscale-User-Login`. Never put it behind Funnel.
    if let Some((identity, trusted)) = &l.identity {
        let srv = Arc::new(
            tiny_http::Server::http(identity.as_str())
                .map_err(|e| (1, format!("cannot listen on {identity}: {e}")))?,
        );
        let trusted = *trusted;
        eprintln!("consent and admin pages for the tailnet on http://{identity}");
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
        eprintln!("admin page through ingress on http://{ingress} (trusted proxy {trusted})");
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
    eprintln!("{banner} listening on http://{}", l.listen);
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
