//! `memcp.toml`: the settings of a Linux installation, one file for the server, the janitor, the voice notes and the
//! backups. Every option of the Home Assistant app has its counterpart here (see docs/PARITY.md); the file is the
//! same for `memcp` and `memcp-manager`, and a flag on the command line always wins over the file.
//!
//! The format is the flat subset of TOML that the file needs: `key = value` lines, `#` comments, strings in double
//! quotes, whole numbers, `true`/`false`, and lists of strings in square brackets (on one line or several). Nothing
//! secret is in it: a key of a model service is a file, named here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What kind of value an option takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Path,
    Bool,
    Int { min: i64, max: i64 },
    List,
    Choice(&'static [&'static str]),
}

/// One option: its name in the file, its counterpart in the add-on, the command-line flag that carries it.
pub struct Opt {
    pub key: &'static str,
    pub kind: Kind,
    /// The default, as written in the file (a list is `a,b`, empty is nothing).
    pub default: &'static str,
    /// The option of `ha-addon/memcp/config.yaml` this one answers to, when there is one.
    pub addon: Option<&'static str>,
    /// The flag of `memcp serve` (or of the command named in `doc`) that carries it; the file fills it when the line
    /// does not give it.
    pub flag: Option<&'static str>,
    pub doc: &'static str,
}

const fn o(
    key: &'static str,
    kind: Kind,
    default: &'static str,
    addon: Option<&'static str>,
    flag: Option<&'static str>,
    doc: &'static str,
) -> Opt {
    Opt {
        key,
        kind,
        default,
        addon,
        flag,
        doc,
    }
}

/// The groups of the documented file, in order, with the options of each.
pub const SECTIONS: &[(&str, &[Opt])] = &[
    ("Where things are", &WHERE),
    ("The node", &NODE),
    ("Who may sign in", &SIGN_IN),
    ("The janitor", &JANITOR),
    ("Voice notes", &VOICE),
    ("Backups", &BACKUPS),
];

const INT_PORT: Kind = Kind::Int { min: 1, max: 1024 };

static WHERE: [Opt; 8] = [
    o(
        "data",
        Kind::Path,
        "/var/lib/memcp",
        None,
        Some("--data"),
        "The data directory: one SQLite file per space, the control database, the files. Private to its owner (0700). Back it up with `memcp backup`, never by copying it while the server runs.",
    ),
    o(
        "listen",
        Kind::Text,
        "127.0.0.1:8791",
        None,
        Some("--listen"),
        "The address the server listens on. The loopback by default: put a TLS proxy or Tailscale serve in front (docs/LINUX.md).",
    ),
    o(
        "allow_remote",
        Kind::Bool,
        "false",
        None,
        Some("--allow-remote"),
        "Allow `listen` to be an address other than the loopback. Keys travel in clear on this port: only behind a firewall that lets nothing from outside through, or with a proxy on the same machine.",
    ),
    o(
        "threads",
        INT_PORT,
        "4",
        None,
        Some("--threads"),
        "Worker threads of the server (1 to 1024).",
    ),
    o(
        "advertise",
        Kind::Bool,
        "false",
        None,
        Some("--advertise"),
        "Announce this node on the local network (mDNS) so that another machine finds it without typing an address.",
    ),
    o(
        "allow_origin",
        Kind::List,
        "",
        None,
        Some("--allow-origin"),
        "Web origins a browser page may call this server from (a list of URLs). Empty: none.",
    ),
    o(
        "console_admin",
        Kind::Bool,
        "false",
        None,
        Some("--console-admin"),
        "Serve the node page (`/admin`) on `listen` with a console token. On by itself while `listen` is the loopback; never through Tailscale Funnel.",
    ),
    o(
        "oauth_redirect",
        Kind::List,
        "",
        None,
        Some("--oauth-redirect"),
        "Extra redirect addresses accepted by the OAuth sign-in of an assistant (a list of URLs).",
    ),
];

static NODE: [Opt; 3] = [
    o(
        "node_name",
        Kind::Text,
        "MemCP",
        Some("node_name"),
        Some("--node-name"),
        "The name of this node, shown to the other nodes of your memory and on the network.",
    ),
    o(
        "language",
        Kind::Choice(&["en", "fr"]),
        "en",
        Some("language"),
        Some("--language"),
        "The search language of a new memory (`en` or `fr`). It is written once, by `memcp setup` or `init`; changing it later needs a reindex.",
    ),
    o(
        "public_url",
        Kind::Text,
        "",
        Some("public_url"),
        Some("--public-url"),
        "The address assistants reach, `https://...`, with no slash at the end. Empty: the loopback address.",
    ),
];

static SIGN_IN: [Opt; 5] = [
    o(
        "admins",
        Kind::List,
        "",
        Some("admins"),
        Some("--admins"),
        "Logins that administer the installation when the page is opened through an ingress listener. Empty: the first login. Not used for the console token or the tailnet.",
    ),
    o(
        "new_people",
        Kind::Choice(&["auto", "ask"]),
        "auto",
        Some("new_people"),
        Some("--new-people"),
        "Only for `memcp-manager` (several people on one node): `auto` gives every new login a memory of their own, `ask` makes them wait for an administrator. `memcp serve` holds one memory and ignores it.",
    ),
    o(
        "identity_listen",
        Kind::Text,
        "",
        None,
        Some("--identity-listen"),
        "The address of the identity listener: the consent page of Claude on the web and the node page for the tailnet. Put it behind `tailscale serve` (never Funnel), for example `127.0.0.1:8792`. Empty: none.",
    ),
    o(
        "identity_trust",
        Kind::Text,
        "",
        Some("identity_trust"),
        Some("--identity-trust"),
        "The one address (an IP) whose `Tailscale-User-Login` header is believed on the identity listener. Empty: the loopback only.",
    ),
    o(
        "login_url",
        Kind::Text,
        "",
        Some("login_url"),
        Some("--login-url"),
        "The public address of the identity listener, where the consent page of an assistant is shown.",
    ),
];

static SIGN_IN_USERS: [Opt; 1] = [o(
    "tailnet_users",
    Kind::List,
    "",
    Some("tailnet_users"),
    Some("--tailnet-user"),
    "Tailnet logins allowed on the node page, each as `login=person` (for example `you@example.org=owner`). Anyone else is refused.",
)];

static JANITOR: [Opt; 11] = [
    o(
        "janitor",
        Kind::Bool,
        "false",
        Some("janitor"),
        None,
        "Run the janitor (`memcp-janitor.service`): it files the notes of the inboxes with the model named below.",
    ),
    o(
        "janitor_person",
        Kind::Text,
        "owner",
        Some("janitor_person"),
        None,
        "The person whose inboxes it files.",
    ),
    o(
        "janitor_inboxes",
        Kind::List,
        "",
        Some("janitor_inboxes"),
        None,
        "The spaces whose inbox it files. Empty: every space its key reaches.",
    ),
    o(
        "janitor_work_words",
        Kind::List,
        "",
        Some("janitor_work_words"),
        None,
        "Words that mark a note as being about your work: it goes to the work space before any model.",
    ),
    o(
        "janitor_quiet",
        Kind::Int {
            min: 10,
            max: 86_400,
        },
        "120",
        Some("janitor_quiet"),
        None,
        "Seconds after the last write into an inbox before it files (10 to 86400).",
    ),
    o(
        "janitor_night",
        Kind::Text,
        "03:00",
        Some("janitor_night"),
        None,
        "The hour of the nightly pass, `HH:MM`.",
    ),
    o(
        "janitor_api_url",
        Kind::Text,
        "",
        Some("janitor_api_url"),
        None,
        "An OpenAI-compatible service the janitor may ask. Empty: none. Your notes leave this machine for it.",
    ),
    o(
        "janitor_api_model",
        Kind::Text,
        "",
        Some("janitor_api_model"),
        None,
        "The model asked at that service.",
    ),
    o(
        "janitor_api_key_file",
        Kind::Path,
        "",
        Some("janitor_api_key"),
        None,
        "A file that holds the key of that service (mode 0600). The key itself is never written here.",
    ),
    o(
        "janitor_ollama_url",
        Kind::Text,
        "",
        Some("janitor_ollama_url"),
        None,
        "An Ollama on this machine or another, for example `http://127.0.0.1:11434`. Empty: none.",
    ),
    o(
        "janitor_ollama_model",
        Kind::Text,
        "qwen3:8b",
        Some("janitor_ollama_model"),
        None,
        "The model asked of that Ollama.",
    ),
];

static VOICE: [Opt; 7] = [
    o(
        "transcribe",
        Kind::Bool,
        "false",
        Some("transcribe"),
        None,
        "Read voice notes (`memcp-transcribe.timer`): a recording waiting in an inbox becomes text through the speech service below. The recording leaves this machine for it.",
    ),
    o(
        "transcribe_person",
        Kind::Text,
        "",
        Some("transcribe_person"),
        None,
        "The person whose recordings are read. Empty: `janitor_person`.",
    ),
    o(
        "transcribe_spaces",
        Kind::Text,
        "",
        Some("transcribe_spaces"),
        None,
        "The spaces whose recordings are read, comma separated. Empty: the person's to-sort space.",
    ),
    o(
        "transcribe_api_url",
        Kind::Text,
        "",
        Some("transcribe_api_url"),
        None,
        "An OpenAI-compatible speech service (`.../v1/audio/transcriptions`).",
    ),
    o(
        "transcribe_api_model",
        Kind::Text,
        "whisper-1",
        Some("transcribe_api_model"),
        None,
        "The speech model asked at that service.",
    ),
    o(
        "transcribe_api_key_file",
        Kind::Path,
        "",
        Some("transcribe_api_key"),
        None,
        "A file that holds the key of that service (mode 0600).",
    ),
    o(
        "transcribe_language",
        Kind::Text,
        "",
        Some("transcribe_language"),
        None,
        "The language of the recordings (`en`, `fr`...). Empty: the model finds it.",
    ),
];

static BACKUPS: [Opt; 2] = [
    o(
        "backup_dir",
        Kind::Path,
        "/var/backups/memcp",
        None,
        Some("--to"),
        "Where `memcp backup` (and `memcp-backup.timer`) puts a consistent copy. Outside the data directory, ideally on another disk.",
    ),
    o(
        "backup_keep",
        Kind::Int { min: 1, max: 3650 },
        "14",
        None,
        Some("--keep"),
        "How many backups are kept; the oldest are removed.",
    ),
];

/// Every option, in the order of the documented file.
pub fn all() -> Vec<&'static Opt> {
    let mut v: Vec<&Opt> = vec![];
    for (name, opts) in SECTIONS {
        v.extend(opts.iter());
        if *name == "Who may sign in" {
            v.extend(SIGN_IN_USERS.iter());
        }
    }
    v
}

pub fn find(key: &str) -> Option<&'static Opt> {
    all().into_iter().find(|o| o.key == key)
}

/// A value read from the file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Value {
    Text(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

/// The settings of a file: only what the file says; the defaults live in the program.
#[derive(Default, Clone, Debug)]
pub struct Settings {
    values: BTreeMap<String, Value>,
}

impl Settings {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// A text (or path) value; `None` when the file does not give it or gives it empty.
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(Value::Text(s)) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        match self.values.get(key) {
            Some(Value::Bool(b)) => *b,
            _ => find(key).is_some_and(|o| o.default == "true"),
        }
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        match self.values.get(key) {
            Some(Value::Int(n)) => Some(*n),
            _ => find(key).and_then(|o| o.default.parse().ok()),
        }
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        match self.values.get(key) {
            Some(Value::List(l)) => l.clone(),
            _ => vec![],
        }
    }

    /// The text of an option, else its default.
    pub fn text_or_default(&self, key: &str) -> String {
        self.text(key)
            .map(String::from)
            .or_else(|| {
                find(key)
                    .map(|o| o.default.to_string())
                    .filter(|d| !d.is_empty())
            })
            .unwrap_or_default()
    }

    /// Sets a value after checking it as the file would.
    pub fn set(&mut self, key: &str, v: Value) -> Result<(), String> {
        let opt = find(key).ok_or_else(|| format!("unknown option {key:?}"))?;
        check(opt, &v)?;
        self.values.insert(key.to_string(), v);
        Ok(())
    }

    pub fn is_set(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn parse(text: &str) -> Result<Settings, String> {
        let mut s = Settings::default();
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let n = i + 1;
            let line = strip_comment(lines[i]).trim().to_string();
            i += 1;
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && !line.contains('=') {
                return Err(format!(
                    "line {n}: tables are not used in memcp.toml, only `key = value`"
                ));
            }
            let (key, rest) = line
                .split_once('=')
                .ok_or_else(|| format!("line {n}: expected `key = value`"))?;
            let key = key.trim();
            let mut rest = rest.trim().to_string();
            // A list may run over several lines.
            if rest.starts_with('[') {
                while !rest.contains(']') && i < lines.len() {
                    rest.push(' ');
                    rest.push_str(strip_comment(lines[i]).trim());
                    i += 1;
                }
            }
            let opt = find(key).ok_or_else(|| format!("line {n}: unknown option {key:?}"))?;
            let value = parse_value(opt, &rest).map_err(|e| format!("line {n}: {key}: {e}"))?;
            check(opt, &value).map_err(|e| format!("line {n}: {key}: {e}"))?;
            if s.values.insert(key.to_string(), value).is_some() {
                return Err(format!("line {n}: {key} is given twice"));
            }
        }
        Ok(s)
    }

    pub fn load(path: &Path) -> Result<Settings, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Settings::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The documented file with these settings in it: a set option is written, the others stay as commented defaults.
    pub fn render(&self) -> String {
        let mut out = String::from(
            "# memcp.toml: the settings of this node. Every option is listed with its default.\n\
             # A line that starts with # is a default left alone: remove the # to change it.\n\
             # A flag on the command line wins over this file. Check it with `memcp config check`.\n\
             # The same options as the Home Assistant app: docs/PARITY.md.\n",
        );
        for (name, opts) in SECTIONS {
            out.push_str(&format!(
                "\n# --- {name} {}\n",
                "-".repeat(60 - name.len().min(58))
            ));
            let extra: &[Opt] = if *name == "Who may sign in" {
                &SIGN_IN_USERS
            } else {
                &[]
            };
            for opt in opts.iter().chain(extra.iter()) {
                out.push('\n');
                for l in wrap(opt.doc, 76) {
                    out.push_str(&format!("# {l}\n"));
                }
                match self.values.get(opt.key) {
                    Some(v) => out.push_str(&format!("{} = {}\n", opt.key, show(v))),
                    None => {
                        out.push_str(&format!("# {} = {}\n", opt.key, show(&default_value(opt))))
                    }
                }
            }
        }
        out
    }
}

fn default_value(opt: &Opt) -> Value {
    match opt.kind {
        Kind::Bool => Value::Bool(opt.default == "true"),
        Kind::Int { .. } => Value::Int(opt.default.parse().unwrap_or(0)),
        Kind::List => Value::List(
            opt.default
                .split(',')
                .filter(|x| !x.is_empty())
                .map(String::from)
                .collect(),
        ),
        _ => Value::Text(opt.default.to_string()),
    }
}

fn show(v: &Value) -> String {
    match v {
        Value::Text(s) => quote(s),
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::List(l) => format!(
            "[{}]",
            l.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + w.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// The line without its comment (a `#` inside a string is kept).
fn strip_comment(line: &str) -> &str {
    let (mut in_str, mut esc) = (false, false);
    for (i, c) in line.char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_string(s: &str) -> Result<(String, &str), String> {
    let mut chars = s.char_indices();
    if chars.next().map(|(_, c)| c) != Some('"') {
        return Err("a text goes in double quotes".into());
    }
    let mut out = String::new();
    let mut esc = false;
    for (i, c) in chars {
        if esc {
            out.push(match c {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Ok((out, &s[i + 1..]));
        } else {
            out.push(c);
        }
    }
    Err("a text is not closed".into())
}

fn parse_value(opt: &Opt, raw: &str) -> Result<Value, String> {
    let raw = raw.trim();
    match opt.kind {
        Kind::Bool => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err("expected true or false".into()),
        },
        Kind::Int { .. } => raw
            .parse()
            .map(Value::Int)
            .map_err(|_| "expected a whole number".to_string()),
        Kind::List => {
            let inner = raw
                .strip_prefix('[')
                .and_then(|r| r.trim_end().strip_suffix(']'))
                .ok_or("expected a list in square brackets")?;
            let mut items = vec![];
            let mut rest = inner.trim();
            while !rest.is_empty() {
                let (s, after) = parse_string(rest)?;
                items.push(s);
                rest = after.trim_start();
                rest = match rest.strip_prefix(',') {
                    Some(r) => r.trim_start(),
                    None if rest.is_empty() => rest,
                    None => return Err("expected a comma between the items".into()),
                };
            }
            Ok(Value::List(items))
        }
        _ => {
            let (s, after) = parse_string(raw)?;
            if !after.trim().is_empty() {
                return Err("nothing may follow the closing quote".into());
            }
            Ok(Value::Text(s))
        }
    }
}

fn check(opt: &Opt, v: &Value) -> Result<(), String> {
    match (opt.kind, v) {
        (Kind::Text | Kind::Path, Value::Text(s)) => {
            if s.chars().any(char::is_control) {
                return Err("no control characters".into());
            }
            if opt.key == "janitor_night" && !valid_hhmm(s) {
                return Err("expected HH:MM".into());
            }
            if opt.key == "public_url" && s.ends_with('/') {
                return Err("no slash at the end".into());
            }
            Ok(())
        }
        (Kind::Choice(allowed), Value::Text(s)) => {
            if allowed.contains(&s.as_str()) {
                Ok(())
            } else {
                Err(format!("one of {}", allowed.join(", ")))
            }
        }
        (Kind::Bool, Value::Bool(_)) => Ok(()),
        (Kind::Int { min, max }, Value::Int(n)) => {
            if (min..=max).contains(n) {
                Ok(())
            } else {
                Err(format!("from {min} to {max}"))
            }
        }
        (Kind::List, Value::List(items)) => {
            for item in items {
                if item.contains(',') || item.chars().any(char::is_control) {
                    return Err(format!("{item:?}: no comma and no control character"));
                }
                if opt.key == "tailnet_users" && !valid_tailnet_pair(item) {
                    return Err(format!("{item:?}: expected login=person"));
                }
            }
            Ok(())
        }
        _ => Err("the wrong kind of value".into()),
    }
}

fn valid_hhmm(s: &str) -> bool {
    let Some((h, m)) = s.split_once(':') else {
        return false;
    };
    h.len() == 2
        && m.len() == 2
        && h.parse::<u32>().is_ok_and(|h| h < 24)
        && m.parse::<u32>().is_ok_and(|m| m < 60)
}

fn valid_tailnet_pair(s: &str) -> bool {
    s.split_once('=').is_some_and(|(login, person)| {
        !login.trim().is_empty()
            && !login.chars().any(char::is_whitespace)
            && !person.trim().is_empty()
    })
}

/// Where the settings file is: the one named, else `MEMCP_CONFIG`, else the user's, else the system's. `Ok(None)`: none
/// of the default places holds one. A file named and missing is an error.
pub fn locate(explicit: Option<&str>) -> Result<Option<PathBuf>, String> {
    if let Some(p) = explicit {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(Some(p))
        } else {
            Err(format!("{}: no such settings file", p.display()))
        };
    }
    if let Some(p) = std::env::var_os("MEMCP_CONFIG").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(Some(p))
        } else {
            Err(format!("MEMCP_CONFIG: {}: no such file", p.display()))
        };
    }
    Ok(default_places().into_iter().find(|p| p.is_file()))
}

/// The places looked at without being told, the user's first.
pub fn default_places() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(h) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        v.push(h.join("memcp").join("memcp.toml"));
    }
    v.push(PathBuf::from("/etc/memcp/memcp.toml"));
    v
}

/// The command line the file stands for: for every option that has a flag and that the file sets (and the line does not),
/// the flag or the switch. A list becomes its items joined by commas.
pub fn fill_flags(
    settings: &Settings,
    flags: &mut std::collections::HashMap<String, String>,
    switches: &mut Vec<String>,
) {
    for opt in all() {
        let Some(flag) = opt.flag else { continue };
        let Some(v) = settings.get(opt.key) else {
            continue;
        };
        match v {
            Value::Bool(true) => {
                if !switches.iter().any(|s| s == flag) {
                    switches.push(flag.to_string());
                }
            }
            Value::Bool(false) => {}
            Value::Int(n) => {
                flags
                    .entry(flag.to_string())
                    .or_insert_with(|| n.to_string());
            }
            Value::Text(s) if !s.is_empty() => {
                flags.entry(flag.to_string()).or_insert_with(|| s.clone());
            }
            Value::List(l) if !l.is_empty() => {
                flags.entry(flag.to_string()).or_insert_with(|| l.join(","));
            }
            _ => {}
        }
    }
}

/// The lines a problem in the file is told in, for `config check`.
pub fn describe(settings: &Settings) -> Vec<String> {
    let mut out = vec![];
    if settings.flag("janitor")
        && settings.text("janitor_api_url").is_none()
        && settings.text("janitor_ollama_url").is_none()
    {
        out.push(
            "janitor is on but no model is named (janitor_api_url or janitor_ollama_url): it will not start".into(),
        );
    }
    if settings.flag("transcribe") && settings.text("transcribe_api_url").is_none() {
        out.push(
            "transcribe is on but no speech service is named (transcribe_api_url): it will not start"
                .into(),
        );
    }
    if settings
        .text("public_url")
        .is_some_and(|u| !u.starts_with("https://"))
    {
        out.push("public_url is not HTTPS: Claude on the web needs HTTPS".into());
    }
    if settings.text("identity_trust").is_some() && settings.text("identity_listen").is_none() {
        out.push("identity_trust is set but identity_listen is not: it has no effect".into());
    }
    if !settings.list("tailnet_users").is_empty() && settings.text("identity_listen").is_none() {
        out.push(
            "tailnet_users is set but identity_listen is not: nobody can sign in that way".into(),
        );
    }
    out
}
