//! The `memcp` binary: the owner's command line and the server (MCP, admin page, OAuth).

use memcp_server::{app, cli, config, net, setup};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use cli::Failure;
use memcp_core::Brain;
use memcp_core::text::{self, Language};

const USAGE: &str = "usage: memcp --data DIR <command>

  init [--spaces health,home,personal | --defaults] [--language en|fr]
                                         --defaults: personal, health, home, finances, hobbies and work
                                         creates the data directory, the control database and the spaces
  setup [--system] [--yes] [--person NAME] [--node-name NAME] [--language en|fr] [--listen ADDR]
        [--public-url URL] [--spaces LIST] [--assistant NAME] [--config FILE] [--force]
                                         first run: the data directory (private), the first memory, the default
                                         spaces, memcp.toml; prints the page address and one address with the key for an
                                         assistant. Asks questions on a terminal; --yes (or no terminal) asks nothing.
  config check|show|example              reads memcp.toml, shows it with every option, or prints the documented file
  prepare janitor --out DIR              what the janitor's service unit runs first: its key and its settings
  transcribe [--user PERSON]             one pass over the recordings waiting in the inboxes (the node does it by itself
                                         while it serves, as set by `transcribe` in memcp.toml)
  watch [--once] [--user PERSON]         looks at the folders of `watch` in memcp.toml, every few seconds until stopped
  intake send FILE... [--space SPACE]    puts files into a space's inbox (À trier by default): a text file becomes a note,
                                         audio a recording, the rest a file
  intake text TEXT | - [--space SPACE]   puts a note into the inbox
  intake absorb [all]                    one pass over the folders of `absorb` (and the proposals of `deliver`)
  intake status                          the recordings waiting for a text per space, and the folders watched
  intake discover [--seconds N]          nodes found on this network and on the tailnet
  catalogue                              lists the kinds of vault this installation offers
  vault add KIND [--user PERSON]         creates a person's vault of a kind of the catalogue (named after them)
  space add NAME [--user PERSON] [--kind KIND] | space list [--kinds] | space kind NAME KIND
                                         a space named by hand, for an operator; it has no kind unless given one
  vocabulary import FILE --space NAME --name NAME [--source TEXT]
  vocabulary list [--space NAME] | remove NAME --space NAME
                                         a list of terms (signs, symptoms…) the router reads after the keywords
  library add PATH --space NAME          mounts an existing folder on a space, read-only: the node indexes it (path,
                                         title, dates, text of .md and .txt, [[links]]) and never copies or changes it
  library scan [--space NAME] | library list
                                         reads the mounted folders again (only what changed) | lists them
  space new LABEL [--kind K] [--parent S] [--keywords a,b] [--tags t] | space show NAME | space tree
  space set NAME [--label L] [--parent S | --top] [--position N] [--kind K]
  space delete NAME (--into SPACE | --archive) [--children grandparent|top|under:SPACE] [--dry-run]
                                         a space is never destroyed: its notes, files and tags go into another space,
                                         or it is archived (full export, database moved aside); sub-spaces move up
  space archived | space restore ARCHIVE [--dry-run]
  rules show|add-keyword|remove-keyword|add-tag|remove-tag SPACE [WORD]
  node list [--all] | node rename|note|pause|resume|revoke|forget PEER [TEXT] [--dry-run]
  connection show|rename|rotate|delete AGENT [NAME] [--dry-run]
  person list | person show|set|heir|recovery-key|disable|enable|delete NAME [--display D] [--role admin|member] [--dry-run]
  note add|show|list|edit|retire|restore SPACE [TEXT|ID] [--tag T] [--reason R]
  library remove FOLDER --space NAME [--dry-run] | vocabulary term-add|term-remove NAME TERM... --space NAME
                                         every command above takes --user PERSON (default: the first administrator)
  console-token PERSON                   a token that opens the admin page (/admin) where it is served, never MCP
  serve [--listen 127.0.0.1:8791] [--allow-remote] [--advertise] [--node-name NAME] [--allow-origin URL] [--threads N]
        [--console-admin]                the admin page on this listener, with a console token: on by
                                         default on the loopback only, never through Tailscale Funnel
        [--public-url URL] [--ingress-listen ADDR --trust-ingress IP --admins ha_login,...]
                                         admin page through Home Assistant ingress
        [--identity-listen ADDR [--identity-trust IP] --tailnet-user login=person,... --login-url URL] [--oauth-redirect URL]
                                         OAuth for Claude on the web: consent page behind Tailscale serve
  connect AGENT [--user PERSON] (--all | --grant health:rw,work:r)
        [--days N] [--read-only] [--rotate] [--internal] [--public-url URL]
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
  merge --plan EXPORT --user PERSON [--policy keep-both|keep-mine|take-theirs|replace] [--out FILE]
                                         reads another memory (an export) and writes a plan: for each of its spaces,
                                         the space it joins, copy or skip, and a preview; writes nothing here
  merge --apply PLAN [--backup-to DIR]   backs up, applies the plan in one go (all or nothing, nothing is deleted,
                                         twice changes nothing), verifies the counts
  connections                            lists the connections
  doctor [--public-url URL] [--listen ADDR]
                                         reads the installation and says what is fine and what deserves a look:
                                         files, versions, integrity, people, connections, the address. Writes nothing.
  regrant AGENT (--all | --grant ...)
                                         changes the rights, same token
  revoke AGENT                           cuts a connection
  internal AGENT on|off                  marks a connection as one of the program's own tools (kept out of the assistants)
  readonly AGENT on|off                  forces read-only
  person-guide AGENT on|off              off: the person's own guide is not told to this connection (a work assistant)
  limit AGENT rate|max-text N     sets a limit
  import --vault DIR --rules FILE [--dry-run] [--yes] [--only DIR] [--protect word,word]
                                         indexes a Markdown folder (first run: --dry-run)
  backup --to DIR [--keep 14]            consistent copy of every database (safe while serving), outside the data dir

Settings come from memcp.toml (--config FILE, MEMCP_CONFIG, ~/.config/memcp/memcp.toml, /etc/memcp/memcp.toml); a flag wins.
The directory may come from MEMCP_DATA. The default public address comes from MEMCP_PUBLIC_URL.";

const VALUE_FLAGS: &[&str] = &[
    "--config",
    "--out",
    "--person",
    "--assistant",
    "--label",
    "--parent",
    "--position",
    "--kind",
    "--into",
    "--children",
    "--keywords",
    "--tags",
    "--reason",
    "--tag",
    "--role",
    "--note",
    "--name",
    "--source",
    "--plan",
    "--apply",
    "--out",
    "--backup-to",
    "--policy",
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
    "--seconds",
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

    let cmd = memcp_server::cli_crud::Cmd {
        pos: &a.pos,
        flag: &flag,
        sw: &sw,
    };
    if let Some(done) = memcp_server::cli_crud::run(&data, &cmd) {
        return done;
    }

    match (a.pos[0].as_str(), a.pos.get(1).map(String::as_str)) {
        ("init", _) => {
            let code = flag("--language").unwrap_or("en");
            let language = Language::parse(code).ok_or_else(|| cli::bad("--language: en or fr"))?;
            cli::init(
                &data,
                &cli::parse_spaces(if sw("--defaults") {
                    "personal,health,home,finances,hobbies,work"
                } else {
                    flag("--spaces").unwrap_or("")
                })?,
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
        ("library", Some("add")) => {
            cli::library_add(&data, Path::new(arg(2, "the folder")?), need("--space")?)
        }
        ("library", Some("scan")) => cli::library_scan(&data, flag("--space")),
        ("library", Some("list")) => cli::library_list(&data),
        ("vocabulary", Some("import")) => cli::vocabulary_import(
            &data,
            Path::new(arg(2, "the file")?),
            need("--space")?,
            need("--name")?,
            flag("--source").unwrap_or(""),
        ),
        ("vocabulary", Some("list")) => cli::vocabulary_list(&data, flag("--space")),
        ("vocabulary", Some("remove")) => {
            cli::vocabulary_remove(&data, need("--space")?, arg(2, "the name")?)
        }
        ("prepare", Some(kind)) => {
            setup::prepare(kind, &data, &settings, Path::new(need("--out")?))
        }
        ("catalogue", _) => cli::catalogue_list(&data),
        ("serve", _) => serve(
            &data,
            &a,
            &settings,
            flag("--listen").unwrap_or("127.0.0.1:8791"),
        ),
        ("transcribe", _) => {
            memcp_server::intake::cli::transcribe(&data, &settings, flag("--user"))
        }
        ("watch", _) => {
            memcp_server::intake::cli::watch_folders(&data, &settings, flag("--user"), sw("--once"))
        }
        ("intake", _) => memcp_server::intake::cli::intake(
            &data,
            &settings,
            flag("--user"),
            &a.pos,
            flag("--space"),
            flag("--seconds"),
        ),
        ("connect", _) => cli::connect(
            &data,
            cli::ConnectOpts {
                agent: arg(1, "the agent name")?.to_string(),
                user: flag("--user").map(String::from),
                rights: rights()?,
                days: flag("--days").and_then(|d| d.parse().ok()),
                read_only: sw("--read-only"),
                rotate: sw("--rotate"),
                internal: sw("--internal"),
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
        ("merge", _) => cli::merge(
            &data,
            flag("--plan").map(Path::new),
            flag("--apply").map(Path::new),
            flag("--user"),
            flag("--out").map(Path::new),
            flag("--backup-to").map(Path::new),
            flag("--policy"),
        ),
        ("connections", _) => cli::connections(&data),
        ("doctor", _) => memcp_server::doctor::run_with(
            &data,
            flag("--public-url"),
            flag("--listen"),
            Some(&memcp_server::doctor::Extra {
                settings_file: cfg_path.as_deref(),
                settings: &settings,
            }),
        ),
        ("regrant", _) => cli::regrant(&data, arg(1, "the agent name")?, &rights()?),
        ("revoke", _) => cli::revoke(&data, arg(1, "the agent name")?),
        ("internal", _) => cli::internal(&data, arg(1, "the agent name")?, arg(2, "on or off")?),
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
            Path::new(
                &flag("--to")
                    .map(String::from)
                    .unwrap_or_else(|| settings.text_or_default("backup_dir")),
            ),
            flag("--keep").map_or(
                Ok(settings.int("backup_keep").map_or(14, |n| n as usize)),
                |k| k.parse().map_err(|_| cli::bad("--keep needs a number")),
            )?,
        ),
        (other, _) => Err((2, format!("unknown command {other:?}\n\n{USAGE}"))),
    }
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
