//! `memcp setup`, `memcp config` and `memcp prepare`: what a Linux installation needs around the server.
//!
//! - `setup` makes the data directory (private), the first memory, the node name, the first person, the default spaces
//!   and the settings file, then says where the page is and prints the one address with the key for an assistant.
//! - `config check|show|example` reads and explains `memcp.toml` (see [`crate::config`]).
//! - `prepare janitor|transcribe` is run by the service units before the janitor or the voice reader: it makes their key
//!   and their settings file from `memcp.toml`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use memcp_core::access::{self, Rights};
use memcp_core::text::Language;
use memcp_core::{Brain, directory, users};

use crate::cli::{Failure, bad, fail};
use crate::config::{self, Settings, Value};
use crate::replica::DEFAULT_SPACES;

/// What `setup` was told on the command line.
#[derive(Default)]
pub struct SetupArgs {
    pub data: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub node_name: Option<String>,
    pub language: Option<String>,
    pub listen: Option<String>,
    pub public_url: Option<String>,
    pub person: Option<String>,
    pub spaces: Option<String>,
    pub assistant: Option<String>,
    pub yes: bool,
    pub system: bool,
    pub force: bool,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn ask(question: &str, default: &str, interactive: bool) -> String {
    if !interactive {
        return default.to_string();
    }
    print!("{question} [{default}] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return default.to_string();
    }
    let line = line.trim();
    if line.is_empty() {
        default.to_string()
    } else {
        line.to_string()
    }
}

fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

/// An address a client on this machine can use for a listen address.
fn local_address(listen: &str) -> String {
    match listen.rsplit_once(':') {
        Some((host, port)) if host.is_empty() || host == "0.0.0.0" || host == "[::]" => {
            format!("127.0.0.1:{port}")
        }
        _ => listen.to_string(),
    }
}

/// Where setup writes the settings: the one named; else the system's with `--system`; else the user's.
fn target_config(a: &SetupArgs) -> PathBuf {
    if a.system {
        return PathBuf::from("/etc/memcp/memcp.toml");
    }
    config::user_target().unwrap_or_else(|| PathBuf::from("/etc/memcp/memcp.toml"))
}

pub fn setup(a: SetupArgs, existing: Option<PathBuf>) -> Result<(), Failure> {
    if a.system && !is_root() {
        return Err(bad(&format!(
            "--system makes /var/lib/memcp and /etc/memcp: run it as root (sudo {} setup --system)",
            crate::commands::command_name(),
        )));
    }
    let interactive = !a.yes && std::io::stdin().is_terminal();
    let mut settings = match &existing {
        Some(p) => Settings::load(p).map_err(bad_text)?,
        None => Settings::default(),
    };
    let cfg_path = a
        .config
        .clone()
        .or(existing)
        .unwrap_or_else(|| target_config(&a));

    println!("Silice setup\n");
    let default_data = a
        .data
        .clone()
        .or_else(|| settings.text("data").map(PathBuf::from))
        .unwrap_or_else(|| {
            if a.system {
                PathBuf::from("/var/lib/memcp")
            } else {
                home().join("memory")
            }
        });
    let data = PathBuf::from(ask(
        "Where should the memory live?",
        &default_data.to_string_lossy(),
        interactive && a.data.is_none(),
    ));
    let stored_name = if data.join("control.sqlite").is_file() {
        let brain = Brain::open(&data).map_err(fail)?;
        directory::stored_own_name(&brain).map_err(fail)?
    } else {
        None
    };
    let node_name = ask(
        "Name of this node",
        a.node_name
            .as_deref()
            .or_else(|| settings.text("node_name"))
            .or(stored_name.as_deref())
            .unwrap_or(config::DEFAULT_NODE_NAME),
        interactive && a.node_name.is_none(),
    );
    let language = ask(
        "Language of search and of the page (en or fr)",
        a.language
            .as_deref()
            .unwrap_or(&settings.text_or_default("language")),
        interactive && a.language.is_none(),
    );
    let listen = ask(
        "Listen address (the loopback unless a proxy needs more)",
        a.listen
            .as_deref()
            .unwrap_or(&settings.text_or_default("listen")),
        interactive && a.listen.is_none(),
    );
    let public_default = a
        .public_url
        .clone()
        .or_else(|| settings.text("public_url").map(String::from))
        .unwrap_or_default();
    let public_url = ask(
        "Public address assistants reach, https://... (empty: this machine only)",
        &public_default,
        interactive && a.public_url.is_none(),
    );
    let person = ask(
        "Your name (the first person of this memory)",
        a.person.as_deref().unwrap_or("Owner"),
        interactive && a.person.is_none(),
    );
    let spaces_default = a.spaces.clone().unwrap_or_else(|| DEFAULT_SPACES.join(","));
    let spaces = ask(
        "First spaces, comma separated",
        &spaces_default,
        interactive && a.spaces.is_none(),
    );

    let lang = Language::parse(&language).ok_or_else(|| bad("language: en or fr"))?;
    if !public_url.is_empty() {
        settings
            .set(
                "public_url",
                Value::Text(public_url.trim_end_matches('/').into()),
            )
            .map_err(bad_text)?;
    }
    settings
        .set("data", Value::Text(data.to_string_lossy().into()))
        .map_err(bad_text)?;
    settings
        .set("node_name", Value::Text(node_name.clone()))
        .map_err(bad_text)?;
    settings
        .set("language", Value::Text(language.clone()))
        .map_err(bad_text)?;
    settings
        .set("listen", Value::Text(listen.clone()))
        .map_err(bad_text)?;

    // The memory: once. An existing one is never touched.
    let fresh = !data.join("control.sqlite").is_file();
    let space_list = crate::cli::parse_spaces(&spaces)?;
    if fresh {
        Brain::init(&data, &space_list, lang).map_err(fail)?;
        println!("memory made in {} (private to its owner)", data.display());
    } else {
        println!("{} already holds a memory: left as it is.", data.display());
    }
    let brain = Brain::open(&data).map_err(fail)?;
    memcp_core::text::set_language(brain.language().map_err(fail)?);
    let ctl = brain.open_control().map_err(fail)?;
    let me = users::resolve(&ctl, users::OWNER).map_err(fail)?;
    if fresh || !person.is_empty() && me.display.is_empty() {
        users::set_display(&ctl, &me.id, &person).map_err(fail)?;
    }
    if stored_name.as_deref() != Some(&node_name) {
        directory::set_own_name(&brain, &node_name).map_err(fail)?;
    }
    let mut granted = vec![];
    for s in brain.spaces().map_err(fail)? {
        granted.push(format!("{s}:rw"));
    }

    // The settings file: written when there is none (or on request); otherwise left as the administrator made it.
    let wrote = if cfg_path.is_file() && !a.force {
        println!(
            "{} exists: left as it is (--force rewrites it).",
            cfg_path.display()
        );
        false
    } else {
        if let Some(dir) = cfg_path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        std::fs::write(&cfg_path, settings.render()).map_err(fail)?;
        println!("settings written to {}", cfg_path.display());
        true
    };

    if a.system {
        // The service runs as its own user: the data belongs to it.
        let who = std::process::Command::new("id")
            .args(["-u", "memcp"])
            .output()
            .ok()
            .is_some_and(|o| o.status.success());
        if who {
            let _ = std::process::Command::new("chown")
                .args(["-R", "memcp:memcp"])
                .arg(&data)
                .status();
        } else {
            println!(
                "note: no user `memcp`; make it, then `chown -R memcp: {}`.",
                data.display()
            );
        }
    }

    // The way in: the page and one assistant.
    let local = local_address(&listen);
    let public = if public_url.is_empty() {
        format!("http://{local}")
    } else {
        public_url.trim_end_matches('/').to_string()
    };
    let console = access::console_token(&ctl, users::OWNER).map_err(fail)?;
    println!("\nYour page, on this machine only:\n  http://{local}/admin");
    println!("Console key (shown once, it opens the page and never the memory):\n  {console}");
    let agent = a.assistant.as_deref().unwrap_or("assistant");
    let rights = Rights::parse(&granted.join(",")).map_err(fail)?;
    match access::connect_for(&ctl, &me.id, agent, &rights, None, false, false) {
        Ok(token) => {
            println!(
                "\nOne address for an assistant, with its key in it (shown once):\n  {public}/mcp/{agent}?key={token}"
            );
            println!(
                "Claude Code:\n  claude mcp add --transport http {agent} {public}/mcp/{agent} --header \"Authorization: Bearer {token}\""
            );
            if public_url.is_empty() {
                println!(
                    "It works from this machine only; put HTTPS in front and set public_url for the others (docs/LINUX.md)."
                );
            }
        }
        Err(_) => println!(
            "\nThe connection {agent:?} exists already: `{} connect {agent} --all --rotate` gives it a new key.",
            crate::commands::command_name()
        ),
    }
    if !wrote {
        println!(
            "\nThe settings you gave were not saved: edit {}.",
            cfg_path.display()
        );
    }
    println!(
        "\nStart it: {name} serve   (or: systemctl enable --now memcp)\nCheck it: {name} doctor",
        name = crate::commands::command_name()
    );
    Ok(())
}

fn bad_text(e: String) -> Failure {
    (2, e)
}

/// `config check|show|example`.
pub fn config_command(sub: &str, path: Option<PathBuf>) -> Result<(), Failure> {
    match sub {
        "example" => {
            print!("{}", Settings::default().render());
            Ok(())
        }
        "check" | "show" => {
            let Some(path) = path else {
                println!(
                    "no settings file found (looked at {}): the defaults apply.",
                    places()
                );
                return Ok(());
            };
            let s = Settings::load(&path).map_err(bad_text)?;
            if sub == "show" {
                print!("{}", s.render());
                return Ok(());
            }
            println!("{}: ok", path.display());
            for w in config::describe(&s) {
                println!("warn  {w}");
            }
            Ok(())
        }
        _ => Err(bad("config check | show | example")),
    }
}

fn places() -> String {
    config::default_places()
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The key of a service connection kept in `dir/key`: used if the memory still accepts it, else made again.
fn service_key(brain: &Brain, user: &str, agent: &str, dir: &Path) -> Result<String, Failure> {
    let ctl = brain.open_control().map_err(fail)?;
    let file = dir.join("key");
    if let Ok(old) = std::fs::read_to_string(&file) {
        let old = old.trim();
        if !old.is_empty()
            && access::authenticate(&ctl, old, memcp_core::util::now())
                .map_err(fail)?
                .is_some()
        {
            return Ok(old.to_string());
        }
    }
    let who = users::resolve(&ctl, user).map_err(fail)?;
    let token =
        match access::connect_for(&ctl, &who.id, agent, &Rights::owner(), None, false, false) {
            Ok(t) => t,
            Err(_) => {
                access::connect_for(&ctl, &who.id, agent, &Rights::owner(), None, false, true)
                    .map_err(fail)?
            }
        };
    write_private(&file, &token)?;
    Ok(token)
}

fn write_private(path: &Path, text: &str) -> Result<(), Failure> {
    std::fs::write(path, text).map_err(fail)?;
    memcp_core::fsx::set_private_file(path).map_err(fail)?;
    Ok(())
}

/// `prepare janitor|transcribe --out DIR`: the key and the settings file of a companion service.
pub fn prepare(kind: &str, data: &Path, s: &Settings, out: &Path) -> Result<(), Failure> {
    std::fs::create_dir_all(out).map_err(fail)?;
    memcp_core::fsx::set_private_dir(out).map_err(fail)?;
    let brain = Brain::open(data).map_err(fail)?;
    let listen = local_address(&s.text_or_default("listen"));
    match kind {
        "janitor" => {
            if !s.flag("janitor") {
                eprintln!(
                    "{} prepare: janitor = false in memcp.toml",
                    crate::commands::command_name()
                );
                return Err((78, "the janitor is off".into()));
            }
            if s.text("janitor_api_url").is_none() && s.text("janitor_ollama_url").is_none() {
                return Err((
                    78,
                    "no model: set janitor_api_url or janitor_ollama_url in memcp.toml".into(),
                ));
            }
            let person = s.text_or_default("janitor_person");
            let key = service_key(&brain, &person, "janitor", out)?;
            let _ = key;
            let mut conf = format!(
                "url = http://{listen}/mcp/janitor\nkey_file = {}\nstate_dir = {}\n",
                out.join("key").display(),
                out.join("state").display()
            );
            let inboxes = s.list("janitor_inboxes").join(",");
            if !inboxes.is_empty() {
                conf.push_str(&format!("inbox_vaults = {inboxes}\n"));
            }
            let words = s.list("janitor_work_words").join(",");
            if !words.is_empty() {
                conf.push_str(&format!("work_words = {words}\n"));
            }
            let mut models = vec![];
            if let Some(u) = s.text("janitor_api_url") {
                conf.push_str(&format!(
                    "api_url = {u}\napi_model = {}\n",
                    s.text_or_default("janitor_api_model")
                ));
                if let Some(k) = s.text("janitor_api_key_file") {
                    conf.push_str(&format!("api_key_file = {k}\n"));
                }
                models.push("api");
            }
            if let Some(u) = s.text("janitor_ollama_url") {
                conf.push_str(&format!(
                    "ollama_url = {u}\nollama_model = {}\n",
                    s.text_or_default("janitor_ollama_model")
                ));
                models.push("local");
            }
            conf.push_str(&format!("models = {}\n", models.join(",")));
            write_private(&out.join("janitor.conf"), &conf)?;
            let env = format!(
                "JANITOR_ARGS=--watch {} --quiet {} --night {}\n",
                data.join(memcp_core::tools::INBOX_WRITTEN).display(),
                s.int("janitor_quiet").unwrap_or(120),
                s.text_or_default("janitor_night")
            );
            write_private(&out.join("janitor.env"), &env)?;
            println!(
                "janitor: settings in {}",
                out.join("janitor.conf").display()
            );
            Ok(())
        }
        // Voice notes are the node's own work now (it reads them itself); an old unit that still calls this has nothing to
        // prepare, and the settings file an older version left is removed.
        "transcribe" => {
            let _ = std::fs::remove_file(out.join("bridge.conf"));
            println!("transcribe: the node reads voice notes itself, nothing to prepare");
            Ok(())
        }
        _ => Err(bad("prepare janitor")),
    }
}
