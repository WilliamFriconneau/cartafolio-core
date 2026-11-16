//! The manager: one memory per person, created and administered from one place.
//!
//! A person is a directory of their own (`people/<name>`), with its own control database and
//! its own vaults, served by an ordinary [`App`] that knows exactly one person. Nothing is
//! shared between people: no group, no share, no common table. The manager holds the
//! **registry** (who the people are, which Home Assistant login each answers to, who
//! administers, who is waiting to be let in), and routes what comes in to the one person it
//! concerns. It stores no memory.
//!
//! Who a request is for is decided by a secret the request already carries, or by the identity
//! the listener vouches for:
//!
//! - a connection key, an OAuth code or a console token: asked of each person in
//!   turn, read-only, and handed to the one that holds it (the others are never bothered);
//! - Home Assistant ingress (the login headers of the trusted proxy) and Tailscale serve (the
//!   tailnet login): looked up in the registry.
//!
//! An unknown Home Assistant login waits in the registry as a request. The first login of a fresh
//! installation becomes its administrator.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use memcp_core::text::{self, Language};
use memcp_core::users::{self, User};
use memcp_core::{Brain, Error};
use memcp_server::app::App;
use memcp_server::http::{Request, Response};
use memcp_server::net::Handler;
use memcp_server::oauth;
use serde_json::{Value, json};

/// What the manager is told at start.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The public address shown in connection instructions.
    pub public_url: Option<String>,
    pub allowed_origins: Vec<String>,
    /// Where the OAuth consent page is reached.
    pub login_url: Option<String>,
    pub oauth_redirects: Vec<String>,
    /// Tailnet login (lowercase) → person.
    pub tailnet_users: HashMap<String, String>,
    /// Home Assistant logins that administer the installation (ingress).
    pub admins: Vec<String>,
    /// An unknown Home Assistant account is let in at once with a memory of its own (the default of
    /// the app), instead of waiting for an administrator.
    pub auto_create_people: bool,
    /// The language of a new directory.
    pub language: Option<Language>,
}

pub struct Manager {
    /// The registry: an installation with no vault, whose people are the people of this one.
    registry: Arc<App>,
    people_dir: PathBuf,
    opts: Options,
    apps: RwLock<BTreeMap<String, Arc<App>>>,
    node_name: RwLock<String>,
}

pub type Failure = (u8, String);

fn fail(e: impl std::fmt::Display) -> Failure {
    (1, e.to_string())
}

/// The registry of a fresh installation starts with a placeholder administrator, who becomes the
/// first person to sign in (renamed and linked to their login). Until then it is nobody, and has
/// no directory.
fn is_placeholder(u: &User) -> bool {
    u.id == users::OWNER && u.name == users::OWNER && u.ha_user.is_none()
}

fn err(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

/// What any person does about their own place in the registry: name an heir, make a recovery key,
/// attach a waiting account to someone they are heir of.
fn is_personal_registry_path(path: &str) -> bool {
    matches!(
        path,
        "/admin/api/heir/set" | "/admin/api/recovery/make" | "/admin/api/requests/attach"
    )
}

/// Paths of the page's API that are about the installation (its people), not about one person.
fn is_installation_path(path: &str) -> bool {
    matches!(
        path,
        "/admin/api/users"
            | "/admin/api/users/update"
            | "/admin/api/users/unlink"
            | "/admin/api/users/console-token"
            | "/admin/api/requests/approve"
            | "/admin/api/requests/refuse"
    )
}

impl Manager {
    /// Opens `data` (creating the registry when it is not there) and every person's directory.
    pub fn open(data: &Path, opts: Options) -> Result<Manager, Failure> {
        let registry_dir = data.join("registry");
        let people_dir = data.join("people");
        std::fs::create_dir_all(data).map_err(fail)?;
        let language = opts.language.unwrap_or(Language::English);
        let brain = if registry_dir.join("control.sqlite").exists() {
            Brain::open(&registry_dir).map_err(fail)?
        } else {
            Brain::init(&registry_dir, &[], language).map_err(fail)?
        };
        text::set_language(brain.language().map_err(fail)?);
        let mut app = App::new(brain);
        Self::configure(&mut app, &opts);
        app.ingress_admins.clone_from(&opts.admins);
        app.auto_create_people = opts.auto_create_people;
        let m = Manager {
            registry: Arc::new(app),
            people_dir,
            opts,
            apps: RwLock::new(BTreeMap::new()),
            node_name: RwLock::new("MemCP".into()),
        };
        m.ensure_people()?;
        Ok(m)
    }

    /// What every installation here shares: the address, the origins, the OAuth redirects.
    fn configure(app: &mut App, opts: &Options) {
        if let Some(u) = &opts.public_url {
            app.public_url = u.trim_end_matches('/').to_string();
        }
        app.allowed_origins.clone_from(&opts.allowed_origins);
        app.login_url = opts
            .login_url
            .as_ref()
            .map(|u| u.trim_end_matches('/').to_string());
        app.oauth_redirects.extend(opts.oauth_redirects.clone());
        // The manager decides who may reach which installation; the one it hands a request to
        // answers it.
        app.console_admin = true;
    }

    pub fn registry(&self) -> &Arc<App> {
        &self.registry
    }

    pub fn people_dir(&self) -> &Path {
        &self.people_dir
    }

    /// The installation of one person.
    pub fn person_app(&self, name: &str) -> Option<Arc<App>> {
        self.apps.read().ok()?.get(name).cloned()
    }

    /// How this node calls itself when it advertises and answers `/.well-known/memcp`.
    pub fn set_node_name(&self, name: &str) {
        self.node_name
            .write()
            .map(|mut n| *n = name.to_string())
            .ok();
    }

    /// The installation that answers a device asking to join: the first administrator's (their page shows the request
    /// and the connection made is theirs). Another person adds their own devices with a code from their page.
    fn joiner(&self) -> Arc<App> {
        let admin = self
            .registry
            .brain
            .open_control()
            .ok()
            .and_then(|ctl| users::list_users(&ctl).ok())
            .and_then(|all| {
                all.into_iter()
                    .filter(|u| u.admin && !u.disabled && !is_placeholder(u))
                    .find_map(|u| self.person_app(&u.name))
            });
        admin.unwrap_or_else(|| Arc::clone(&self.registry))
    }

    /// Every installation, the registry last.
    fn all(&self) -> Vec<Arc<App>> {
        let mut v: Vec<Arc<App>> = self
            .apps
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        v.push(Arc::clone(&self.registry));
        v
    }

    /// Gives every person of the registry a directory and an installation, and keeps what the
    /// registry decides (disabled, the Home Assistant login) true in theirs.
    pub fn ensure_people(&self) -> Result<(), Failure> {
        let ctl = self.registry.brain.open_control().map_err(fail)?;
        for u in users::list_users(&ctl).map_err(fail)? {
            if is_placeholder(&u) {
                continue;
            }
            let app = match self.person_app(&u.name) {
                Some(a) => a,
                None => {
                    let a = Arc::new(self.build_person(&u)?);
                    if let Ok(mut m) = self.apps.write() {
                        m.insert(u.name.clone(), Arc::clone(&a));
                    }
                    a
                }
            };
            Self::sync_owner(&app, &u).map_err(fail)?;
        }
        Ok(())
    }

    fn build_person(&self, u: &User) -> Result<App, Failure> {
        let dir = self.people_dir.join(&u.name);
        let language = self.opts.language.unwrap_or(Language::English);
        let fresh = !dir.join("control.sqlite").exists();
        let brain = if fresh {
            std::fs::create_dir_all(&self.people_dir).map_err(fail)?;
            Brain::init(&dir, &[], language).map_err(fail)?
        } else {
            Brain::open(&dir).map_err(fail)?
        };
        if fresh {
            // The bootstrap user of a new directory becomes the person. No vault is made for
            // them: they make the ones they want, from the catalogue, on their page.
            let ctl = brain.open_control().map_err(fail)?;
            users::claim_owner(&ctl, &u.name, &u.display, u.ha_user.as_deref()).map_err(fail)?;
        }
        let mut app = App::new(brain);
        Self::configure(&mut app, &self.opts);
        for (login, person) in &self.opts.tailnet_users {
            if person == &u.name {
                app.tailnet_users.insert(login.clone(), u.name.clone());
            }
        }
        Ok(app)
    }

    /// Keeps the person's own record in line with the registry.
    fn sync_owner(app: &App, reg: &User) -> Result<(), Error> {
        let ctl = app.brain.open_control()?;
        let Some(owner) = users::user(&ctl, users::OWNER)? else {
            return Ok(());
        };
        if owner.ha_user != reg.ha_user {
            users::set_ha_user(&ctl, &owner.id, reg.ha_user.as_deref())?;
        }
        if owner.disabled != reg.disabled {
            users::set_disabled(&ctl, "registry", &owner.id, reg.disabled)?;
        }
        Ok(())
    }

    /// Adds a person to the registry (and gives them a directory).
    pub fn add_person(
        &self,
        name: &str,
        display: &str,
        ha_user: Option<&str>,
        admin: bool,
    ) -> Result<User, Failure> {
        let ctl = self.registry.brain.open_control().map_err(fail)?;
        let u = users::create_user(&ctl, "manager", name, display, admin).map_err(fail)?;
        if let Some(ha) = ha_user {
            users::set_ha_user(&ctl, &u.id, Some(ha)).map_err(fail)?;
        }
        self.ensure_people()?;
        users::user(&ctl, &u.id)
            .map_err(fail)?
            .ok_or_else(|| (1, "the person vanished".to_string()))
    }

    /// Prints the people of the registry, with what each holds.
    pub fn print_people(&self) -> Result<(), Failure> {
        let ctl = self.registry.brain.open_control().map_err(fail)?;
        for u in users::list_users(&ctl).map_err(fail)? {
            if is_placeholder(&u) {
                continue;
            }
            let vaults = self
                .person_app(&u.name)
                .and_then(|a| a.brain.spaces().ok())
                .map(|s| s.len())
                .unwrap_or(0);
            println!(
                "{:<20} {:<7} {:<24} {} vault(s){}",
                u.name,
                if u.admin { "admin" } else { "member" },
                u.ha_user.as_deref().unwrap_or("(no Home Assistant login)"),
                vaults,
                if u.disabled { "  (disabled)" } else { "" }
            );
        }
        Ok(())
    }

    /// Prints a token that opens the registry's page (people and requests) for a person who
    /// administers the installation.
    pub fn console_token(&self, person: &str) -> Result<(), Failure> {
        let ctl = self.registry.brain.open_control().map_err(fail)?;
        let u = users::user_by_name(&ctl, person)
            .map_err(fail)?
            .ok_or_else(|| (1, format!("unknown person: {person}")))?;
        let token = memcp_core::access::console_token(&ctl, &u.id).map_err(fail)?;
        println!("{token}");
        Ok(())
    }

    /// The registry's person behind a Home Assistant login, if it is a person here.
    fn person_by_ha(&self, ha_id: &str) -> Option<User> {
        let ctl = self.registry.brain.open_control().ok()?;
        users::list_users(&ctl)
            .ok()?
            .into_iter()
            .find(|u| u.ha_user.as_deref() == Some(ha_id) && !u.disabled)
    }

    fn person_by_name(&self, name: &str) -> Option<User> {
        let ctl = self.registry.brain.open_control().ok()?;
        users::user_by_name(&ctl, name)
            .ok()
            .flatten()
            .filter(|u| !u.disabled)
    }

    /// The installation that holds a secret, found read-only, the registry included.
    fn holder(&self, holds: impl Fn(&App) -> bool) -> Option<Arc<App>> {
        self.all().into_iter().find(|a| holds(a))
    }

    // --- routing -----------------------------------------------------------------------------

    fn identity(&self, req: &Request, login: &str) -> Response {
        let login = login.to_lowercase();
        let Some(name) = self.opts.tailnet_users.get(&login) else {
            return self.registry.handle(req);
        };
        let Some(u) = self.person_by_name(name) else {
            return self.registry.handle(req);
        };
        self.for_person(req, &u)
    }

    fn ingress(&self, req: &Request) -> Response {
        let id = req.header("x-remote-user-id");
        if id.is_empty() {
            return self.registry.handle(req);
        }
        let mut known = self.person_by_ha(id);
        if known.is_none() {
            // The registry meets the login: it is linked (a fresh installation), or it waits for
            // an administrator. A harmless read is enough to make it meet.
            let mut probe = req.clone();
            probe.method = "GET".into();
            probe.path = "/admin/api/state".into();
            let _ = self.registry.handle(&probe);
            known = self.person_by_ha(id);
            if known.is_some() {
                let _ = self.ensure_people();
            }
        }
        match known {
            Some(u) => self.for_person(req, &u),
            None => {
                let r = self.registry.handle(req);
                // A recovery key just gave this account its person back: keep their directory in line.
                if r.status == 200
                    && req
                        .path
                        .trim_end_matches('/')
                        .ends_with("/admin/api/recover")
                {
                    let _ = self.ensure_people();
                }
                r
            }
        }
    }

    /// A request from a known person, on a listener that vouches for who they are.
    fn for_person(&self, req: &Request, u: &User) -> Response {
        let Some(app) = self.person_app(&u.name) else {
            return err(500, "this person has no installation yet");
        };
        let path = req.path.trim_end_matches('/');
        if let Ok(ctl) = self.registry.brain.open_control() {
            let _ = users::touch_seen(&ctl, &u.id, memcp_core::util::now());
        }
        if is_personal_registry_path(path) {
            let r = self.registry.handle(req);
            // Attaching an account changes who a person answers to: their directory follows.
            let _ = self.ensure_people();
            return r;
        }
        if is_installation_path(path) {
            if !u.admin {
                return err(403, "only an administrator of the installation does this");
            }
            let r = self.registry.handle(req);
            let _ = self.ensure_people();
            return r;
        }
        if path == "/oauth/authorize" {
            let cid = req.query.get("client_id").cloned().unwrap_or_else(|| {
                oauth::form(&req.body)
                    .get("client_id")
                    .cloned()
                    .unwrap_or_default()
            });
            if !cid.is_empty() {
                oauth::copy_client(&self.registry, &app, &cid);
            }
            return app.handle(req);
        }
        let mut r = app.handle(req);
        if path == "/admin/api/state" && r.status == 200 {
            r = self.with_safety(r, u);
        }
        if path == "/admin/api/state" && r.status == 200 && u.admin {
            let reg = self.registry.handle(req);
            if reg.status == 200 {
                let (mine, theirs) = (r.body_json(), reg.body_json());
                let mut st = mine;
                // What each person holds, counted, never read: vaults and live connections.
                let people: Vec<Value> = theirs["users"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter(|x| {
                                !(x["id"] == users::OWNER
                                    && x["name"] == users::OWNER
                                    && x["home_assistant"] != true)
                            })
                            .map(|x| {
                                let mut x = x.clone();
                                let name = x["name"].as_str().unwrap_or_default().to_string();
                                if let Some(app) = self.person_app(&name) {
                                    x["vaults"] = json!(app.brain.spaces().map_or(0, |s| s.len()));
                                    // Their names only, to draw the tiles: never what a vault holds.
                                    x["vault_names"] = json!(
                                        app.brain
                                            .spaces()
                                            .map(|s| s
                                                .iter()
                                                .map(ToString::to_string)
                                                .collect::<Vec<_>>())
                                            .unwrap_or_default()
                                    );
                                    x["connections"] = json!(
                                        app.brain
                                            .open_control()
                                            .ok()
                                            .and_then(|c| memcp_core::access::list(&c).ok())
                                            .map_or(0, |l| l
                                                .iter()
                                                .filter(|c| c.revoked_at.is_none())
                                                .count())
                                    );
                                }
                                x
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                st["installation"] = json!({
                    "admin": true,
                    "me": u.id,
                    "users": people,
                    "requests": theirs["requests"],
                });
                r = Response::json(200, &st);
            }
        }
        r
    }

    /// Adds to a person's state what keeps their memory from being lost with their account: their
    /// heir, whether they have a recovery key, and, if they are heir to someone, who that is and
    /// whether the accounts waiting could be attached.
    fn with_safety(&self, r: Response, u: &User) -> Response {
        let Ok(ctl) = self.registry.brain.open_control() else {
            return r;
        };
        let now = memcp_core::util::now();
        let person = |p: &User| json!({ "id": p.id, "name": p.name, "display": p.display });
        let all = users::list_users(&ctl).unwrap_or_default();
        let heir = users::heir_of(&ctl, &u.id)
            .ok()
            .flatten()
            .and_then(|h| users::user(&ctl, &h).ok().flatten());
        let heir_of: Vec<Value> = users::heir_to(&ctl, &u.id)
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let idle = now - users::last_seen(&ctl, p.id.as_str()).unwrap_or(now);
                let mut j = person(p);
                j["idle_days"] = json!(idle / 86_400);
                j["eligible"] = json!(idle >= users::HEIR_WAIT_SECS);
                j
            })
            .collect();
        let waiting: Vec<Value> = if heir_of.is_empty() {
            vec![]
        } else {
            users::list_requests(&ctl)
                .unwrap_or_default()
                .iter()
                .filter(|q| q.status == "pending")
                .map(|q| json!({ "ha_user": q.ha_user, "name": q.name, "display": q.display }))
                .collect()
        };
        let mut st = r.body_json();
        st["safety"] = json!({
            "heir": heir.as_ref().map(person),
            "has_key": users::has_recovery_key(&ctl, &u.id).unwrap_or(false),
            "candidates": all
                .iter()
                .filter(|p| p.id != u.id && !p.disabled && !is_placeholder(p))
                .map(person)
                .collect::<Vec<_>>(),
            "heir_of": heir_of,
            "waiting": waiting,
            "wait_days": users::HEIR_WAIT_SECS / 86_400,
        });
        Response::json(200, &st)
    }

    /// The installation to answer a request that carries a secret of its own.
    fn by_secret(&self, req: &Request) -> Arc<App> {
        let path = req.path.trim_end_matches('/');
        let bearer = memcp_server::http::bearer_of(req.header("authorization"));
        let found = match path {
            "/oauth/token" => self.holder(|a| a.holds_oauth_grant(&req.body)),
            "/admin" | "/admin/api/state" => bearer
                .as_deref()
                .and_then(|t| self.holder(|a| a.holds_console_token(t))),
            p if p.starts_with("/admin/api/") => bearer
                .as_deref()
                .and_then(|t| self.holder(|a| a.holds_console_token(t))),
            p if p.starts_with("/mcp/") => bearer
                .as_deref()
                .and_then(|t| self.holder(|a| a.holds_token(t))),
            _ => None,
        };
        found.unwrap_or_else(|| Arc::clone(&self.registry))
    }
}

impl Handler for Manager {
    fn body_limit(
        &self,
        method: &str,
        path: &str,
        headers: &std::collections::HashMap<String, String>,
        trusted: bool,
    ) -> u64 {
        memcp_server::app::large_body_allowed(
            method,
            path,
            headers,
            trusted,
            |t| self.holder(|a| a.holds_token(t)).is_some(),
            |t| self.holder(|a| a.holds_console_token(t)).is_some(),
        )
    }

    fn handle(&self, req: &Request) -> Response {
        if let Some(login) = &req.tailnet_login {
            return self.identity(req, login);
        }
        if req.via_ingress {
            return self.ingress(req);
        }
        let path = req.path.trim_end_matches('/');
        // `/mcp/<person>/<connection>`: the address says whose connection it is, as the page shows it. The
        // person's own installation answers it as `/mcp/<connection>`.
        for base in ["/mcp/", "/.well-known/oauth-protected-resource/mcp/"] {
            let Some(rest) = path.strip_prefix(base) else { continue };
            if let Some((who, agent)) = rest.split_once('/')
                && !agent.contains('/')
                && let Some(app) = self.person_app(who)
            {
                let mut inner = req.clone();
                inner.path = format!("{base}{agent}");
                return app.handle(&inner);
            }
        }
        let reply = |app: &App| app.handle(req);
        match (req.method.as_str(), path) {
            ("GET", "/health") => Response::json(200, &json!({ "ok": true })),
            ("POST", "/oauth/register") => reply(&self.registry),
            ("POST", "/pair") => {
                // Wrong codes are answered, and counted, by the registry: an address held off there is
                // held off before any person's installation is asked.
                let now = memcp_core::util::now();
                let who = req.ip.clone().unwrap_or_else(|| "?".into());
                if self.registry.blocked(now, &who) {
                    return reply(&self.registry);
                }
                match self.holder(|a| a.holds_pairing_code(&req.body)) {
                    Some(app) if !Arc::ptr_eq(&app, &self.registry) => reply(&app),
                    _ => reply(&self.registry),
                }
            }
            ("GET", "/.well-known/memcp") => Response::json(
                200,
                &memcp_server::discovery::well_known(
                    &self.node_name.read().map(|n| n.clone()).unwrap_or_default(),
                ),
            ),
            ("POST", "/join") => reply(&self.joiner()),
            ("GET", p) if p.starts_with("/join/") => reply(&self.joiner()),
            _ => reply(&self.by_secret(req)),
        }
    }
}
