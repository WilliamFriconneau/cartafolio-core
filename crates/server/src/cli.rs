//! The owner's commands (command line).

use std::path::Path;

use memcp_core::access::{self, Rights};
use memcp_core::export;
use memcp_core::import::{self, ImportOptions, ImportRules};
use memcp_core::text::{self, Language};
use memcp_core::util::{iso, now};
use memcp_core::{Brain, Space, users};

pub type Failure = (u8, String);

pub fn fail<E: std::fmt::Display>(e: E) -> Failure {
    (1, e.to_string())
}

pub fn bad(m: &str) -> Failure {
    (2, m.to_string())
}

fn open(data: &Path) -> Result<Brain, Failure> {
    let brain = Brain::open(data).map_err(fail)?;
    text::set_language(brain.language().map_err(fail)?);
    Ok(brain)
}

pub fn parse_spaces(list: &str) -> Result<Vec<Space>, Failure> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| Space::new(s).map_err(fail))
        .collect()
}

pub fn init(data: &Path, spaces: &[Space], language: Language) -> Result<(), Failure> {
    let brain = Brain::init(data, spaces, language).map_err(fail)?;
    let names: Vec<&str> = spaces.iter().map(Space::as_str).collect();
    sayln!(
        "initialised: {} (spaces: {}; search language: {})",
        brain.dir().display(),
        if names.is_empty() {
            "none yet".into()
        } else {
            names.join(", ")
        },
        language.code()
    );
    Ok(())
}

pub fn space_add(
    data: &Path,
    name: &str,
    user: Option<&str>,
    kind: Option<&str>,
) -> Result<(), Failure> {
    let brain = open(data)?;
    let owner = person(&brain, user)?;
    let space = Space::new(name).map_err(fail)?;
    brain.add_space_for(&space, &owner.id).map_err(fail)?;
    if let Some(k) = kind {
        brain.set_space_kind(&space, k).map_err(fail)?;
    }
    sayln!(
        "space {space} created, owned by {}{}. Share it with `space share` or a group.",
        owner.name,
        kind.map_or(String::new(), |k| format!(", kind {k}"))
    );
    Ok(())
}

/// Creates a person's vault of a kind of the catalogue, named after them and the kind.
pub fn vault_add(data: &Path, kind: &str, user: Option<&str>) -> Result<(), Failure> {
    let brain = open(data)?;
    let owner = person(&brain, user)?;
    let space = brain
        .add_vault(&owner.id, &owner.name, kind)
        .map_err(fail)?;
    sayln!("space {space} created for {}, kind {kind}.", owner.name);
    Ok(())
}

/// Gives an existing space a kind of the catalogue (or `custom`).
pub fn space_kind(data: &Path, name: &str, kind: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let space = Space::new(name).map_err(fail)?;
    brain.set_space_kind(&space, kind).map_err(fail)?;
    sayln!("space {space} is now of kind {kind}.");
    Ok(())
}

/// The kinds of vault this installation offers.
pub fn catalogue_list(data: &Path) -> Result<(), Failure> {
    for k in open(data)?.catalogue().map_err(fail)? {
        sayln!(
            "{:<12} {:<12} {}{}",
            k.key,
            k.label,
            k.description,
            k.feature
                .map_or(String::new(), |f| format!("  [{} tools]", f.word()))
        );
    }
    Ok(())
}

/// The person given by `--user` (id or login name), or the bootstrap administrator.
fn person(brain: &Brain, user: Option<&str>) -> Result<users::User, Failure> {
    let ctl = brain.open_control().map_err(fail)?;
    users::resolve(&ctl, user.unwrap_or(users::OWNER)).map_err(fail)
}

pub fn console_token(data: &Path, user: &str) -> Result<(), Failure> {
    let ctl = open(data)?.open_control().map_err(fail)?;
    let token = access::console_token(&ctl, user).map_err(fail)?;
    sayln!("Console token (shown ONCE; opens /admin, never MCP):\n  {token}");
    Ok(())
}

/// The spaces, one name per line; with `kinds`, `name<TAB>kind`.
pub fn space_list(data: &Path, kinds: bool) -> Result<(), Failure> {
    let brain = open(data)?;
    for s in brain.spaces().map_err(fail)? {
        if kinds {
            sayln!("{s}\t{}", brain.space_kind(&s).map_err(fail)?);
        } else {
            sayln!("{s}");
        }
    }
    Ok(())
}

/// `--all`, or `--grant health:rw,work:r`. A connection reaches memory and nothing else.
pub fn rights(all: bool, grant: Option<&str>) -> Result<Rights, Failure> {
    match (all, grant) {
        (true, None) => Ok(Rights::owner()),
        (false, Some(g)) => Rights::parse(g).map_err(fail),
        _ => Err(bad("give either --all or --grant space:rw,space:r")),
    }
}

pub struct ConnectOpts {
    pub agent: String,
    pub user: Option<String>,
    pub rights: Rights,
    pub days: Option<i64>,
    pub read_only: bool,
    pub rotate: bool,
    pub internal: bool,
    pub url: String,
}

pub fn connect(data: &Path, o: ConnectOpts) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = person(&brain, o.user.as_deref())?;
    let ctl = brain.open_control().map_err(fail)?;
    // The owner of the node, on its own command line, may make the connection of a machine by hand: a name `peer-…`
    // makes one (no expiry, never read-only). Nowhere else does a name make a machine.
    let token = if o.agent.starts_with(memcp_core::sync::PEER_PREFIX) {
        if o.days.is_some() || o.read_only {
            return Err(bad(
                "the connection of a machine has no --days and is not --read-only",
            ));
        }
        access::connect_peer(&ctl, &who.id, &o.agent, &o.rights, o.rotate)
    } else {
        access::connect_for(
            &ctl,
            &who.id,
            &o.agent,
            &o.rights,
            o.days,
            o.read_only,
            o.rotate,
        )
    }
    .map_err(fail)?;
    if o.internal {
        access::set_internal(&ctl, &o.agent, true).map_err(fail)?;
    }
    let info = access::list(&ctl)
        .map_err(fail)?
        .into_iter()
        .find(|c| c.agent == o.agent)
        .ok_or_else(|| fail("connection not found after creation"))?;
    sayln!(
        "Connection {:?} {}: rights {}{}.",
        o.agent,
        if o.rotate { "renewed" } else { "created" },
        info.rights,
        if info.read_only_forced {
            ", read-only"
        } else {
            ""
        }
    );
    sayln!("\nToken (shown ONCE, cannot be recovered):\n  {token}\n");
    sayln!(
        "Claude Code:\n  claude mcp add --transport http {a} {}/mcp/{a} --header \"Authorization: Bearer {token}\"\n",
        o.url,
        a = o.agent
    );
    sayln!(
        "Any other MCP client: address {}/mcp/{}, header \"Authorization: Bearer <token>\".",
        o.url,
        o.agent
    );
    Ok(())
}

/// Opens one vault to an assistant: a connection limited to it, token shown once.
pub fn open_vault(
    data: &Path,
    vault: &str,
    user: Option<&str>,
    read_only: bool,
    days: Option<i64>,
    url: &str,
) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = person(&brain, user)?;
    let space = Space::new(vault).map_err(fail)?;
    let ctl = brain.open_control().map_err(fail)?;
    let (agent, token) =
        access::open_space_connection(&ctl, &who.id, &space, !read_only, days).map_err(fail)?;
    sayln!(
        "Space {space} opened to {agent:?}, {}.",
        if read_only {
            "read-only"
        } else {
            "read and write"
        }
    );
    sayln!(
        "\nAddress:\n  {url}/mcp/{agent}\n\nToken (shown ONCE, cannot be recovered):\n  {token}\n"
    );
    sayln!(
        "Claude Code:\n  claude mcp add --transport http {agent} {url}/mcp/{agent} --header \"Authorization: Bearer {token}\"\n"
    );
    sayln!(
        "Any other MCP client: address {url}/mcp/{agent}, header \"Authorization: Bearer <token>\"."
    );
    let data_abs = std::path::absolute(data).unwrap_or_else(|_| data.to_path_buf());
    sayln!(
        "\nA local assistant that runs a command (no network, nothing exposed):\n  command: memcp\n  arguments: --data {} mcp\n  environment: MEMCP_TOKEN=<token>",
        data_abs.display()
    );
    Ok(())
}

/// Writes a person's memory to a directory (see the `export` module) and checks what it wrote.
pub fn export_person(data: &Path, user: Option<&str>, to: &Path) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = person(&brain, user)?;
    let dir = export::export_person(&brain, &who, to, now()).map_err(fail)?;
    let manifest = export::verify_export(&dir).map_err(fail)?;
    export_report(&who.name, &dir, &manifest);
    Ok(())
}

/// What an export says about itself, the same for the one written here and the one that came down from a node.
pub fn export_report(person: &str, dir: &Path, manifest: &serde_json::Value) {
    sayln!("exported the memory of {person} to {}", dir.display());
    for v in memcp_core::export::manifest_spaces(manifest)
        .into_iter()
        .flatten()
    {
        sayln!(
            "  {:<24} {:<10} facts {}  files {}",
            v["name"].as_str().unwrap_or(""),
            v["kind"].as_str().unwrap_or(""),
            v["facts"],
            v["files"]
        );
    }
    sayln!("checked: every file matches its fingerprint. Readable versions are in readable/.");
}

/// Checks an export directory without any installation.
pub fn verify_export(dir: &Path) -> Result<(), Failure> {
    let manifest = export::verify_export(dir).map_err(fail)?;
    sayln!(
        "ok: export of {} made on {}, {} space(s), every file matches its fingerprint.",
        manifest["person"]["name"].as_str().unwrap_or("?"),
        manifest["created_at"].as_str().unwrap_or("?"),
        memcp_core::export::manifest_spaces(&manifest).map_or(0, Vec::len)
    );
    Ok(())
}

/// Takes the vaults of an export into this installation, as the property of a person.
pub fn adopt(
    data: &Path,
    dir: &Path,
    user: Option<&str>,
    prefix: Option<&str>,
) -> Result<(), Failure> {
    let brain = open(data)?;
    let owner = person(&brain, user)?;
    let done = export::adopt(&brain, dir, &owner, prefix).map_err(fail)?;
    for a in &done {
        sayln!(
            "adopted {:<24} {:<10} facts {}  files {}",
            a.name,
            a.kind,
            a.facts,
            a.files
        );
    }
    sayln!(
        "{} space(s) now belong to {}. Instructions came with them; connections do not: make new ones.",
        done.len(),
        owner.name
    );
    Ok(())
}

/// Plans or applies the merge of another memory (an export) into this installation.
pub fn merge(
    data: &Path,
    plan_of: Option<&Path>,
    apply_of: Option<&Path>,
    user: Option<&str>,
    out: Option<&Path>,
    backup_to: Option<&Path>,
    policy: Option<&str>,
) -> Result<(), Failure> {
    use memcp_core::merge::{self, Plan, Policy};
    let brain = open(data)?;
    match (plan_of, apply_of) {
        (Some(export_dir), None) => {
            let who = person(&brain, user)?;
            let policy = Policy::parse(policy.unwrap_or("keep-both")).map_err(fail)?;
            let plan = merge::plan(&brain, export_dir, &who, policy).map_err(fail)?;
            for l in &plan.lines {
                let p = &l.preview;
                esayln!(
                    "{:<24} {:<5} -> {:<24} new {} already {} differing {} files {} ({})",
                    l.source,
                    l.action.word(),
                    l.destination(),
                    p.new,
                    p.already,
                    p.differing,
                    p.files_new,
                    l.note
                );
            }
            match out {
                Some(path) => {
                    std::fs::write(path, plan.to_text()).map_err(fail)?;
                    esayln!(
                        "plan written to {}; nothing else was written.",
                        path.display()
                    );
                }
                None => say!("{}", plan.to_text()),
            }
            Ok(())
        }
        (None, Some(file)) => {
            let text = std::fs::read_to_string(file).map_err(fail)?;
            let plan = Plan::from_text(&text).map_err(fail)?;
            let dest = match backup_to {
                Some(p) => p.to_path_buf(),
                None => {
                    let abs = std::fs::canonicalize(data).map_err(fail)?;
                    let mut name = abs.file_name().unwrap_or_default().to_os_string();
                    name.push("-merge-backups");
                    abs.with_file_name(name)
                }
            };
            let opts = merge::Options {
                backup_to: dest,
                at: now(),
                stop_after: None,
            };
            let report = merge::apply(&brain, &plan, &opts).map_err(fail)?;
            match &report.backup {
                Some(b) => sayln!("backup: {}", b.display()),
                None => sayln!("nothing to change: the memories already agree on this plan."),
            }
            for l in &report.lines {
                sayln!(
                    "{:<24} -> {:<24}{} added {} tagged {} replaced {} left {} files {}",
                    l.source,
                    l.destination,
                    if l.created { " (new)" } else { "" },
                    l.added,
                    l.tagged,
                    l.replaced,
                    l.left,
                    l.files
                );
            }
            for (space, _) in &report.guide_proposals {
                sayln!(
                    "the instructions of the other memory for {space} differ: not applied (policy replace applies them)."
                );
            }
            Ok(())
        }
        _ => Err(bad(
            "merge --plan EXPORT --user PERSON, or merge --apply PLAN",
        )),
    }
}

/// The MCP server on standard input and output, with the rights of the key in the environment
/// variable `token_env`. Nothing but protocol goes to standard output.
pub fn mcp_stdio(data: &Path, token_env: &str) -> Result<(), Failure> {
    let token = std::env::var(token_env).map_err(|_| {
        (
            2,
            format!("set {token_env} to a key (make one with `memcp open SPACE`)"),
        )
    })?;
    let brain = open(data)?;
    // What a crash cut off is finished or undone first (nothing goes to stdout: it is the protocol's).
    match brain.recover() {
        Ok(r) if r.is_clean() && r.swept => {}
        Ok(r) => eprintln!("{}", r.summary()),
        Err(e) => eprintln!("recovery: not finished yet: {e}"),
    }
    let app = crate::app::App::new(brain);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    crate::stdio::run(&app, token.trim(), stdin.lock(), stdout.lock()).map_err(|e| (1, e))
}

/// `status`: what this memory holds, in a few lines: version, people, spaces with their counts, connections.
pub fn status(data: &Path, user: Option<&str>) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = person(&brain, user)?;
    let ctl = brain.open_control().map_err(fail)?;
    sayln!(
        "memcp {}, search language {}",
        memcp_core::directory::own_version(),
        brain.language().map_err(fail)?.code()
    );
    let people = users::list_users(&ctl).map_err(fail)?;
    sayln!(
        "people: {} ({} administrator(s), {} disabled)",
        people.len(),
        people.iter().filter(|p| p.admin).count(),
        people.iter().filter(|p| p.disabled).count()
    );
    let spaces = brain.spaces().map_err(fail)?;
    sayln!("spaces: {}", spaces.len());
    for s in &spaces {
        match brain.space_info(&who.id, s) {
            Ok(i) => sayln!(
                "  {:<24} {:<10} {} notes, {} files",
                s,
                i.kind,
                i.facts,
                i.files
            ),
            Err(_) => sayln!("  {s}"),
        }
    }
    let conns = access::list(&ctl).map_err(fail)?;
    sayln!(
        "connections: {} active, {} revoked",
        conns.iter().filter(|c| c.revoked_at.is_none()).count(),
        conns.iter().filter(|c| c.revoked_at.is_some()).count()
    );
    Ok(())
}

pub fn connections(data: &Path) -> Result<(), Failure> {
    let ctl = open(data)?.open_control().map_err(fail)?;
    let list = access::list(&ctl).map_err(fail)?;
    if list.is_empty() {
        sayln!("No connection.");
    }
    for c in list {
        let state = if c.revoked_at.is_some() {
            "revoked"
        } else if c.read_only_forced {
            "read-only"
        } else {
            "active"
        };
        sayln!(
            "{:<24} {:<34} {:<10} last used: {}",
            c.agent,
            c.rights,
            state,
            c.last_used_at.map_or("never".into(), iso)
        );
    }
    Ok(())
}

pub fn revoke(data: &Path, agent: &str) -> Result<(), Failure> {
    let ctl = open(data)?.open_control().map_err(fail)?;
    if access::revoke(&ctl, agent).map_err(fail)? {
        sayln!("Connection {agent:?} revoked: refused from the next request on.");
        Ok(())
    } else {
        Err(fail(format!("no active connection {agent:?}")))
    }
}

pub fn regrant(data: &Path, agent: &str, rights: &Rights) -> Result<(), Failure> {
    let ctl = open(data)?.open_control().map_err(fail)?;
    if !access::regrant(&ctl, agent, rights).map_err(fail)? {
        return Err(fail(format!("no active connection {agent:?}")));
    }
    sayln!(
        "Connection {agent:?}: rights {}. Same token, effective on the next request.",
        rights.label()
    );
    Ok(())
}

pub fn readonly(data: &Path, agent: &str, on: &str) -> Result<(), Failure> {
    let on = match on {
        "on" => true,
        "off" => false,
        _ => return Err(bad("readonly AGENT on|off")),
    };
    let ctl = open(data)?.open_control().map_err(fail)?;
    if access::set_read_only(&ctl, agent, on).map_err(fail)? {
        sayln!("{agent:?}: read-only {}.", if on { "on" } else { "off" });
        Ok(())
    } else {
        Err(fail(format!("unknown connection: {agent}")))
    }
}

pub fn person_guide(data: &Path, agent: &str, on: &str) -> Result<(), Failure> {
    let told = match on {
        "on" => true,
        "off" => false,
        _ => return Err(bad("person-guide AGENT on|off")),
    };
    let ctl = open(data)?.open_control().map_err(fail)?;
    if access::set_no_person_guide(&ctl, agent, !told).map_err(fail)? {
        sayln!(
            "{agent:?}: {} the person's own guide when it connects.",
            if told { "told" } else { "not told" }
        );
        Ok(())
    } else {
        Err(fail(format!("unknown connection: {agent}")))
    }
}

pub fn limit(data: &Path, agent: &str, key: &str, value: &str) -> Result<(), Failure> {
    let v: i64 = value.parse().map_err(|_| bad("the limit is an integer"))?;
    let ctl = open(data)?.open_control().map_err(fail)?;
    if access::set_limit(&ctl, agent, key, v).map_err(fail)? {
        sayln!("{agent:?}: {key} = {v}.");
        Ok(())
    } else {
        Err(fail(format!("unknown connection: {agent}")))
    }
}

/// What says the same thing twice, for a person to decide on. Changes nothing.
pub fn duplicates(data: &Path) -> Result<(), Failure> {
    use memcp_core::facts;
    use std::collections::BTreeMap;
    let brain = open(data)?;
    let short = |t: &str| t.chars().take(100).collect::<String>();
    let mut by_text: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for space in brain.spaces().map_err(fail)? {
        let conn = brain.open_space(&space).map_err(fail)?;
        for (id, hash, text) in facts::active_digests(&conn).map_err(fail)? {
            by_text
                .entry(hash)
                .or_default()
                .push((space.to_string(), id, text));
        }
        let pairs = facts::close_pairs(&conn, facts::SAME_FACT_THRESHOLD).map_err(fail)?;
        sayln!("{space}: {} pairs of close facts", pairs.len());
        for p in pairs {
            sayln!(
                "  {:.2}  {}  {}",
                p.similarity,
                p.first.0,
                short(&p.first.1)
            );
            sayln!("        {}  {}", p.second.0, short(&p.second.1));
        }
    }
    let same: Vec<_> = by_text.values().filter(|v| v.len() > 1).collect();
    sayln!("Same text in several spaces: {}", same.len());
    for v in same {
        let places: Vec<String> = v.iter().map(|(s, id, _)| format!("{s}:{id}")).collect();
        sayln!("  {}  {}", places.join(" "), short(&v[0].2));
    }
    Ok(())
}

pub fn import(
    data: &Path,
    vault: &Path,
    rules: &Path,
    only: Option<&str>,
    dry_run: bool,
    yes: bool,
    protect: Vec<String>,
) -> Result<(), Failure> {
    let brain = open(data)?;
    let src =
        std::fs::read_to_string(rules).map_err(|e| fail(format!("{}: {e}", rules.display())))?;
    let rules = ImportRules::parse(&src).map_err(fail)?;
    let report = import::import(
        &brain,
        vault,
        &rules,
        &ImportOptions {
            only: only.map(String::from),
            dry_run,
            yes,
            extra_protected: protect,
        },
    )
    .map_err(fail)?;
    say!("{report}");
    if dry_run {
        sayln!(
            "\nNothing was written. To import for real: run again without --dry-run (within 24 h) or with --yes."
        );
    }
    Ok(())
}

pub fn backup(data: &Path, to: &Path, keep: usize) -> Result<(), Failure> {
    let dest = open(data)?.backup(to, keep, now()).map_err(fail)?;
    sayln!("backup: {} ({keep} kept)", dest.display());
    Ok(())
}

/// `library add PATH --space NAME`: an alias of `folder set NAME PATH --read`: the folder of the space on this node,
/// only read, indexed for search. Nothing in the folder is copied, changed or moved. It replaces the folder the space
/// had (a space has one folder on a node).
pub fn library_add(data: &Path, folder: &Path, space: &str) -> Result<(), Failure> {
    folder_set(data, space, Some(folder), memcp_core::folders::Mode::Read)
}

pub fn folder_line(info: &serde_json::Value) -> String {
    let text = |k: &str| info[k].as_str().unwrap_or("-").to_string();
    let mut line = format!(
        "{}\t{}\t{}",
        text("space"),
        text("path"),
        if info["custom"] == true {
            text("mode")
        } else {
            "default (no mirror)".into()
        }
    );
    if let Some(c) = info["cloud"].as_str() {
        line.push_str(&format!("\tcloud: {c}"));
    }
    if let Some(t) = info["last_scan"].as_str() {
        line.push_str(&format!("\tscanned {t}, {} files", info["files"]));
    }
    line
}

/// `folder set SPACE PATH [--read]` | `folder set SPACE --default`: the folder of a space on THIS node (never shared
/// with the other nodes). A first pass runs at once.
pub fn folder_set(
    data: &Path,
    space: &str,
    folder: Option<&Path>,
    mode: memcp_core::folders::Mode,
) -> Result<(), Failure> {
    let brain = open(data)?;
    let space = Space::new(space).map_err(fail)?;
    let path = match folder {
        Some(p) => Some(std::path::absolute(p).map_err(fail)?),
        None => None,
    };
    let info =
        memcp_core::folders::set(&brain, &space, path.as_deref(), mode, now()).map_err(fail)?;
    sayln!("{}", folder_line(&info));
    if let Some(w) = info["warning"].as_str() {
        sayln!("warning: {w}");
    }
    if path.is_some() {
        let r = memcp_core::folders::sync_space(
            &brain,
            &space,
            now(),
            memcp_core::folders::Options::default(),
        )
        .map_err(fail)?;
        sayln!("{}", r.to_json());
    }
    Ok(())
}

/// `folder get [SPACE]`: the folder of a space on this node, or of every space that has one.
pub fn folder_get(data: &Path, space: Option<&str>) -> Result<(), Failure> {
    let brain = open(data)?;
    match space {
        Some(s) => {
            let s = Space::new(s).map_err(fail)?;
            let info = memcp_core::folders::info(&brain, &s).map_err(fail)?;
            sayln!("{}", folder_line(&info));
            if let Some(w) = info["warning"].as_str() {
                sayln!("warning: {w}");
            }
            for e in info["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str())
            {
                sayln!("error: {e}");
            }
        }
        None => {
            for f in memcp_core::folders::all(&brain).map_err(fail)? {
                sayln!(
                    "{}",
                    folder_line(&memcp_core::folders::info(&brain, &f.space).map_err(fail)?)
                );
            }
        }
    }
    Ok(())
}

/// `folder sync [SPACE]`: one pass now (the node does it by itself every few seconds while it serves).
pub fn folder_sync(data: &Path, space: Option<&str>) -> Result<(), Failure> {
    let brain = open(data)?;
    let opts = memcp_core::folders::Options::default();
    let spaces = match space {
        Some(s) => vec![Space::new(s).map_err(fail)?],
        None => memcp_core::folders::all(&brain)
            .map_err(fail)?
            .into_iter()
            .map(|f| f.space)
            .collect(),
    };
    for s in spaces {
        let r = memcp_core::folders::sync_space(&brain, &s, now(), opts).map_err(fail)?;
        sayln!("{s}: {}", r.to_json());
    }
    Ok(())
}

/// `library scan [--space NAME]`: reads the mounted folders again; only what changed is read.
pub fn library_scan(data: &Path, space: Option<&str>) -> Result<(), Failure> {
    let brain = open(data)?;
    let spaces = match space {
        Some(s) => vec![Space::new(s).map_err(fail)?],
        None => brain.spaces().map_err(fail)?,
    };
    for s in spaces {
        let conn = brain.open_space(&s).map_err(fail)?;
        if memcp_core::library::roots(&conn).map_err(fail)?.is_empty() {
            continue;
        }
        let r = memcp_core::library::rescan_all(&conn, now()).map_err(fail)?;
        sayln!(
            "{s}: {} added, {} updated, {} unchanged, {} removed",
            r.added,
            r.updated,
            r.unchanged,
            r.removed
        );
    }
    Ok(())
}

/// `library list`: the mounted folders, by space.
pub fn library_list(data: &Path) -> Result<(), Failure> {
    let brain = open(data)?;
    for s in brain.spaces().map_err(fail)? {
        let conn = brain.open_space(&s).map_err(fail)?;
        for r in memcp_core::library::roots(&conn).map_err(fail)? {
            sayln!("{s}\t{}\t{} files", r.root, r.entries);
        }
    }
    Ok(())
}

/// `vocabulary import FILE --space NAME --name NAME [--source TEXT]`: a list of terms the router reads after the
/// space's own keywords. One term per line, or `term<TAB>group<TAB>code`. The same name again replaces it.
pub fn vocabulary_import(
    data: &Path,
    file: &Path,
    space: &str,
    name: &str,
    source: &str,
) -> Result<(), Failure> {
    let brain = open(data)?;
    let space = Space::new(space).map_err(fail)?;
    if !brain.has_space(&space).map_err(fail)? {
        return Err(bad(&format!("unknown space {space}")));
    }
    let text =
        std::fs::read_to_string(file).map_err(|e| bad(&format!("{}: {e}", file.display())))?;
    let terms = memcp_core::vocabulary::parse(&text);
    let mut conn = brain.open_space(&space).map_err(fail)?;
    let kept =
        memcp_core::vocabulary::import(&mut conn, name, source, &terms, now()).map_err(fail)?;
    sayln!("vocabulary {name} on {space}: {kept} terms");
    Ok(())
}

/// `vocabulary list [--space NAME]`.
pub fn vocabulary_list(data: &Path, space: Option<&str>) -> Result<(), Failure> {
    let brain = open(data)?;
    let spaces = match space {
        Some(s) => vec![Space::new(s).map_err(fail)?],
        None => brain.spaces().map_err(fail)?,
    };
    for s in spaces {
        let conn = brain.open_space(&s).map_err(fail)?;
        for v in memcp_core::vocabulary::list(&conn).map_err(fail)? {
            sayln!("{s}\t{}\t{} terms\t{}", v.name, v.terms, v.source);
        }
    }
    Ok(())
}

/// `vocabulary remove NAME --space NAME`.
pub fn vocabulary_remove(data: &Path, space: &str, name: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let space = Space::new(space).map_err(fail)?;
    let mut conn = brain.open_space(&space).map_err(fail)?;
    if memcp_core::vocabulary::remove(&mut conn, name).map_err(fail)? {
        sayln!("removed {name} from {space}");
        Ok(())
    } else {
        Err(bad(&format!("no vocabulary {name} on {space}")))
    }
}

/// `internal AGENT on|off`: marks a connection as one of the program's own tools.
pub fn internal(data: &Path, agent: &str, on: &str) -> Result<(), Failure> {
    let on = match on {
        "on" => true,
        "off" => false,
        _ => return Err(bad("internal AGENT on|off")),
    };
    let ctl = open(data)?.open_control().map_err(fail)?;
    if access::set_internal(&ctl, agent, on).map_err(fail)? {
        sayln!(
            "{agent:?}: internal tool {}.",
            if on { "on" } else { "off" }
        );
        Ok(())
    } else {
        Err(fail(format!("unknown connection: {agent}")))
    }
}
