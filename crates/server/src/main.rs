//! The `memcp` binary: the owner's command line and the server (MCP, admin page, OAuth).

use memcp_server::{app, cli, net};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use cli::Failure;
use memcp_core::Brain;
use memcp_core::text::{self, Language};

const USAGE: &str = "usage: memcp --data DIR <command>

  init [--spaces health,home,personal] [--language en|fr]
                                         creates the data directory, the control database and the spaces
  catalogue                              lists the kinds of vault this installation offers
  vault add KIND [--user PERSON]         creates a person's vault of a kind of the catalogue (named after them)
  space add NAME [--user PERSON] [--kind KIND] | space list [--kinds] | space kind NAME KIND
                                         a space named by hand, for an operator; it has no kind unless given one
  console-token PERSON                   a token that opens the admin page (/admin) where it is served, never MCP
  serve [--listen 127.0.0.1:8791] [--allow-remote] [--advertise] [--node-name NAME] [--allow-origin URL] [--threads N]
        [--console-admin]                the admin page on this listener, with a console token: on by
                                         default on the loopback only, never through Tailscale Funnel
        [--public-url URL] [--ingress-listen ADDR --trust-ingress IP --admins ha_login,...]
                                         admin page through Home Assistant ingress
        [--identity-listen ADDR [--identity-trust IP] --tailnet-user login=person,... --login-url URL] [--oauth-redirect URL]
                                         OAuth for Claude on the web: consent page behind Tailscale serve
  connect AGENT [--user PERSON] (--all | --grant health:rw,work:r)
        [--days N] [--read-only] [--rotate] [--public-url URL]
                                         creates an MCP connection and shows its token once
  open VAULT [--user PERSON] [--read-only] [--days N] [--public-url URL]
                                         opens one vault to an assistant: a connection limited to it,
                                         token shown once
  mcp [--token-env NAME]                 the MCP server on standard input and output, for an assistant that
                                         runs a command: the key is read from MEMCP_TOKEN (or NAME)
  export [--user PERSON] --to DIR        writes a person's memory to a directory: each vault they own (a copy,
                                         its files, a readable Markdown version), their instructions, a
                                         manifest of fingerprints; no key, connection or right
  verify-export DIR                      checks an export (no --data needed)
  adopt DIR [--user PERSON] [--prefix P] takes the vaults of an export into this installation
  connections                            lists the connections
  doctor [--public-url URL] [--listen ADDR]
                                         reads the installation and says what is fine and what deserves a look:
                                         files, versions, integrity, people, connections, the address. Writes nothing.
  regrant AGENT (--all | --grant ...)
                                         changes the rights, same token
  revoke AGENT                           cuts a connection
  readonly AGENT on|off                  forces read-only
  person-guide AGENT on|off              off: the person's own guide is not told to this connection (a work assistant)
  limit AGENT rate|max-text N     sets a limit
  import --vault DIR --rules FILE [--dry-run] [--yes] [--only DIR] [--protect word,word]
                                         indexes a Markdown folder (first run: --dry-run)
  backup --to DIR [--keep 14]            consistent copy of every database (safe while serving), outside the data dir

The directory may come from MEMCP_DATA. The default public address comes from MEMCP_PUBLIC_URL.";

const VALUE_FLAGS: [&str; 28] = [
    "--data",
    "--node-name",
    "--spaces",
    "--language",
    "--listen",
    "--allow-origin",
    "--threads",
    "--grant",
    "--days",
    "--public-url",
    "--space",
    "--vault",
    "--rules",
    "--only",
    "--protect",
    "--to",
    "--keep",
    "--ingress-listen",
    "--trust-ingress",
    "--admins",
    "--user",
    "--display",
    "--access",
    "--identity-listen",
    "--identity-trust",
    "--tailnet-user",
    "--login-url",
    "--oauth-redirect",
];

struct Args {
    flags: HashMap<String, String>,
    switches: Vec<String>,
    pos: Vec<String>,
}

fn parse(args: &[String]) -> Result<Args, Failure> {
    let mut a = Args {
        flags: HashMap::new(),
        switches: vec![],
        pos: vec![],
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if VALUE_FLAGS.contains(&x.as_str()) {
            let v = it
                .next()
                .ok_or_else(|| (2, format!("{x} needs a value\n\n{USAGE}")))?;
            a.flags.insert(x.clone(), v.clone());
        } else if x.starts_with("--") || x == "-h" {
            a.switches.push(x.clone());
        } else {
            a.pos.push(x.clone());
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("{message}");
            ExitCode::from(code)
        }
    }
}

fn list(v: Option<&str>) -> Vec<String> {
    v.map(|p| {
        p.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn run(args: &[String]) -> Result<(), Failure> {
    let a = parse(args)?;
    if a.switches.iter().any(|s| s == "-h" || s == "--help") || a.pos.is_empty() {
        println!("{USAGE}");
        return Ok(());
    }
    // Checking an export needs no installation.
    if a.pos[0] == "verify-export" {
        let dir = a
            .pos
            .get(1)
            .ok_or_else(|| (2, format!("missing the export directory\n\n{USAGE}")))?;
        return cli::verify_export(Path::new(dir));
    }
    let data: PathBuf = a
        .flags
        .get("--data")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("MEMCP_DATA").map(PathBuf::from))
        .ok_or_else(|| (2, format!("missing --data DIR\n\n{USAGE}")))?;
    let sw = |name: &str| a.switches.iter().any(|s| s == name);
    let flag = |name: &str| a.flags.get(name).map(String::as_str);
    let arg = |i: usize, what: &str| -> Result<&str, Failure> {
        a.pos
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| (2, format!("missing {what}\n\n{USAGE}")))
    };
    let need = |name: &str| flag(name).ok_or_else(|| (2, format!("missing {name}\n\n{USAGE}")));
    let public = flag("--public-url")
        .map(String::from)
        .or_else(|| std::env::var("MEMCP_PUBLIC_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8791".into());
    let rights = || cli::rights(sw("--all"), flag("--grant"));

    match (a.pos[0].as_str(), a.pos.get(1).map(String::as_str)) {
        ("init", _) => {
            let code = flag("--language").unwrap_or("en");
            let language = Language::parse(code).ok_or_else(|| cli::bad("--language: en or fr"))?;
            cli::init(
                &data,
                &cli::parse_spaces(flag("--spaces").unwrap_or(""))?,
                language,
            )
        }
        ("space", Some("add")) => cli::space_add(
            &data,
            arg(2, "the space name")?,
            flag("--user"),
            flag("--kind"),
        ),
        ("space", Some("list")) => cli::space_list(&data, sw("--kinds")),
        ("space", Some("kind")) => {
            cli::space_kind(&data, arg(2, "the space")?, arg(3, "the kind")?)
        }
        ("vault", Some("add")) => cli::vault_add(&data, arg(2, "the kind")?, flag("--user")),
        ("catalogue", _) => cli::catalogue_list(&data),
        ("serve", _) => serve(&data, &a, flag("--listen").unwrap_or("127.0.0.1:8791")),
        ("connect", _) => cli::connect(
            &data,
            cli::ConnectOpts {
                agent: arg(1, "the agent name")?.to_string(),
                user: flag("--user").map(String::from),
                rights: rights()?,
                days: flag("--days").and_then(|d| d.parse().ok()),
                read_only: sw("--read-only"),
                rotate: sw("--rotate"),
                url: public,
            },
        ),
        ("open", _) => cli::open_vault(
            &data,
            arg(1, "the vault")?,
            flag("--user"),
            sw("--read-only"),
            flag("--days").and_then(|d| d.parse().ok()),
            &public,
        ),
        ("mcp", _) => cli::mcp_stdio(&data, flag("--token-env").unwrap_or("MEMCP_TOKEN")),
        ("export", _) => cli::export_person(&data, flag("--user"), Path::new(need("--to")?)),
        ("adopt", _) => cli::adopt(
            &data,
            Path::new(arg(1, "the export directory")?),
            flag("--user"),
            flag("--prefix"),
        ),
        ("connections", _) => cli::connections(&data),
        ("doctor", _) => memcp_server::doctor::run(&data, flag("--public-url"), flag("--listen")),
        ("regrant", _) => cli::regrant(&data, arg(1, "the agent name")?, &rights()?),
        ("revoke", _) => cli::revoke(&data, arg(1, "the agent name")?),
        ("readonly", _) => cli::readonly(&data, arg(1, "the agent name")?, arg(2, "on or off")?),
        ("person-guide", _) => {
            cli::person_guide(&data, arg(1, "the agent name")?, arg(2, "on or off")?)
        }
        ("limit", _) => cli::limit(
            &data,
            arg(1, "the agent")?,
            arg(2, "rate or max-text")?,
            arg(3, "the value")?,
        ),
        ("duplicates", _) => cli::duplicates(&data),
        ("console-token", _) => cli::console_token(&data, arg(1, "the person")?),
        ("import", _) => cli::import(
            &data,
            Path::new(need("--vault")?),
            Path::new(need("--rules")?),
            flag("--only"),
            sw("--dry-run"),
            sw("--yes"),
            list(flag("--protect")),
        ),
        ("backup", _) => cli::backup(
            &data,
            Path::new(need("--to")?),
            flag("--keep").map_or(Ok(14), |k| {
                k.parse().map_err(|_| cli::bad("--keep needs a number"))
            })?,
        ),
        (other, _) => Err((2, format!("unknown command {other:?}\n\n{USAGE}"))),
    }
}

fn serve(data: &Path, a: &Args, listen: &str) -> Result<(), Failure> {
    let flag = |name: &str| a.flags.get(name).map(String::as_str);
    // Tokens travel in clear between the client and this process: only a local TLS
    // proxy should reach it. A non-local address is refused without explicit consent.
    let loopback = listen.starts_with("127.")
        || listen.starts_with("[::1]")
        || listen.starts_with("localhost");
    if !loopback && !a.switches.iter().any(|s| s == "--allow-remote") {
        return Err((
            2,
            format!(
                "{listen} is not a local address: tokens would travel in clear. Put a TLS proxy in front, or add --allow-remote."
            ),
        ));
    }
    let brain = Brain::open(data).map_err(cli::fail)?;
    text::set_language(brain.language().map_err(cli::fail)?);
    let mut app = app::App::new(brain);
    if let Some(o) = flag("--allow-origin") {
        app.allowed_origins.push(o.to_string());
    }
    // The admin page on this listener (console token): by default only on the loopback.
    app.console_admin = a.switches.iter().any(|s| s == "--console-admin") || loopback;
    app.ingress_admins = list(flag("--admins"));
    app.login_url = flag("--login-url").map(|u| u.trim_end_matches('/').to_string());
    app.oauth_redirects.extend(list(flag("--oauth-redirect")));
    for pair in list(flag("--tailnet-user")) {
        let (login, user) = pair
            .split_once('=')
            .ok_or((2, "--tailnet-user: login=person".to_string()))?;
        app.tailnet_users
            .insert(login.trim().to_lowercase(), user.trim().to_string());
    }
    if let Some(u) = flag("--public-url")
        .map(String::from)
        .or_else(|| std::env::var("MEMCP_PUBLIC_URL").ok())
    {
        app.public_url = u.trim_end_matches('/').to_string();
    }
    app.node_name = flag("--node-name").unwrap_or("MemCP").to_string();
    // Found without typing an address: advertised on the local network (see docs/DISCOVERY.md).
    let _advertised = if a.switches.iter().any(|s| s == "--advertise") {
        let port = listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8791);
        memcp_server::discovery::advertise(port, &app.node_name)
    } else {
        None
    };
    let app = Arc::new(app);
    let identity = match flag("--identity-listen") {
        Some(addr) => {
            let trusted: Option<std::net::IpAddr> = match flag("--identity-trust") {
                Some(ip) => Some(
                    ip.parse()
                        .map_err(|_| (2, "--identity-trust: an IP address".to_string()))?,
                ),
                None => None,
            };
            Some((addr.to_string(), trusted))
        }
        None => None,
    };
    let ingress = match flag("--ingress-listen") {
        Some(addr) => {
            let trusted: std::net::IpAddr = flag("--trust-ingress")
                .unwrap_or("172.30.32.2")
                .parse()
                .map_err(|_| (2, "--trust-ingress: an IP address".to_string()))?;
            Some((addr.to_string(), trusted))
        }
        None => None,
    };
    let threads = flag("--threads")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(4);
    net::serve(
        app,
        &net::Listeners {
            listen: listen.to_string(),
            identity,
            ingress,
            threads,
        },
        &format!("{} {}", app::NAME, app::VERSION),
    )
}
