//! The command line's words: the usage text, the options, and the one function that runs a command on a data directory.
//! `main.rs` calls it for the local mode; the node calls it for a remote caller (see `cli_remote`), so the two print the
//! same text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::{self, Failure};
use crate::config;
use memcp_core::text::Language;

pub const USAGE: &str = "usage: memcp --data DIR <command>

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
  intake send FILE... [--space SPACE]    puts files into a space, tagged `inbox` (the Inbox by default): a text file becomes a note,
                                         audio a recording, the rest a file
  intake text TEXT | - [--space SPACE]   puts a note into the inbox
  intake absorb [all]                    one pass over the folders of `absorb` (and the proposals of `deliver`)
  intake status                          the recordings waiting for a text per space, and the folders watched
  intake discover [--seconds N]          nodes found on this network and on the tailnet
  catalogue                              lists the kinds of space this node offers
  space make KIND [--user PERSON]        creates a person's space of a kind of the catalogue (named after them;
                                         `vault add KIND`, its former name, still works)
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
  node policy PEER [SPACE both|send|receive|off] [--later both|send|receive|off]
  connection show|rename|rotate|delete AGENT [NAME] [--dry-run]
  person list | person show|set|heir|recovery-key|disable|enable|delete NAME [--display D] [--role admin|member] [--dry-run]
  note add|show|list|edit|retire|restore SPACE [TEXT|ID] [--tag T] [--reason R]
  library remove FOLDER --space NAME [--dry-run] | vocabulary term-add|term-remove NAME TERM... --space NAME
                                         every command above takes --user PERSON (default: the first administrator)
  console-token PERSON                   a token that opens the page of this node (/admin) where it is served, never MCP
  serve [--listen 127.0.0.1:8791] [--allow-remote] [--advertise] [--node-name NAME] [--allow-origin URL] [--threads N]
        [--console-admin]                the page on this listener, with a console token: on by
                                         default on the loopback only, never through Tailscale Funnel
        [--public-url URL] [--ingress-listen ADDR --trust-ingress IP --admins ha_login,...]
                                         the page through Home Assistant ingress
        [--identity-listen ADDR [--identity-trust IP] --tailnet-user login=person,... --login-url URL] [--oauth-redirect URL]
                                         OAuth for Claude on the web: consent page behind Tailscale serve
  connect AGENT [--user PERSON] (--all | --grant health:rw,work:r)
        [--days N] [--read-only] [--rotate] [--internal] [--public-url URL]
                                         creates an MCP connection and shows its token once
  open SPACE [--user PERSON] [--read-only] [--days N] [--public-url URL]
                                         opens one space to an assistant: a connection limited to it,
                                         token shown once
  mcp [--token-env NAME]                 MCP on standard input and output, for an assistant that
                                         runs a command: the key is read from MEMCP_TOKEN (or NAME)
  export [--user PERSON] --to DIR        writes a person's memory to a directory: each space they own (a copy,
                                         its files, a readable Markdown version), their instructions, a
                                         manifest of fingerprints; no key, connection or right
  verify-export DIR                      checks an export (no --data needed)
  skills-export DIR [--data DIR]         writes the professions of the spaces as Claude skills, a plugin marketplace (no --data needed)
  professions [accept|reject KIND/ID]    lists the professions the person added and those an assistant proposed; accepts or drops a proposal
  adopt DIR [--user PERSON] [--prefix P] takes the spaces of an export into this node
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
  import --folder DIR --rules FILE [--dry-run] [--yes] [--only DIR] [--protect word,word]
                                         indexes a Markdown folder (first run: --dry-run; --vault DIR still works)
  backup --to DIR [--keep 14]            consistent copy of every database (safe while serving), outside the data dir

  status [--user PERSON]                 what this memory holds: version, people, spaces with their counts, connections
  folder set SPACE FOLDER [--read] | folder set SPACE --default
                                         the local folder of a space ON THIS NODE (never shared with the other nodes):
                                         the node keeps the notes and files there as Markdown, both ways; --read: only read
  folder get [SPACE] | folder sync [SPACE]
                                         the folder of a space (path, mode, cloud, last pass) | one pass now
  person new NAME [--display D] [--role admin|member]
  note recall SPACE WORDS [--tag T] [--limit N] | note tags SPACE ID [--tag a,b | --add a,b | --remove a,b]
  connection list|new|regrant|revoke|internal|readonly|person-guide|limit ...
                                         the connection commands, under one word (`connections`, `connect`... still work)
  invite code|ticket|pending|approve ID|deny ID|allow DEVICE [--minutes N]
                                         devices joining the person's memory (a running node: --node)
  join LINK|ID|ADDRESS [--ticket TICKET] asks another node to let this one in (a running node: --node)

Against a running node, with a key (docs/CLI.md): `memcp --node ADDRESS --key-file FILE <command>`, or MEMCP_NODE and
MEMCP_KEY_FILE / MEMCP_KEY. The same commands print the same text; --json prints the node's JSON; --agent NAME (or
MEMCP_AGENT) with a connection key does the note commands through the assistants' tools; --local forces the data directory.
Exit codes: 0 done, 1 failed, 2 wrong command line, 3 key refused, 4 key lacks the rights, 5 node unreachable, 6 odd answer.

Settings come from memcp.toml (--config FILE, MEMCP_CONFIG, ~/.config/memcp/memcp.toml, /etc/memcp/memcp.toml); a flag wins.
The directory may come from MEMCP_DATA. The default public address comes from MEMCP_PUBLIC_URL.";

pub const VALUE_FLAGS: &[&str] = &[
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
    "--folder",
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
    "--node",
    "--key-file",
    "--key",
    "--agent",
    "--ticket",
    "--prefix",
    "--token-env",
    "--minutes",
    "--limit",
    "--add",
    "--remove",
    "--later",
];

pub struct Args {
    pub flags: HashMap<String, String>,
    pub switches: Vec<String>,
    pub pos: Vec<String>,
}

pub fn parse(args: &[String]) -> Result<Args, Failure> {
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

pub fn list(v: Option<&str>) -> Vec<String> {
    v.map(|p| {
        p.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// `connection list|new|regrant|revoke|internal|readonly|person-guide|limit ...` are the verbs of the `connection` family
/// under the names the commands always had (`connections`, `connect`, `regrant`...): the same command, said either way.
pub fn normalize(a: &Args) -> Args {
    let mut b = Args {
        flags: a.flags.clone(),
        switches: a.switches.clone(),
        pos: a.pos.clone(),
    };
    if b.pos.first().map(String::as_str) == Some("connection") {
        let verb = b.pos.get(1).cloned().unwrap_or_default();
        let target = match verb.as_str() {
            "list" => Some("connections"),
            "new" => Some("connect"),
            "regrant" | "revoke" | "internal" | "readonly" | "person-guide" | "limit" => {
                Some(verb.as_str())
            }
            _ => None,
        };
        if let Some(t) = target {
            b.pos.remove(0);
            b.pos[0] = t.to_string();
        }
    }
    b
}

/// Runs a command that works on the data directory (everything but `serve`, `setup`, `config`, `verify-export`).
pub fn dispatch(
    data: &Path,
    a: &Args,
    settings: &config::Settings,
    cfg_path: Option<&Path>,
) -> Result<(), Failure> {
    let data: PathBuf = data.to_path_buf();
    let normal = normalize(a);
    let a = &normal;
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

    let cmd = crate::cli_crud::Cmd {
        pos: &a.pos,
        flag: &flag,
        sw: &sw,
    };
    if let Some(done) = crate::cli_crud::run(&data, &cmd) {
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
        // `vault add`: the former name of `space make`, kept so that a script written for it still works.
        ("space", Some("make")) | ("vault", Some("add")) => {
            cli::vault_add(&data, arg(2, "the kind")?, flag("--user"))
        }
        ("folder", Some("set")) => cli::folder_set(
            &data,
            arg(2, "the space")?,
            if sw("--default") || sw("--clear") {
                None
            } else {
                Some(Path::new(arg(3, "the folder (or --default)")?))
            },
            if sw("--read") {
                memcp_core::folders::Mode::Read
            } else {
                memcp_core::folders::Mode::ReadWrite
            },
        ),
        ("folder", Some("get")) => cli::folder_get(&data, a.pos.get(2).map(String::as_str)),
        ("folder", Some("sync")) => cli::folder_sync(&data, a.pos.get(2).map(String::as_str)),
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
            crate::setup::prepare(kind, &data, settings, Path::new(need("--out")?))
        }
        ("catalogue", _) => cli::catalogue_list(&data),
        ("professions", None) => {
            for p in memcp_core::profession::pending(&data) {
                println!("proposed  {}/{}  {}", p["kind"].as_str().unwrap_or(""), p["id"].as_str().unwrap_or(""), p["title"].as_str().unwrap_or(""));
            }
            let file = data.join(memcp_core::profession::FILE);
            println!(
                "the person's own profiles are in {} ({})",
                file.display(),
                if file.exists() { "present" } else { "none yet" }
            );
            Ok(())
        }
        ("professions", Some(verb @ ("accept" | "reject"))) => {
            let what = arg(2, "KIND/ID")?;
            let (kind, id) = what
                .split_once('/')
                .ok_or_else(|| cli::bad("KIND/ID, for example hobbies/gamemaster"))?;
            let done = memcp_core::profession::decide(&data, kind, id, verb == "accept").map_err(|e| (1, e))?;
            if done {
                println!("{} {what}", if verb == "accept" { "accepted" } else { "dropped" });
                Ok(())
            } else {
                Err((1, format!("no proposal {what}")))
            }
        }
        ("status", _) => cli::status(&data, flag("--user")),
        // What a running node holds in its memory (the requests waiting, the tickets): there is nothing to read in a directory.
        ("invite" | "join", _) => Err((
            2,
            format!(
                "`{}` talks to a running node: add --node ADDRESS and a key (--key-file FILE); see docs/CLI.md",
                a.pos[0]
            ),
        )),
        // The folder of a space: the routes it calls (`/admin/api/space/folder`) and the tools (`folder_set`, `folder_get`)
        // come with the branch `space-folders`; until it is merged the command says so.
        ("folder", _) => Err((
            1,
            "folder: not available yet in this build (it comes with the space folders); the remote form is defined and \
             calls /admin/api/space/folder"
                .into(),
        )),
        ("transcribe", _) => {
            crate::intake::cli::transcribe(&data, settings, flag("--user"))
        }
        ("watch", _) => {
            crate::intake::cli::watch_folders(&data, settings, flag("--user"), sw("--once"))
        }
        ("intake", _) => crate::intake::cli::intake(
            &data,
            settings,
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
            arg(1, "the space")?,
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
        ("doctor", _) => crate::doctor::run_with(
            &data,
            flag("--public-url"),
            flag("--listen"),
            Some(&crate::doctor::Extra {
                settings_file: cfg_path,
                settings,
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
            Path::new(match flag("--vault") {
                Some(old) if flag("--folder").is_none() => old,
                _ => need("--folder")?,
            }),
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
