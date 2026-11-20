//! Every command of the command line, with how it runs on a node reached by `--node` (see docs/CLI.md).
//!
//! This table is the one place that says it. The client reads it to know what to send, the node reads it to know who may
//! run what, `docs/CLI.md` is generated from it, and a test fails when a command of the usage text is missing from it or
//! has no remote form and no reason for not having one.

/// What a command sends to the node, from the files it names on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Up {
    /// The positional word at this index is a file or a folder of this machine.
    Pos(usize),
    /// Every positional word from this index on is.
    PosFrom(usize),
    /// The value of this option is.
    Flag(&'static str),
}

/// How a command runs through `--node`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    /// The node runs the very same function (`POST /admin/api/cli`) and answers what it printed.
    Exec,
    /// Like `Exec`, after the files the command names have been sent to the node (a chunk at a time).
    Upload(&'static [Up]),
    /// A memory downloaded as a stream (`GET /admin/api/export`) and written here.
    Download,
    /// Calls routes of the page directly: the command only exists against a running node.
    Direct,
    /// Only on the machine itself, for the reason given.
    Local(&'static str),
}

/// One command.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// The words that name it: `space new`.
    pub words: &'static str,
    /// What it does, in a few words.
    pub summary: &'static str,
    pub remote: Remote,
    /// Only an administrator's key may run it through a node (it acts on every person, or on the node's machine).
    pub admin: bool,
}

const fn e(words: &'static str, summary: &'static str, remote: Remote, admin: bool) -> Entry {
    Entry {
        words,
        summary,
        remote,
        admin,
    }
}

pub const COMMANDS: &[Entry] = &[
    e(
        "init",
        "creates the data directory, the control database and the spaces",
        Remote::Local("creates the data directory of this machine"),
        false,
    ),
    e(
        "setup",
        "first run: data directory, first memory, memcp.toml",
        Remote::Local("prepares this machine"),
        false,
    ),
    e(
        "config",
        "checks, shows or prints the settings file",
        Remote::Local("reads the settings file of this machine"),
        false,
    ),
    e(
        "prepare",
        "what the janitor's service unit runs first",
        Remote::Local("writes files on this machine"),
        false,
    ),
    e(
        "serve",
        "runs the node",
        Remote::Local("it is the node"),
        false,
    ),
    e(
        "mcp",
        "MCP on standard input and output",
        Remote::Local(
            "standard input and output of a program on this machine; a remote assistant uses the node's MCP address",
        ),
        false,
    ),
    e(
        "skills-export",
        "writes the professions as Claude skills (a plugin marketplace)",
        Remote::Local("writes a folder of this machine from the built-in professions"),
        false,
    ),
    e(
        "purge",
        "removes notes for good, by id, on this machine only",
        Remote::Local(
            "deletes rows of the database of this machine, and must never be asked of a node by a key",
        ),
        false,
    ),
    e(
        "repair provenance",
        "puts back the date, author, source and origin a retag lost",
        Remote::Local("edits rows of the database of this machine, after a count"),
        false,
    ),
    e(
        "professions",
        "lists the professions the person added and the proposals",
        Remote::Local("reads the professions file in the data folder of this machine"),
        false,
    ),
    e(
        "professions accept",
        "accepts an assistant's proposed profession",
        Remote::Local("moves a proposal inside the data folder of this machine"),
        false,
    ),
    e(
        "professions reject",
        "drops an assistant's proposed profession",
        Remote::Local("edits the proposals file in the data folder of this machine"),
        false,
    ),
    e(
        "verify-export",
        "checks an export folder",
        Remote::Local("checks a folder of this machine and needs no node"),
        false,
    ),
    e(
        "transcribe",
        "one pass over the recordings waiting",
        Remote::Local("the node does it by itself while it serves, from its own settings"),
        false,
    ),
    e(
        "watch",
        "looks at the folders of `watch`",
        Remote::Local("watches folders of this machine"),
        false,
    ),
    e(
        "intake send",
        "puts files into a space's inbox",
        Remote::Upload(&[Up::PosFrom(2)]),
        false,
    ),
    e(
        "intake text",
        "puts a note into the inbox",
        Remote::Exec,
        false,
    ),
    e(
        "intake absorb",
        "one pass over the folders of `absorb`",
        Remote::Local("reads folders named in the settings of the node's machine"),
        false,
    ),
    e(
        "intake status",
        "recordings waiting and folders watched",
        Remote::Local("reads the settings file of the node's machine"),
        false,
    ),
    e(
        "intake discover",
        "nodes found on this network",
        Remote::Local("looks at the network of this machine"),
        false,
    ),
    e("catalogue", "lists the kinds of space", Remote::Exec, false),
    e(
        "space make",
        "creates a person's space of a kind",
        Remote::Exec,
        false,
    ),
    e(
        "vault add",
        "former name of `space make`",
        Remote::Exec,
        false,
    ),
    e("space add", "a space named by hand", Remote::Exec, true),
    e(
        "space list",
        "lists the spaces of the node",
        Remote::Exec,
        true,
    ),
    e("space kind", "gives a space a kind", Remote::Exec, true),
    e(
        "space new",
        "creates a space with a label",
        Remote::Exec,
        false,
    ),
    e(
        "space show",
        "one space: label, kind, counts, rules",
        Remote::Exec,
        false,
    ),
    e("space tree", "the spaces as a tree", Remote::Exec, false),
    e(
        "space set",
        "label, parent, position, kind",
        Remote::Exec,
        false,
    ),
    e(
        "space delete",
        "moves a space's notes into another, or archives it",
        Remote::Exec,
        false,
    ),
    e(
        "space archived",
        "lists the archived spaces",
        Remote::Exec,
        false,
    ),
    e(
        "space restore",
        "brings an archived space back",
        Remote::Exec,
        false,
    ),
    e("folder get", "the folder of a space", Remote::Direct, false),
    e(
        "folder sync",
        "one pass of the folder of a space now",
        Remote::Direct,
        false,
    ),
    e(
        "folder set",
        "sets or clears the folder of a space",
        Remote::Direct,
        false,
    ),
    e("rules", "keywords and tags of a space", Remote::Exec, false),
    e(
        "vocabulary import",
        "imports a list of terms into a space",
        Remote::Upload(&[Up::Pos(2)]),
        true,
    ),
    e(
        "vocabulary list",
        "lists the vocabularies",
        Remote::Exec,
        true,
    ),
    e(
        "vocabulary remove",
        "removes a vocabulary",
        Remote::Exec,
        true,
    ),
    e(
        "vocabulary term-add",
        "adds terms to a vocabulary",
        Remote::Exec,
        true,
    ),
    e(
        "vocabulary term-remove",
        "removes terms from a vocabulary",
        Remote::Exec,
        true,
    ),
    e(
        "library add",
        "mounts a folder of the node's machine on a space",
        Remote::Exec,
        true,
    ),
    e(
        "library scan",
        "reads the mounted folders again",
        Remote::Exec,
        true,
    ),
    e(
        "library list",
        "lists the mounted folders",
        Remote::Exec,
        true,
    ),
    e("library remove", "unmounts a folder", Remote::Exec, true),
    e(
        "node",
        "the other nodes: list, rename, note, pause, resume, revoke, forget, and the sync policy per space",
        Remote::Exec,
        false,
    ),
    e(
        "connection",
        "show, rename, rotate, delete a connection; also list, new, regrant, revoke, internal",
        Remote::Exec,
        false,
    ),
    e("connections", "lists the connections", Remote::Exec, true),
    e(
        "connect",
        "creates a connection and shows its key once",
        Remote::Exec,
        false,
    ),
    e(
        "open",
        "opens one space to an assistant",
        Remote::Exec,
        false,
    ),
    e(
        "regrant",
        "changes the rights of a connection",
        Remote::Exec,
        true,
    ),
    e("revoke", "cuts a connection", Remote::Exec, true),
    e(
        "internal",
        "marks a connection as the program's own",
        Remote::Exec,
        true,
    ),
    e(
        "readonly",
        "forces a connection read-only",
        Remote::Exec,
        true,
    ),
    e(
        "person-guide",
        "tells or not the person's guide to a connection",
        Remote::Exec,
        true,
    ),
    e("limit", "sets a limit of a connection", Remote::Exec, true),
    e(
        "person",
        "list, show, new, set, heir, recovery-key, disable, enable, delete",
        Remote::Exec,
        true,
    ),
    e(
        "note",
        "add, show, list, recall, tags, edit, retire, restore",
        Remote::Exec,
        false,
    ),
    e(
        "console-token",
        "a token that opens the page",
        Remote::Exec,
        true,
    ),
    e(
        "invite code",
        "a code (and link) a device exchanges for a key to the inbox",
        Remote::Direct,
        false,
    ),
    e(
        "invite ticket",
        "a link with a one-time ticket another node joins with",
        Remote::Direct,
        false,
    ),
    e(
        "invite pending",
        "devices asking to join",
        Remote::Direct,
        false,
    ),
    e("invite approve", "lets a device in", Remote::Direct, false),
    e("invite deny", "turns a device away", Remote::Direct, false),
    e(
        "invite allow",
        "lets a device come back without the words, with a one-time ticket",
        Remote::Direct,
        false,
    ),
    e(
        "join",
        "asks another node (link, id or address) to let this node in",
        Remote::Direct,
        true,
    ),
    e(
        "export",
        "writes a person's memory to a folder",
        Remote::Download,
        false,
    ),
    e(
        "adopt",
        "takes the spaces of an export into the node",
        Remote::Upload(&[Up::Pos(1)]),
        true,
    ),
    e(
        "merge",
        "plans or applies the merge of another memory",
        Remote::Upload(&[Up::Flag("--plan"), Up::Flag("--apply")]),
        true,
    ),
    e(
        "import",
        "indexes a Markdown folder",
        Remote::Upload(&[
            Up::Flag("--folder"),
            Up::Flag("--vault"),
            Up::Flag("--rules"),
        ]),
        true,
    ),
    e(
        "duplicates",
        "what says the same thing twice",
        Remote::Exec,
        true,
    ),
    e(
        "backup",
        "a consistent copy of every database",
        Remote::Exec,
        true,
    ),
    e(
        "doctor",
        "reads the installation and says what deserves a look",
        Remote::Exec,
        true,
    ),
    e("status", "what this memory holds", Remote::Exec, false),
];

/// The entry of a command line: the longest `words` that its positional words begin with.
pub fn find(pos: &[String]) -> Option<&'static Entry> {
    COMMANDS
        .iter()
        .filter(|c| {
            let w: Vec<&str> = c.words.split(' ').collect();
            pos.len() >= w.len() && w.iter().zip(pos).all(|(a, b)| a == b)
        })
        .max_by_key(|c| c.words.split(' ').count())
}

/// The commands of the usage text: the first word (and the second one, where two commands share the first), from the
/// lines that start a command.
pub fn usage_commands(usage: &str) -> Vec<String> {
    let mut out = vec![];
    for line in usage.lines() {
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if rest.starts_with(' ') || rest.starts_with('[') || rest.starts_with('-') {
            continue;
        }
        let first: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '-')
            .collect();
        if first.is_empty() || first == "every" {
            continue;
        }
        out.push(first);
    }
    out.sort();
    out.dedup();
    out
}

/// The table of docs/CLI.md, as Markdown: one row per command with its local and its remote form.
pub fn markdown() -> String {
    let mut s = String::from(
        "| Command | What it does | Local (`--data`) | Remote (`--node`) | Key |\n|---|---|---|---|---|\n",
    );
    for c in COMMANDS {
        let (local, remote) = match c.remote {
            Remote::Exec => ("yes".to_string(), "yes, runs on the node".to_string()),
            Remote::Upload(_) => ("yes".into(), "yes, files are sent first".into()),
            Remote::Download => ("yes".into(), "yes, streamed down".into()),
            Remote::Direct => (
                "needs a node".into(),
                "yes, through the page's routes".into(),
            ),
            Remote::Local(why) => ("yes".into(), format!("no: {why}")),
        };
        let key = match c.remote {
            Remote::Local(_) => "-",
            _ if c.admin => "administrator",
            _ => "any person",
        };
        s.push_str(&format!(
            "| `{}` | {} | {local} | {remote} | {key} |\n",
            c.words, c.summary
        ));
    }
    s
}
