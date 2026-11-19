//! The `memcp` binary: the owner's command line and the node (MCP, the page, OAuth, sync with its peers).

use memcp_server::{app, cli, config, net, setup};

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use cli::Failure;
use memcp_core::Brain;
use memcp_core::text;

use memcp_server::commands::{Args, USAGE, dispatch, list, parse};
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            // A command that ran on a node already printed its own message.
            if !message.is_empty() {
                eprintln!("{message}");
            }
            ExitCode::from(code)
        }
    }
}

fn run(args: &[String]) -> Result<(), Failure> {
    let mut a = parse(args)?;
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
    // Against a running node (`--node`, or MEMCP_NODE): the same commands, over HTTP with a key (docs/CLI.md).
    if let Some(target) = memcp_server::remote::target(&a)? {
        return memcp_server::remote::run(&target, args, &a);
    }
    // The settings file fills in what the command line does not say.
    // `setup` may name a file that does not exist yet: it makes it.
    let named = a.flags.get("--config").map(String::as_str);
    let cfg_path = match named {
        Some(f) if a.pos[0] == "setup" && !Path::new(f).is_file() => None,
        _ => config::locate(named).map_err(|e| (2, e))?,
    };
    let settings = match &cfg_path {
        Some(p) => config::Settings::load(p).map_err(|e| (2, e))?,
        None => config::Settings::default(),
    };
    if a.pos[0] == "config" {
        return setup::config_command(a.pos.get(1).map_or("check", String::as_str), cfg_path);
    }
    if a.pos[0] == "setup" {
        let f = |n: &str| a.flags.get(n).cloned();
        let sw = |n: &str| a.switches.iter().any(|s| s == n);
        return setup::setup(
            setup::SetupArgs {
                data: f("--data").map(PathBuf::from),
                config: f("--config").map(PathBuf::from),
                node_name: f("--node-name"),
                language: f("--language"),
                listen: f("--listen"),
                public_url: f("--public-url"),
                person: f("--person"),
                spaces: if sw("--defaults") {
                    None
                } else {
                    f("--spaces")
                },
                assistant: f("--assistant"),
                yes: sw("--yes") || sw("-y"),
                system: sw("--system"),
                force: sw("--force"),
            },
            cfg_path,
        );
    }
    config::fill_flags(&settings, &mut a.flags, &mut a.switches);
    let data: PathBuf = a
        .flags
        .get("--data")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("MEMCP_DATA").map(PathBuf::from))
        .ok_or_else(|| (2, format!("missing --data DIR\n\n{USAGE}")))?;
    if a.pos[0] == "serve" {
        return serve(
            &data,
            &a,
            &settings,
            a.flags
                .get("--listen")
                .map_or("127.0.0.1:8791", String::as_str),
        );
    }
    dispatch(&data, &a, &settings, cfg_path.as_deref())
}

/// The intake's background tasks, from the settings. A setting that cannot work is said, and the node starts without it.
fn start_intake(
    brain: &Brain,
    settings: &config::Settings,
    data: &Path,
) -> Option<memcp_server::intake::Running> {
    let intake = memcp_server::intake::Settings::from_file(settings, &data.join("intake"))
        .and_then(|s| memcp_server::intake::start(brain, s).map_err(|e| e.to_string()));
    match intake {
        Ok(running) => running,
        Err(e) => {
            eprintln!("intake: not started: {e}");
            None
        }
    }
}

fn serve(data: &Path, a: &Args, settings: &config::Settings, listen: &str) -> Result<(), Failure> {
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
    // What a crash cut off is finished or undone before anything else is served (see core recover.rs).
    match brain.recover() {
        Ok(r) if r.is_clean() && r.swept => {}
        Ok(r) => eprintln!("{}", r.summary()),
        Err(e) => eprintln!("recovery: not finished yet: {e}"),
    }
    let mut app = app::App::new(brain);
    app.allowed_origins.extend(list(flag("--allow-origin")));
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
    // The name the node goes by in the peer directory: the one given, else the one it made for itself once.
    if let Some(n) = flag("--node-name") {
        let _ = memcp_core::directory::set_own_name(&app.brain, n);
    }
    // How this node can be reached, from its settings: told to the other nodes in its directory entry.
    let tailnet = flag("--identity-listen")
        .map(String::from)
        .filter(|_| !app.tailnet_users.is_empty() || app.login_url.is_some())
        .map(|l| app.login_url.clone().unwrap_or(l));
    let _ = memcp_core::directory::set_exposures(
        &app.brain,
        &memcp_core::directory::exposures_from(listen, &app.public_url, tailnet.as_deref()),
    );
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
    // The peers this node keeps in step with, started again with it.
    memcp_server::replica::resume(&app);
    // Voice notes, watched folders and absorbed folders: tasks of the node itself, as set in memcp.toml.
    let _intake = start_intake(&app.brain, settings, data);
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
