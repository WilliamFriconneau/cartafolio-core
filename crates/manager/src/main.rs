//! `memcp-manager`: one memory per person, created and administered from one place.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use memcp_core::text::Language;
use memcp_manager::{Failure, Manager, Options};
use memcp_server::net;

const USAGE: &str = "usage: memcp-manager --data DIR <command>

  Settings: cartafolio.toml or memcp.toml. Use --config FILE to choose a path (see docs/LINUX.md).
  serve [--listen 127.0.0.1:8791] [--allow-remote] [--advertise] [--node-name NAME] [--allow-origin URL] [--threads N]
        [--public-url URL] [--admins ha_name,...] [--language en|fr] [--new-people auto|ask]
        [--ingress-listen ADDR --trust-ingress IP]
        [--identity-listen ADDR [--identity-trust IP] --tailnet-user login=person,... --login-url URL]
        [--oauth-redirect URL]
                                         serves every person, each from a directory of their own
  people                                 lists the people
  person add NAME [--display TEXT] [--ha-user ID] [--admin]
                                         adds a person and gives them a directory
  console-token PERSON                   a token for the registry's page (people and requests)
  doctor                                 reads every directory and says what deserves a look

A person's own commands are the engine's, on their directory:
  memcp --data DIR/people/NAME connections";

fn command_name() -> &'static str {
    memcp_core::invoked_as("memcp-manager", "cartafolio-manager")
}

fn usage() -> String {
    USAGE.replace("memcp-manager ", &format!("{} ", command_name()))
}

struct Args {
    pos: Vec<String>,
    flags: HashMap<String, String>,
    switches: Vec<String>,
}

const VALUE_FLAGS: [&str; 20] = [
    "--config",
    "--data",
    "--node-name",
    "--listen",
    "--allow-origin",
    "--threads",
    "--public-url",
    "--admins",
    "--language",
    "--ingress-listen",
    "--trust-ingress",
    "--identity-listen",
    "--identity-trust",
    "--tailnet-user",
    "--login-url",
    "--oauth-redirect",
    "--from",
    "--map",
    "--display",
    "--new-people",
];

fn parse(args: &[String]) -> Result<Args, Failure> {
    let mut a = Args {
        pos: vec![],
        flags: HashMap::new(),
        switches: vec![],
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if x == "--ha-user" || VALUE_FLAGS.contains(&x.as_str()) {
            let v = it
                .next()
                .ok_or_else(|| (2, format!("{x} needs a value\n\n{}", usage())))?;
            a.flags.insert(x.clone(), v.clone());
        } else if x.starts_with("--") || x == "-h" {
            a.switches.push(x.clone());
        } else {
            a.pos.push(x.clone());
        }
    }
    Ok(a)
}

fn list(v: Option<&str>) -> Vec<String> {
    v.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn language(a: &Args) -> Result<Option<Language>, Failure> {
    match a.flags.get("--language").map(String::as_str) {
        None => Ok(None),
        Some("en") => Ok(Some(Language::English)),
        Some("fr") => Ok(Some(Language::French)),
        Some(_) => Err((2, "--language: en or fr".into())),
    }
}

fn options(a: &Args) -> Result<Options, Failure> {
    let flag = |n: &str| a.flags.get(n).map(String::as_str);
    let mut tailnet_users = HashMap::new();
    for pair in list(flag("--tailnet-user")) {
        let (login, person) = pair
            .split_once('=')
            .ok_or((2, "--tailnet-user: login=person".to_string()))?;
        tailnet_users.insert(login.trim().to_lowercase(), person.trim().to_string());
    }
    Ok(Options {
        public_url: flag("--public-url")
            .map(String::from)
            .or_else(|| memcp_core::env::var("MEMCP_PUBLIC_URL").ok()),
        node_name: flag("--node-name").map(String::from),
        listen: flag("--listen").map(String::from),
        allowed_origins: flag("--allow-origin")
            .map(|o| vec![o.to_string()])
            .unwrap_or_default(),
        login_url: flag("--login-url").map(String::from),
        oauth_redirects: list(flag("--oauth-redirect")),
        tailnet_users,
        admins: list(flag("--admins")),
        auto_create_people: match flag("--new-people") {
            None | Some("auto") => true,
            Some("ask") => false,
            Some(_) => return Err((2, "--new-people: auto or ask".into())),
        },
        language: language(a)?,
    })
}

fn run(args: &[String]) -> Result<(), Failure> {
    let mut a = parse(args)?;
    // memcp.toml (the same file as `memcp`) fills in what the command line does not say.
    let mut settings = memcp_server::config::Settings::default();
    let data_hint = a
        .flags
        .get("--data")
        .map(PathBuf::from)
        .or_else(|| memcp_core::env::var_os("MEMCP_DATA").map(PathBuf::from));
    if let Some(p) = memcp_server::config::locate_with_data(
        a.flags.get("--config").map(String::as_str),
        data_hint.as_deref(),
    )
    .map_err(|e| (2, e))?
    {
        settings = memcp_server::config::Settings::load(&p).map_err(|e| (2, e))?;
        memcp_server::config::fill_flags(&settings, &mut a.flags, &mut a.switches);
    }
    if a.switches.iter().any(|s| s == "-h" || s == "--help") || a.pos.is_empty() {
        println!("{}", usage());
        return Ok(());
    }
    let data: PathBuf = a
        .flags
        .get("--data")
        .map(PathBuf::from)
        .or(data_hint)
        .ok_or_else(|| (2, format!("missing --data DIR\n\n{}", usage())))?;
    let flag = |n: &str| a.flags.get(n).map(String::as_str);
    match (a.pos[0].as_str(), a.pos.get(1).map(String::as_str)) {
        ("serve", _) => serve(&data, &a, &settings),
        ("people", _) => {
            let m = Manager::open(&data, options(&a)?)?;
            m.print_people()
        }
        ("person", Some("add")) => {
            let name = a
                .pos
                .get(2)
                .ok_or_else(|| (2, "person add NAME".to_string()))?;
            let m = Manager::open(&data, options(&a)?)?;
            let u = m.add_person(
                name,
                flag("--display").unwrap_or(""),
                flag("--ha-user"),
                a.switches.iter().any(|s| s == "--admin"),
            )?;
            println!(
                "{} added, with a directory of their own: {}",
                u.name,
                m.people_dir().join(&u.name).display()
            );
            Ok(())
        }
        ("console-token", _) => {
            let person = a
                .pos
                .get(1)
                .ok_or_else(|| (2, "console-token PERSON".to_string()))?;
            let m = Manager::open(&data, options(&a)?)?;
            m.console_token(person)
        }
        ("doctor", _) => m_doctor(&data, &a),
        (other, _) => Err((2, format!("unknown command {other:?}\n\n{}", usage()))),
    }
}

fn m_doctor(data: &Path, a: &Args) -> Result<(), Failure> {
    let flag = |n: &str| a.flags.get(n).map(String::as_str);
    let mut problems = 0;
    let mut dirs: Vec<(String, PathBuf)> = vec![("registry".into(), data.join("registry"))];
    if let Ok(rd) = std::fs::read_dir(data.join("people")) {
        let mut names: Vec<_> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        for n in names {
            dirs.push((format!("person {n}"), data.join("people").join(&n)));
        }
    }
    for (i, (label, dir)) in dirs.iter().enumerate() {
        println!("== {label}");
        let role = if i == 0 {
            memcp_server::doctor::Role::Registry
        } else {
            memcp_server::doctor::Role::Person
        };
        let r = memcp_server::doctor::inspect_as(
            dir,
            role,
            if i == 0 { flag("--public-url") } else { None },
            if i == 0 { flag("--listen") } else { None },
        );
        print!("{}", r.render());
        problems += r.problems();
    }
    println!("\n{problems} problem(s).");
    if problems > 0 {
        return Err((1, "problems found".into()));
    }
    Ok(())
}

/// The intake's background tasks for the person the settings name (the first of the people when none is named). A setting
/// that cannot work is said, and the node starts without it.
fn start_intake(
    manager: &Manager,
    settings: &memcp_server::config::Settings,
) -> Option<memcp_server::intake::Running> {
    let probe = memcp_server::intake::Settings::from_file(settings, Path::new("."));
    match probe {
        Ok(p) if p.is_idle() => return None,
        Err(e) => {
            eprintln!("intake: not started: {e}");
            return None;
        }
        Ok(_) => {}
    }
    let person = ["intake_person", "transcribe_person", "janitor_person"]
        .iter()
        .find_map(|k| settings.text(k).map(String::from));
    let Some(person) = person else {
        eprintln!("intake: not started: name the person it works for (intake_person)");
        return None;
    };
    let Some(app) = manager.person_app(&person) else {
        eprintln!("intake: not started: {person} has no memory on this node");
        return None;
    };
    let started =
        memcp_server::intake::Settings::from_file(settings, &app.brain.dir().join("intake"))
            .and_then(|mut s| {
                // Inside the person's own memory, the first administrator of it is the person.
                s.person = None;
                memcp_server::intake::start(&app.brain, s).map_err(|e| e.to_string())
            });
    match started {
        Ok(running) => running,
        Err(e) => {
            eprintln!("intake: not started: {e}");
            None
        }
    }
}

fn serve(data: &Path, a: &Args, settings: &memcp_server::config::Settings) -> Result<(), Failure> {
    let flag = |n: &str| a.flags.get(n).map(String::as_str);
    let listen = flag("--listen").unwrap_or("127.0.0.1:8791");
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
    let manager = Arc::new(Manager::open(data, options(a)?)?);
    // Voice notes, watched folders and absorbed folders work for one person of this node: tasks of the node itself.
    let _intake = start_intake(&manager, settings);
    // Found without typing an address: advertised on the local network (see docs/DISCOVERY.md).
    let _advertised = if a.switches.iter().any(|s| s == "--advertise") {
        let port = listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8791);
        memcp_server::discovery::advertise(port, &manager.node_name())
    } else {
        None
    };
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
        manager,
        &net::Listeners {
            listen: listen.to_string(),
            identity,
            ingress,
            threads,
        },
        &format!("{} {}", command_name(), memcp_server::app::VERSION),
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, msg)) => {
            eprintln!("{msg}");
            ExitCode::from(code)
        }
    }
}
