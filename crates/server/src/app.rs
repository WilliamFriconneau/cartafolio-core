//! The server without a socket: a request comes in, a response goes out. Routes
//! for MCP. Everything is testable without a network.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::http::bearer_of;
use memcp_core::access::{self, Caller};
use memcp_core::catalogue::Feature;
use memcp_core::guide;
use memcp_core::tools::{self, ToolDef, ToolError};
use memcp_core::{Brain, Space, util};
use serde_json::{Value, json};

use crate::http::{Request, Response};

pub const NAME: &str = "memcp";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// MCP protocol revisions understood, newest first.
pub const SUPPORTED: [&str; 4] = ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"];
/// Room for a 4 MB file in base64 inside a JSON-RPC request.
/// A recording of 24 MB, in base64 (a third more), and its envelope.
const MCP_BODY_MAX: usize = 34 * 1024 * 1024;
const FAIL_WINDOW: i64 = 900;
const MAX_FAILS_PER_IP: usize = 10;
const MAX_CONSOLE_FAILURES: usize = 10;
const MAX_FAILS_HELD: usize = 5000;
const READ_WINDOW: i64 = 30 * 60;
const READ_RETENTION: i64 = 24 * 60 * 60;
const MAX_RECENT_READS: usize = 4096;
const READ_FIRST: &str = "Read first. Call `recall` with the topic of what you are about to write, read what it returns, then call this tool again: write only what the memory does not already say, and correct a fact with `supersede`, never with a second note.";

pub struct App {
    pub brain: Brain,
    /// Browser origins accepted for MCP (besides the loopback).
    pub allowed_origins: Vec<String>,
    /// Home Assistant logins that are administrators (ingress only).
    pub ingress_admins: Vec<String>,
    /// A Home Assistant account the installation does not know yet is let in at once, with a
    /// person of its own, instead of waiting for an administrator.
    pub auto_create_people: bool,
    /// The admin page and its API on the main listener, behind a console token. Off unless
    /// the server listens on the loopback or `--console-admin` is given: the main listener
    /// is the one that can be exposed to the internet.
    pub console_admin: bool,
    /// The public address shown in connection instructions.
    pub public_url: String,
    /// How this node calls itself when it advertises and answers `/.well-known/memcp`.
    pub node_name: String,
    /// Where the OAuth consent page is reached (the identity listener's address).
    pub login_url: Option<String>,
    /// Redirect addresses OAuth clients may register.
    pub oauth_redirects: Vec<String>,
    /// Tailnet login (lowercase) → person, for the identity listener.
    pub tailnet_users: std::collections::HashMap<String, String>,
    /// Connections subject to the MCP read-before-write rule.
    pub strict_write_agents: Vec<String>,
    last_reads: Mutex<HashMap<String, i64>>,
    fails: Mutex<VecDeque<(i64, String)>>,
    rates: Mutex<HashMap<String, VecDeque<i64>>>,
    /// Pending OAuth approvals, keyed by their one-time nonce.
    pub(crate) consents: Mutex<HashMap<String, crate::oauth::Consent>>,
    /// One-time invitation codes for the consent page, for a person who is not on the tailnet: hash of the code to
    /// (person id, end of validity). In memory only: a restart forgets them, and a code is made just before it is used.
    pub(crate) invites: Mutex<HashMap<String, (String, i64)>>,
    /// Requests to join, waiting for the person's answer (in memory only).
    pub(crate) joins: Mutex<HashMap<String, crate::join::Join>>,
    /// One-time invitation tickets made by a peer or by the person: hash of the ticket to (person id, person name, end
    /// of validity).
    pub(crate) tickets: Mutex<HashMap<String, (String, String, i64)>>,
    /// The `memcp-sync` programs this node started (joining a memory, keeping in step with a peer).
    pub(crate) children: Mutex<Vec<std::process::Child>>,
    /// The peers whose sync was started (by name).
    pub(crate) synced: Mutex<std::collections::HashSet<String>>,
    /// This node takes and gives big files in chunks, and says so to its peers (`chunks` in `GET /sync/spaces`). Off only
    /// to stand for a node of an earlier version, which takes every file whole inside its event (the tests).
    pub file_chunks: bool,
    /// This node's id, read once (`GET /.well-known/memcp` is answered often and must not open the control database).
    node_id: std::sync::OnceLock<String>,
}

fn bearer(req: &Request) -> Option<String> {
    req.key()
}

pub(crate) fn rpc_ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub(crate) fn rpc_err(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = json!({ "code": code, "message": message });
    if let Some(d) = data {
        e["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": e })
}

fn loopback_origin(origin: &str) -> bool {
    [
        "http://127.0.0.1",
        "http://localhost",
        "http://[::1]",
        "https://127.0.0.1",
        "https://localhost",
    ]
    .iter()
    .any(|p| origin == *p || origin.starts_with(&format!("{p}:")))
}

/// The address a node is reached at when no `--public-url` says it: the one it listens on, over http (a wildcard host
/// is reached on 127.0.0.1). `None` for a port the system picks (`:0`) or an address that is not one.
pub fn listened_url(listen: &str) -> Option<String> {
    if let Ok(a) = listen.parse::<std::net::SocketAddr>() {
        if a.port() == 0 {
            return None;
        }
        let host = match a.ip() {
            ip if ip.is_unspecified() => "127.0.0.1".to_string(),
            std::net::IpAddr::V4(v4) => v4.to_string(),
            std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        };
        return Some(format!("http://{host}:{}", a.port()));
    }
    let (host, port) = listen.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    if host.is_empty() || host.contains(['/', ' ']) {
        return None;
    }
    Some(format!("http://{host}:{port}"))
}

impl App {
    pub fn new(brain: Brain) -> App {
        let node_name = memcp_core::directory::own_name(&brain).unwrap_or_else(|e| {
            eprintln!("node name: {e}");
            memcp_core::directory::DEFAULT_NODE_NAME.into()
        });
        Self::with_node_name(brain, node_name)
    }

    pub fn new_with_name(brain: Brain, name: Option<&str>) -> memcp_core::Result<App> {
        if let Some(name) = name {
            memcp_core::directory::set_own_name(&brain, name)?;
        }
        let node_name = memcp_core::directory::own_name(&brain)?;
        Ok(Self::with_node_name(brain, node_name))
    }

    fn with_node_name(brain: Brain, node_name: String) -> App {
        App {
            brain,
            allowed_origins: vec![],
            ingress_admins: vec![],
            auto_create_people: false,
            console_admin: false,
            public_url: "http://127.0.0.1:8791".into(),
            login_url: None,
            oauth_redirects: crate::oauth::DEFAULT_REDIRECTS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            tailnet_users: std::collections::HashMap::new(),
            strict_write_agents: vec![],
            last_reads: Mutex::new(HashMap::new()),
            fails: Mutex::new(VecDeque::new()),
            rates: Mutex::new(HashMap::new()),
            node_name,
            consents: Mutex::new(HashMap::new()),
            invites: Mutex::new(HashMap::new()),
            joins: Mutex::new(HashMap::new()),
            tickets: Mutex::new(HashMap::new()),
            children: Mutex::new(Vec::new()),
            synced: Mutex::new(std::collections::HashSet::new()),
            file_chunks: true,
            node_id: std::sync::OnceLock::new(),
        }
    }

    /// The space this caller reaches whose kind carries a built-in set of tools: the one its
    /// person owns if there is one, else the first it can read. `None` hides those tools from
    /// a connection that reaches no such space.
    fn feature_space(&self, caller: &Caller, feature: Feature) -> Option<Space> {
        let catalogue = self.brain.catalogue().ok()?;
        let ctl = self.brain.open_control().ok()?;
        catalogue
            .iter()
            .filter(|k| k.feature == Some(feature))
            .find_map(|k| access::space_of_kind(&ctl, caller, &k.key).ok().flatten())
    }

    pub fn handle(&self, req: &Request) -> Response {
        let now = util::now();
        let path = req.path.trim_end_matches('/');
        if req.tailnet_login.is_some() {
            // The identity listener: the OAuth consent page and the admin page, nothing else.
            return match (req.method.as_str(), path) {
                ("GET" | "POST", "/oauth/authorize") => crate::oauth::authorize(self, req),
                ("GET", "" | "/admin") => crate::admin::page(),
                (_, p) if p.starts_with("/admin/api/") => crate::admin::handle(self, req, now),
                _ => Response::json(404, &json!({ "error": "not found" })),
            };
        }
        if req.via_ingress {
            // The ingress listener serves the admin page and its API, nothing else.
            return match (req.method.as_str(), path) {
                ("GET", "" | "/admin") => crate::admin::page(),
                (_, p) if p.starts_with("/admin/api/") => crate::admin::handle(self, req, now),
                _ => Response::json(404, &json!({ "error": "not found" })),
            };
        }
        match (req.method.as_str(), path) {
            ("GET", p) if p.starts_with("/.well-known/oauth-protected-resource/mcp/") => {
                match address_segment(&p["/.well-known/oauth-protected-resource/mcp/".len()..]) {
                    Some(a) => crate::oauth::protected_resource(self, a),
                    None => Response::json(404, &json!({ "error": "not found" })),
                }
            }
            ("GET", "/.well-known/oauth-authorization-server") => {
                crate::oauth::authorization_server(self)
            }
            ("POST", "/oauth/register") => crate::oauth::register(self, req),
            ("POST", "/oauth/token") => crate::oauth::token(self, req),
            ("POST", "/pair") => crate::pair::exchange(self, req),
            ("GET", "/.well-known/pensine" | "/.well-known/silice" | "/.well-known/memcp") => {
                Response::json(
                    200,
                    &match self.own_node_id() {
                        Some(id) => crate::discovery::well_known_with_id(&self.node_name, id),
                        None => crate::discovery::well_known(&self.node_name),
                    },
                )
            }
            ("POST", "/join") => crate::join::request(self, req),
            ("GET", p) if p.starts_with("/join/") => {
                crate::join::poll(self, req, &p["/join/".len()..])
            }
            // The consent page: an invitation code opens it, here as on the identity listener.
            ("GET" | "POST", "/oauth/authorize") => crate::oauth::authorize(self, req),
            // The control plane is never served to a request that came through Funnel.
            ("GET", "/admin") if self.console_admin && !through_funnel(req) => crate::admin::page(),
            (_, p)
                if p.starts_with("/admin/api/") && self.console_admin && !through_funnel(req) =>
            {
                crate::admin::handle(self, req, now)
            }
            // There is no address for "the whole memory": every assistant has its own, named after
            // its connection (`/mcp/<name>`), and what it reaches is what its key allows.
            (_, "/mcp") => Response::json(
                404,
                &json!({ "error": "no address here: use the address of your connection, /mcp/<its name>" }),
            ),
            // An address that names a vault: a key used there reaches that vault and no other.
            // One that names a connection: only that connection's key is let in.
            // The synchronisation between peers, at the address of the peer's own connection.
            // A peer invites another device of the same person: a one-time ticket (see join.rs).
            ("POST", p) if p.starts_with("/mcp/") && p.ends_with("/join/invite") => {
                let agent = &p["/mcp/".len()..p.len() - "/join/invite".len()];
                if access::valid_agent(agent) {
                    crate::join::invite(self, req, now, agent)
                } else {
                    Response::json(404, &json!({ "error": "not found" }))
                }
            }
            // Every peer of the person sees and answers the devices asking to join (see join::by_peer).
            (_, p)
                if p.starts_with("/mcp/")
                    && p.contains("/join/")
                    && !p.ends_with("/join/invite") =>
            {
                let rest = &p["/mcp/".len()..];
                match rest.split_once("/join/") {
                    Some((agent, op))
                        if access::valid_agent(agent)
                            && matches!(op, "pending" | "approve" | "deny") =>
                    {
                        crate::join::by_peer(self, req, now, agent, op)
                    }
                    _ => Response::json(404, &json!({ "error": "not found" })),
                }
            }
            (_, p) if p.starts_with("/mcp/") && p.contains("/sync/") => {
                let rest = &p["/mcp/".len()..];
                match rest.split_once("/sync/") {
                    Some((agent, op)) if access::valid_agent(agent) && !op.contains('/') => {
                        crate::sync_http::handle(self, req, now, agent, op)
                    }
                    _ => Response::json(404, &json!({ "error": "not found" })),
                }
            }
            (_, p) if p.starts_with("/mcp/") => match address_segment(&p["/mcp/".len()..]) {
                Some(a) => self.mcp(req, now, a),
                None => Response::json(404, &json!({ "error": "not found" })),
            },
            ("GET", "/health") => Response::json(200, &json!({ "ok": true })),
            _ => Response::json(404, &json!({ "error": "not found" })),
        }
    }

    // --- brute-force guard (MCP) -----------------------------------------------------

    /// Who a failure is counted against: an IPv6 client holds a whole /64, so the address is cut to it (otherwise one
    /// machine tries a different address for every guess).
    pub(crate) fn guard_key(who: &str) -> String {
        match who.parse::<std::net::Ipv6Addr>() {
            Ok(a) => {
                let s = a.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
            Err(_) => who.to_string(),
        }
    }

    pub(crate) fn invite_guard_key(req: &Request) -> String {
        format!(
            "invite:{}",
            Self::guard_key(req.ip.as_deref().unwrap_or("?"))
        )
    }

    pub fn console_guard_key(req: &Request) -> String {
        format!(
            "console:{}",
            Self::guard_key(req.ip.as_deref().unwrap_or("?"))
        )
    }

    /// Failures held at the moment (bounded).
    pub fn failures_held(&self) -> usize {
        self.fails.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn console_blocked(&self, now: i64, key: &str) -> bool {
        self.blocked_with_limit(now, key, MAX_CONSOLE_FAILURES)
    }

    pub fn blocked(&self, now: i64, who: &str) -> bool {
        self.blocked_with_limit(now, who, MAX_FAILS_PER_IP)
    }

    pub(crate) fn blocked_with_limit(&self, now: i64, who: &str, limit: usize) -> bool {
        let who = Self::guard_key(who);
        let mut q = self.fails.lock().unwrap_or_else(|e| e.into_inner());
        while q.front().is_some_and(|(t, _)| now - t > FAIL_WINDOW) {
            q.pop_front();
        }
        q.iter().filter(|(_, w)| *w == who).count() >= limit
    }

    pub fn fail(&self, now: i64, who: &str) {
        let mut q = self.fails.lock().unwrap_or_else(|e| e.into_inner());
        // A flood from many addresses cannot grow the list, nor slow every check, without bound.
        if q.len() >= MAX_FAILS_HELD {
            q.pop_front();
        }
        q.push_back((now, Self::guard_key(who)));
    }

    pub fn rate_ok(&self, now: i64, key: &str, per_min: i64) -> bool {
        let mut m = self.rates.lock().unwrap_or_else(|e| e.into_inner());
        // Addresses seen once are forgotten: the table never grows past what a minute holds.
        if m.len() > 10_000 {
            m.retain(|_, q| q.back().is_some_and(|t| now - t < 60));
        }
        let q = m.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now - t >= 60) {
            q.pop_front();
        }
        if i64::try_from(q.len()).unwrap_or(i64::MAX) >= per_min {
            return false;
        }
        q.push_back(now);
        true
    }

    // --- MCP -------------------------------------------------------------------------

    /// The bank statements of the money space this caller reaches, if any.
    /// A key limited to paths of that vault gets none: the statements are not under a path, they are the vault.
    fn ledger_for(&self, caller: &Caller) -> Option<memcp_finances::Ledger<'_>> {
        self.feature_space(caller, Feature::Money)
            .filter(|space| caller.read_paths(space).is_none())
            .map(|space| memcp_finances::Ledger {
                brain: &self.brain,
                space,
            })
    }

    /// `space_view`: the core's read model, plus the money projection when the space is the one
    /// this caller reaches and the view asks for it. The core has already checked the rights and
    /// written the audit row; a node without the finance ledger simply answers the core's view.
    pub(crate) fn space_view(
        &self,
        caller: &Caller,
        args: &Value,
        _now: i64,
    ) -> Result<Value, ToolError> {
        let view = args
            .get("view")
            .and_then(Value::as_str)
            .unwrap_or("capabilities");
        let typ = args.get("type").and_then(Value::as_str).unwrap_or("note");
        let space = args.get("space").and_then(Value::as_str);
        // A chronology cursor for a money space is `sort_at|source|id`; give the core the fact part
        // only, and let the merge below order the facts and the ledger's movements together.
        let mut core_args = args.clone();
        if view == "chronology" {
            core_args["limit"] = json!(200);
        }
        if view == "chronology"
            && let Some(cursor) = args.get("cursor").and_then(Value::as_str)
            && let Some((at, source, id)) = cursor
                .split_once('|')
                .and_then(|(a, rest)| rest.split_once('|').map(|(s, i)| (a, s, i)))
        {
            // The core keeps the facts up to the cursor's day; the merge below removes the cursor
            // itself and everything newer. No epoch conversion, and the movement cursor is the
            // ledger's own id.
            let _ = (source, id);
            core_args.as_object_mut().map(|o| o.remove("cursor"));
            if let Some(day) = at.get(..10) {
                core_args["to"] = json!(day);
            }
        }
        let mut base = tools::call(&self.brain, caller, "space_view", &core_args)?;
        let Some(ledger) = self.ledger_for(caller) else {
            return Ok(base);
        };
        if space != Some(ledger.space.as_str()) {
            return Ok(base);
        }
        if view == "chronology" {
            if typ == "movement" {
                let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
                let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
                let mut page = ledger.movements_page(
                    s("account"),
                    s("from"),
                    s("to"),
                    s("filter"),
                    limit,
                    s("cursor"),
                )?;
                page["view"] = json!("chronology");
                return Ok(page);
            }
            if typ == "all" || args.get("type").is_none() {
                return crate::view::merge_chronology(&ledger, base, args);
            }
        }
        if view == "register" && typ == "movement" {
            let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
            return ledger.movements_page(
                s("account"),
                s("from"),
                s("to"),
                s("filter"),
                limit,
                s("cursor"),
            );
        }
        if view == "capabilities" {
            base = crate::view::augment(&ledger, base);
        } else if view == "stats" {
            base = crate::view::augment_stats(&ledger, base, util::now())?;
        }
        Ok(base)
    }

    fn tool_defs(&self, caller: &Caller) -> Vec<ToolDef> {
        let mut defs = tools::definitions(caller);
        if let Some(l) = self.ledger_for(caller) {
            defs.extend(l.definitions(caller));
        }
        defs
    }

    fn mcp(&self, req: &Request, now: i64, address: &str) -> Response {
        if req.method != "POST" {
            return Response::empty(405).with("Allow", "POST");
        }
        let origin = req.header("origin");
        if !origin.is_empty()
            && !loopback_origin(origin)
            && !self.allowed_origins.iter().any(|o| o == origin)
        {
            return Response::json(403, &json!({ "error": "origin not allowed" }));
        }
        let who = req.ip.clone().unwrap_or_else(|| "?".into());
        if self.blocked(now, &who) {
            return Response::json(429, &json!({ "error": "too many attempts" }))
                .with("Retry-After", "900");
        }
        let Ok(ctl) = self.brain.open_control() else {
            return Response::json(500, &json!({ "error": "storage failed" }));
        };
        let Some(mut caller) =
            bearer(req).and_then(|t| access::authenticate(&ctl, &t, now).ok().flatten())
        else {
            self.fail(now, &who);
            access::auth_event(&ctl, None, req.ip.as_deref(), "mcp_refused", "");
            return Response::json(401, &json!({ "error": "refused" }))
                .with("WWW-Authenticate", &crate::oauth::challenge(self, address));
        };
        caller.local = from_this_machine(req);
        let _ = access::touch(&ctl, &caller, client_seen(req).as_deref(), now);
        // At an address that names a vault, the key is seen through that vault alone; one that
        // does not reach it is told so, the same whether the vault exists or not. At an address
        // that names a connection, only that connection's own key is let in.
        let vault = self
            .brain
            .spaces()
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.as_str() == address));
        let caller = match vault {
            Some(v) if caller.can_read(&v) || caller.can_write(&v) => caller.only(&v),
            None if caller.agent == address => caller,
            // The same answer whether a vault of that name exists or not: a key learns no other person's vault names.
            _ => {
                return Response::json(
                    403,
                    &json!({ "error": "this key does not reach this address" }),
                );
            }
        };
        if req.body.len() > MCP_BODY_MAX {
            return Response::json(413, &json!({ "error": "body too large" }));
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
            return Response::json(400, &rpc_err(&Value::Null, -32700, "Parse error", None));
        };
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return Response::json(400, &rpc_err(&Value::Null, -32600, "Invalid Request", None));
        };
        if msg.get("jsonrpc") != Some(&json!("2.0")) {
            return Response::json(400, &rpc_err(&Value::Null, -32600, "Invalid Request", None));
        }
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        // Revision 2026-07-28: mirror headers must match the body.
        let version = req.header("mcp-protocol-version");
        if !version.is_empty() {
            let rid = id.clone().unwrap_or(Value::Null);
            if !SUPPORTED.contains(&version) {
                let data = Some(json!({ "supported": SUPPORTED }));
                return Response::json(
                    400,
                    &rpc_err(&rid, -32022, "Unsupported protocol version", data),
                );
            }
            if version == "2026-07-28" {
                let name_ok = method != "tools/call"
                    || params.get("name").and_then(Value::as_str) == Some(req.header("mcp-name"));
                if req.header("mcp-method") != method || !name_ok {
                    let m = "Header mismatch: Mcp-Method / Mcp-Name must match the request body";
                    return Response::json(400, &rpc_err(&rid, -32020, m, None));
                }
            }
        }
        // A notification: no answer.
        let Some(id) = id else {
            return Response::empty(202);
        };
        if !self.rate_ok(now, &caller.id, caller.rate_per_min) {
            return Response::json(429, &rpc_err(&id, -32000, "Rate limit exceeded", None))
                .with("Retry-After", "60");
        }
        let (status, body) = self.dispatch(&caller, &id, method, &params, now);
        Response::json(status, &body)
    }

    /// One JSON-RPC request of a signed-in connection, whatever carries it: HTTP or the
    /// standard input of a local assistant. The status is the HTTP one; a transport that has
    /// none ignores it.
    pub fn dispatch(
        &self,
        caller: &Caller,
        id: &Value,
        method: &str,
        params: &Value,
        now: i64,
    ) -> (u16, Value) {
        let server_info =
            json!({ "name": NAME, "version": VERSION, "viewContract": memcp_core::view::CONTRACT });
        // What this connection is told: the general rules, the guides of what it reaches, and
        // the indexes of those vaults (see the `guide` module).
        let instructions = || {
            guide::instructions(&self.brain, caller)
                .unwrap_or_else(|_| access::INSTRUCTIONS.to_string())
        };
        let body = match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let chosen = if SUPPORTED.contains(&asked) {
                    asked
                } else {
                    SUPPORTED[1]
                };
                rpc_ok(
                    id,
                    json!({
                        "protocolVersion": chosen,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": server_info,
                        "instructions": instructions(),
                    }),
                )
            }
            "ping" => rpc_ok(id, json!({})),
            "server/discover" => rpc_ok(
                id,
                json!({
                    "supportedVersions": SUPPORTED,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": server_info,
                    "instructions": instructions(),
                    // The content depends on the token: never shareable.
                    "ttlMs": 0,
                    "cacheScope": "private",
                }),
            ),
            "tools/list" => {
                let defs = self.tool_defs(caller);
                let list: Vec<Value> = defs
                    .iter()
                    .map(|d| json!({ "name": d.name, "description": d.description, "inputSchema": d.schema }))
                    .collect();
                rpc_ok(id, json!({ "tools": list }))
            }
            "tools/call" if self.write_needs_read(caller, params, now) => {
                if let Ok(ctl) = self.brain.open_control() {
                    access::auth_event(
                        &ctl,
                        Some(&caller.agent),
                        None,
                        "write_refused_read_first",
                        params["name"].as_str().unwrap_or_default(),
                    );
                }
                rpc_ok(
                    id,
                    json!({ "content": [{ "type": "text", "text": READ_FIRST }], "isError": true }),
                )
            }
            "tools/call" => self.tools_call(caller, id, params, now),
            _ => return (404, rpc_err(id, -32601, "Method not found", None)),
        };
        (200, body)
    }

    fn write_needs_read(&self, caller: &Caller, params: &Value, now: i64) -> bool {
        let agent = &caller.agent;
        let flagged = if self.strict_write_agents == ["*"] {
            !matches!(caller.kind, access::Kind::Peer | access::Kind::Console)
                && !agent.starts_with(memcp_core::sync::PEER_PREFIX)
                && !agent.starts_with("console-")
                && !agent.starts_with("app-")
                && !agent.ends_with("-app")
                && agent != "janitor"
        } else {
            self.strict_write_agents.contains(agent)
        };
        if !flagged
            || params
                .get("arguments")
                .is_some_and(|args| !args.is_object())
        {
            return false;
        }
        let name = params["name"].as_str().unwrap_or_default();
        let read = matches!(
            name,
            "recall"
                | "read_fact"
                | "history"
                | "space_view"
                | "links"
                | "library_search"
                | "disputes"
                | "living_get"
                | "stats"
                | "triage_waiting"
        );
        let write = matches!(
            name,
            "remember" | "supersede" | "retire" | "tags_set" | "notes_move" | "living_set"
        );
        let inbox_note = name == "remember"
            && params["arguments"]["tags"].as_array().is_some_and(|tags| {
                tags.iter().filter_map(Value::as_str).any(|tag| {
                    memcp_core::facts::normalize_tag(tag).is_ok_and(|tag| tag == "inbox")
                })
            });
        if !read && (!write || inbox_note) {
            return false;
        }
        let mut reads = self.last_reads.lock().unwrap_or_else(|e| e.into_inner());
        reads.retain(|_, at| now.saturating_sub(*at) <= READ_RETENTION);
        if read {
            if reads.len() >= MAX_RECENT_READS
                && !reads.contains_key(agent)
                && let Some(oldest) = reads
                    .iter()
                    .min_by_key(|(_, at)| *at)
                    .map(|(name, _)| name.clone())
            {
                reads.remove(&oldest);
            }
            reads
                .entry(agent.clone())
                .and_modify(|at| *at = (*at).max(now))
                .or_insert(now);
            return false;
        }
        !reads
            .get(agent)
            .is_some_and(|at| now.saturating_sub(*at) <= READ_WINDOW)
    }

    fn tools_call(&self, caller: &Caller, id: &Value, params: &Value, now: i64) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return rpc_err(id, -32602, "Invalid params: name expected", None);
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !args.is_object() {
            let m = "Invalid params: arguments must be an object";
            return rpc_err(id, -32602, m, None);
        }
        let result = match self.ledger_for(caller) {
            Some(l) if name.starts_with("finances_") => l.call(caller, name, &args, now),
            _ if name == "space_view" => self.space_view(caller, &args, now),
            _ => tools::call(&self.brain, caller, name, &args),
        };
        match result {
            // A tool that returns its own MCP content blocks (a photo, a document).
            Ok(v) if v.get("mcp_content").is_some_and(Value::is_array) => {
                let mut blocks = v["mcp_content"].as_array().cloned().unwrap_or_default();
                let mut meta = v.clone();
                if let Some(m) = meta.as_object_mut() {
                    m.remove("mcp_content");
                }
                blocks.insert(0, json!({ "type": "text", "text": meta.to_string() }));
                rpc_ok(id, json!({ "content": blocks, "isError": false }))
            }
            Ok(v) => rpc_ok(
                id,
                json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }], "isError": false }),
            ),
            Err(ToolError::Unknown) => rpc_err(id, -32602, &format!("Unknown tool: {name}"), None),
            Err(e) => rpc_ok(
                id,
                json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true }),
            ),
        }
    }
}

/// The client address kept with a connection's last use: `funnel <address>` for a request that came through Tailscale
/// Funnel (the internet: Claude on the web), whose address seen here is the proxy's. The peer directory reads it to say
/// whether an assistant is in the cloud or on the person's machines.
pub(crate) fn client_seen(req: &Request) -> Option<String> {
    if through_funnel(req) {
        Some(format!("funnel {}", req.ip.as_deref().unwrap_or("?")))
    } else {
        req.ip.clone()
    }
}

/// A request made on this machine, straight to the server: from the loopback address, and carrying none of the marks a
/// proxy leaves (`X-Forwarded-For`, `Forwarded`, the Tailscale headers, the ingress). A proxy on this machine that adds
/// none of them makes its requests look local: such a proxy must add `X-Forwarded-For`.
pub(crate) fn from_this_machine(req: &Request) -> bool {
    !req.via_ingress
        && req.tailnet_login.is_none()
        && !through_funnel(req)
        && !req.headers.contains_key("x-forwarded-for")
        && !req.headers.contains_key("forwarded")
        && !req.headers.contains_key("tailscale-user-login")
        && req
            .ip
            .as_deref()
            .and_then(|i| i.parse::<std::net::IpAddr>().ok())
            .is_some_and(|a| a.is_loopback())
}

/// A request that reached the server through Tailscale Funnel (the public internet).
pub fn through_funnel(req: &Request) -> bool {
    req.headers.contains_key("tailscale-funnel-request")
}

/// Whose secret is this? Each asks only this installation, read-only: nothing is recorded as a
/// failure. A gateway that holds several installations (one per person) asks them in turn to
/// find the one a token belongs to, then hands the request to it.
impl App {
    /// A connection token (MCP).
    pub fn holds_token(&self, token: &str) -> bool {
        self.brain
            .open_control()
            .ok()
            .and_then(|ctl| {
                access::authenticate(&ctl, token, util::now())
                    .ok()
                    .flatten()
            })
            .is_some()
    }

    /// A console token (the admin page on the main listener).
    pub fn holds_console_token(&self, token: &str) -> bool {
        self.brain
            .open_control()
            .ok()
            .and_then(|ctl| {
                access::authenticate_console(&ctl, token, util::now())
                    .ok()
                    .flatten()
            })
            .is_some()
    }

    /// The code of a device's pairing request.
    pub fn holds_pairing_code(&self, body: &[u8]) -> bool {
        crate::pair::holds(self, body)
    }

    /// This node's id (`ABCD-EFGH-IJKL`), read from the control database the first time only.
    pub fn own_node_id(&self) -> Option<&str> {
        if self.node_id.get().is_none()
            && let Ok(id) = memcp_core::directory::node_id(&self.brain)
        {
            let _ = self.node_id.set(id);
        }
        self.node_id.get().map(String::as_str)
    }

    /// The invitation ticket of a `POST /join` body, still valid here (each person's installation keeps its own).
    pub fn holds_join_ticket(&self, body: &[u8]) -> bool {
        crate::join::holds_ticket(self, body)
    }

    /// A join request this installation answered (`GET /join/<id>`).
    pub fn holds_join_request(&self, id: &str) -> bool {
        self.joins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }

    /// The invitation code of a consent page's form.
    pub fn holds_invite(&self, body: &[u8]) -> bool {
        crate::oauth::holds_invite(self, body)
    }

    /// The nonce of a consent page that was opened with an invitation code.
    pub fn holds_consent(&self, body: &[u8]) -> bool {
        crate::oauth::holds_consent(self, body)
    }

    /// The code or refresh token of an OAuth token request.
    pub fn holds_oauth_grant(&self, body: &[u8]) -> bool {
        crate::oauth::holds_grant(self, body)
    }
}

impl crate::net::Handler for App {
    fn handle(&self, req: &Request) -> Response {
        App::handle(self, req)
    }

    fn oversize(&self, ip: &str) {
        self.fail(util::now(), ip);
    }

    fn body_limit(
        &self,
        method: &str,
        path: &str,
        headers: &std::collections::HashMap<String, String>,
        trusted: bool,
    ) -> u64 {
        large_body_allowed(
            method,
            path,
            headers,
            trusted,
            |t| self.holds_token(t),
            |t| self.holds_console_token(t),
        )
    }
}

/// Whether a request may bring a recording: an MCP call with a key of this installation, or a file uploaded on the page
/// by someone the page knows. Anything else gets a small body.
pub fn large_body_allowed(
    method: &str,
    path: &str,
    headers: &std::collections::HashMap<String, String>,
    trusted: bool,
    key: impl Fn(&str) -> bool,
    console: impl Fn(&str) -> bool,
) -> u64 {
    use crate::net::{LARGE_MAX, SMALL_MAX};
    if method != "POST" {
        return SMALL_MAX;
    }
    let bearer = bearer_of(headers.get("authorization").map_or("", String::as_str));
    let path = path.trim_end_matches('/');
    let large = if path.starts_with("/mcp/") {
        bearer.as_deref().is_some_and(key)
    } else if path.ends_with("/admin/api/files") || path.ends_with("/admin/api/upload") {
        trusted || bearer.as_deref().is_some_and(console)
    } else {
        false
    };
    if large { LARGE_MAX } else { SMALL_MAX }
}

/// The last part of `/mcp/<x>`: a vault's name or a connection's, both of the same shape.
fn address_segment(raw: &str) -> Option<&str> {
    let a = raw.trim_end_matches('/');
    (Space::new(a).is_ok() || access::valid_agent(a)).then_some(a)
}

#[cfg(test)]
mod listened {
    use super::listened_url;

    #[test]
    fn the_default_address_follows_the_listener() {
        assert_eq!(
            listened_url("127.0.0.1:8799").as_deref(),
            Some("http://127.0.0.1:8799")
        );
        assert_eq!(
            listened_url("0.0.0.0:8800").as_deref(),
            Some("http://127.0.0.1:8800")
        );
        assert_eq!(
            listened_url("[::]:8801").as_deref(),
            Some("http://127.0.0.1:8801")
        );
        assert_eq!(
            listened_url("[::1]:8802").as_deref(),
            Some("http://[::1]:8802")
        );
        assert_eq!(
            listened_url("localhost:8803").as_deref(),
            Some("http://localhost:8803")
        );
        assert_eq!(listened_url("127.0.0.1:0"), None);
        assert_eq!(listened_url("nonsense"), None);
    }
}
