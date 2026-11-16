//! `memcp-bridge`: the one program on the side of the person who captures, on Linux and on macOS.
//!
//! - `memcp-bridge [gateway]`: the memory's tools on standard input and output, for an assistant that runs a
//!   command. Writes without a decision are kept while the network is down.
//! - `memcp-bridge send FILE...`, `text TEXT`: put captures into « À trier », the person's one box for everything
//!   that comes in (kept first, sent at once if the memory can be reached). The bridge never chooses a vault.
//! - `memcp-bridge flush`, `status`: send what waits; list what waits and what went.
//!
//! - `memcp-bridge --config FILE --socket PATH`: the gateway on a Unix socket, run as a user of its own; `memcp-bridge connect
//!   PATH` is what an assistant starts: it carries messages to the socket and knows neither the address nor the key.
//!
//! The settings come from `--config FILE` (mode 600: address, key), else from the options: the key from the file named by
//! `--key-file`, else from the variable named by `--token-env` (`MEMCP_TOKEN`), else, on macOS, from the Keychain.

use std::io::{self, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use memcp_bridge::{Config, Flush, Gateway, Intake, State, serve_http};

const USAGE: &str = "usage: memcp-bridge [gateway|send|text|flush|status|retry|forget|absorb|transcribe] (--config FILE | --url https://<server>/mcp/<connection>) [options]
       memcp-bridge connect SOCKET

  gateway (default)      the memory's tools on standard input and output; writes without a decision
                         (remember with the tag inbox, deposit, file_put) are kept while the network is down.
                         It has tools of its own: capture_text, capture_file, outbox_status, outbox_flush.
                         --listen ADDRESS serves them over HTTP instead (POST /mcp), on this machine only
  send FILE...           put files (or recordings) into À trier, the one box the key reaches
  text TEXT | text -     put a note into À trier (- reads standard input)
  flush                  send what waits
  status                 what waits and what was sent, with the id of each
  retry [ID]             put a refused item (or all of them) back in the queue, and send
  forget ID              drop an item that waits, or take one off the list
  absorb [all]           with --config: send what is new in the folders of `absorb = FOLDER -> VAULT` to their inbox
                         (the janitor draws facts from it), and put down the proposals of `deliver = VAULT TAG -> FOLDER`
                         as files. The first pass only takes stock, unless `all`. Run it on a timer
  transcribe             with --config: read the recordings that reached the inboxes of `transcribe = SPACE, ...` (the
                         person's À trier, `<person>-a-trier`, where the devices put them) with
                         no text (whisper_model, whisper_cmd, ffmpeg_cmd, lexicon), put their text in the same inbox, and
                         erase each recording a week after its text was filed. Run it on a timer
  discover               nodes found on this network (mDNS) and on the tailnet: name, address, version, how
  join ADDRESS           ask a node to let this computer join: a person on the node compares a phrase and allows it;
                         with --config FILE (not overwritten), --name NAME
  connect SOCKET|ADDRESS carry standard input and output to a gateway's Unix socket, or to its HTTP address on this machine
                         (127.0.0.1:PORT, from --listen; the way on Windows): for an assistant that must know neither the
                         memory's address nor its key (it needs no other option)

options:
  --config FILE          the settings, one `key = value` a line (mode 600, refused otherwise): url, key (or key_file;
                         on macOS, without either, the Keychain),
                         outbox, capture_dir (repeat). With it, no address or key is taken from the options or
                         the environment, and none appears in a process list
  --socket PATH          gateway on a Unix socket at PATH (mode 600; in a folder only its owner writes)
  --socket-group GID     the socket is 660 with this group: the gateway's user and the person, nobody else
  --url ADDRESS          the address of the connection (or MEMCP_URL)
  --note TEXT            a line saying what a file is (send)
  --listen ADDRESS       gateway over HTTP on this machine, e.g. 127.0.0.1:8792 (a loopback address only)
  --capture-dir DIR      a folder capture_file may read from (repeat it; or MEMCP_CAPTURE_DIRS, separated by :).
                         With none, capture_file is off. Hidden folders are never read.
  --key-file PATH        the key is read from this file (MEMCP_KEY_FILE). Refused if group or others can read it (chmod 600).
                         The assistant's own configuration then holds the path only, never the key or the memory's address
  --token-env NAME       the variable holding the key (MEMCP_TOKEN by default); on macOS, else the Keychain
                         (service memcp-bridge, account the address)
  --outbox DIR           where what waits is kept (MEMCP_OUTBOX; by default a folder of this connection in the app's
                         data folder: one connection never lists or sends another's captures)
  --utc-offset MINUTES   the user's offset from UTC, for dates (by default read from the system)

exit: 0 done, 1 the key is refused, 2 wrong use, 3 something was refused by the memory";

fn keychain(url: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // The relay of an earlier version stored its keys under its own name: still found.
    ["memcp-bridge", "memcp-relay"].iter().find_map(|service| {
        let out = Command::new("security")
            .args(["find-generic-password", "-s", service, "-a", url, "-w"])
            .output()
            .ok()?;
        let key = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !key.is_empty()).then_some(key)
    })
}

/// The key from a file only its owner can read: a key in a file that others can read is no longer the bridge's alone.
fn read_key_file(file: &std::path::Path) -> Result<String, String> {
    private_file(file)?;
    let key = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if key.trim().is_empty() {
        return Err(format!("{} is empty", file.display()));
    }
    Ok(key)
}

/// The settings file: `key = value` lines, `#` comments. Only its owner may read it.
struct FileConfig {
    url: String,
    key: String,
    outbox: Option<PathBuf>,
    capture_dirs: Vec<PathBuf>,
    absorb: Vec<memcp_bridge::AbsorbSource>,
    deliver: Vec<memcp_bridge::Delivery>,
    /// The vaults whose recordings are transcribed here, and how.
    transcribe: Vec<String>,
    voice: memcp_bridge::VoiceSettings,
}

fn home_path(v: &str) -> PathBuf {
    match v.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(v),
    }
}

fn list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn private_file(file: &std::path::Path) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = file;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(file)
            .map_err(|e| format!("{}: {e}", file.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "{} can be read by others (mode {:o}): chmod 600 it",
                file.display(),
                mode & 0o777
            ));
        }
    }
    Ok(())
}

fn read_config(file: &std::path::Path) -> Result<FileConfig, String> {
    private_file(file)?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (mut url, mut key, mut outbox, mut dirs) = (None, None, None, vec![]);
    let (mut absorb, mut deliver) = (vec![], vec![]);
    let (mut exclude, mut protect, mut roles) = (vec![], vec![], vec![]);
    let (mut transcribe, mut voice) = (vec![], memcp_bridge::VoiceSettings::default());
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or(format!(
            "{}: line {}: `key = value` expected",
            file.display(),
            n + 1
        ))?;
        let v = v.trim().to_string();
        match k.trim() {
            "url" => url = Some(v),
            "key" => key = Some(v),
            "key_file" => key = Some(read_key_file(&home_path(&v))?),
            // No longer used: everything goes to À trier. Accepted, so that an old file still works.
            "vault" => {}
            "outbox" => outbox = Some(home_path(&v)),
            "capture_dir" => dirs.push(home_path(&v)),
            // `absorb = ~/Notes/Work -> work`: a folder whose new text goes to the inbox of a vault.
            "absorb" => {
                let (d, vlt) = v
                    .split_once("->")
                    .ok_or(format!("{}: absorb = FOLDER -> VAULT", file.display()))?;
                absorb.push(memcp_bridge::AbsorbSource {
                    dir: home_path(d.trim()),
                    vault: vlt.trim().to_string(),
                    exclude: vec![],
                    protect: vec![],
                    roles: vec![],
                });
            }
            "transcribe" => transcribe.extend(list(&v)),
            "whisper_model" => voice.whisper_model = home_path(&v),
            "whisper_cmd" => voice.whisper_cmd = v,
            "whisper_lang" => voice.whisper_lang = v,
            "whisper_threads" => {
                voice.whisper_threads = v
                    .parse()
                    .map_err(|_| format!("{}: whisper_threads: a number", file.display()))?;
            }
            "ffmpeg_cmd" => voice.ffmpeg_cmd = v,
            "whisper_api_url" => voice.api_url = Some(v),
            "whisper_api_key" => voice.api_key = Some(v),
            "whisper_api_model" => voice.api_model = v,
            "lexicon" => voice.lexicon = Some(home_path(&v)),
            "absorb_exclude" => exclude.extend(list(&v)),
            "absorb_protect" => protect.extend(list(&v)),
            "absorb_roles" => {
                let path = home_path(&v);
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("absorb_roles {}: {e}", path.display()))?;
                roles = memcp_bridge::read_roles(&text);
            }
            // `deliver = work proposals/pro -> ~/Notes/Work/Ideas`: proposals of a vault put down as files.
            "deliver" => {
                let (what, d) = v
                    .split_once("->")
                    .ok_or(format!("{}: deliver = VAULT TAG -> FOLDER", file.display()))?;
                let (vlt, tag) = what
                    .trim()
                    .split_once(' ')
                    .ok_or(format!("{}: deliver = VAULT TAG -> FOLDER", file.display()))?;
                deliver.push(memcp_bridge::Delivery {
                    vault: vlt.trim().to_string(),
                    tag: tag.trim().to_string(),
                    dir: home_path(d.trim()),
                });
            }
            other => return Err(format!("{}: unknown setting {other}", file.display())),
        }
    }
    for a in &mut absorb {
        a.exclude.clone_from(&exclude);
        a.protect.clone_from(&protect);
        a.roles.clone_from(&roles);
        // What the bridge puts down in a folder it also absorbs is not sent back.
        for d in &deliver {
            if let Ok(rel) = d.dir.strip_prefix(&a.dir) {
                a.exclude.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    // A speech model on this machine, or a speech service by API: one of the two.
    if !transcribe.is_empty()
        && voice.whisper_model.as_os_str().is_empty()
        && voice.api_url.is_none()
    {
        return Err(format!(
            "{}: transcribe needs whisper_model, or whisper_api_url",
            file.display()
        ));
    }
    let url = url.ok_or(format!("{}: url is needed", file.display()))?;
    // Without key or key_file, on macOS, the Keychain (service memcp-bridge, account the address), as with the options.
    let key = key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| keychain(&url))
        .ok_or(format!(
            "{}: key or key_file is needed (or, on macOS, the key in the Keychain)",
            file.display()
        ))?;
    Ok(FileConfig {
        absorb,
        deliver,
        transcribe,
        voice,
        url,
        key,
        outbox,
        capture_dirs: dirs,
    })
}

fn default_outbox() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/memcp-bridge")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("memcp-bridge")
    }
}

/// Minutes east of UTC, from the system's `date +%z` (`+0100`).
fn system_offset() -> i32 {
    let out = Command::new("date").arg("+%z").output().ok();
    let z = out
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let z = z.trim();
    if z.len() != 5 {
        return 0;
    }
    let sign = if z.starts_with('-') { -1 } else { 1 };
    let (h, m) = (
        z[1..3].parse::<i32>().unwrap_or(0),
        z[3..5].parse::<i32>().unwrap_or(0),
    );
    sign * (h * 60 + m)
}

struct Args {
    mode: String,
    rest: Vec<String>,
    url: Option<String>,
    note: String,
    token_env: String,
    key_file: Option<PathBuf>,
    outbox: Option<PathBuf>,
    offset: Option<i32>,
    listen: Option<String>,
    capture_dirs: Vec<PathBuf>,
    config: Option<PathBuf>,
    socket: Option<PathBuf>,
    socket_group: Option<u32>,
    /// What this device calls itself when it asks to join (`join`).
    name: Option<String>,
    /// How long `discover` looks.
    seconds: u64,
    /// What came from the options or the environment and must not be mixed with `--config`.
    given: Vec<&'static str>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        mode: String::new(),
        rest: vec![],
        url: std::env::var("MEMCP_URL").ok(),
        note: String::new(),
        token_env: "MEMCP_TOKEN".into(),
        key_file: std::env::var_os("MEMCP_KEY_FILE").map(PathBuf::from),
        outbox: std::env::var_os("MEMCP_OUTBOX").map(PathBuf::from),
        offset: None,
        listen: None,
        capture_dirs: std::env::var("MEMCP_CAPTURE_DIRS")
            .map(|v| {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default(),
        config: None,
        socket: None,
        socket_group: None,
        name: None,
        seconds: 4,
        given: vec![],
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--url" => {
                a.url = Some(value("--url")?);
                a.given.push("--url");
            }
            "--vault" => {
                return Err(
                    "--vault is gone: a capture goes to À trier, the janitor or the person sorts it".into(),
                );
            }
            "--config" => a.config = Some(PathBuf::from(value("--config")?)),
            "--socket" => a.socket = Some(PathBuf::from(value("--socket")?)),
            "--socket-group" => {
                a.socket_group = Some(
                    value("--socket-group")?
                        .parse()
                        .map_err(|_| "--socket-group: a numeric group id")?,
                );
            }
            "--note" => a.note = value("--note")?,
            "--name" => a.name = Some(value("--name")?),
            "--seconds" => {
                a.seconds = value("--seconds")?
                    .parse()
                    .map_err(|_| "--seconds: a number")?;
            }
            "--token-env" => {
                a.token_env = value("--token-env")?;
                a.given.push("--token-env");
            }
            "--key-file" => {
                a.key_file = Some(PathBuf::from(value("--key-file")?));
                a.given.push("--key-file");
            }
            "--outbox" => a.outbox = Some(PathBuf::from(value("--outbox")?)),
            "--listen" => a.listen = Some(value("--listen")?),
            "--capture-dir" => a.capture_dirs.push(PathBuf::from(value("--capture-dir")?)),
            "--utc-offset" => {
                a.offset = Some(
                    value("--utc-offset")?
                        .parse()
                        .map_err(|_| "--utc-offset: minutes, e.g. 60")?,
                );
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s if a.mode.is_empty()
                && [
                    "gateway",
                    "send",
                    "text",
                    "flush",
                    "status",
                    "retry",
                    "forget",
                    "connect",
                    "absorb",
                    "transcribe",
                    "discover",
                    "join",
                ]
                .contains(&s) =>
            {
                a.mode = s.to_string();
            }
            // A word that is no command, before any command: a typo, never a silent start of the gateway.
            s if a.mode.is_empty() => return Err(format!("unknown command {s}")),
            s => a.rest.push(s.to_string()),
        }
    }
    if a.mode.is_empty() {
        a.mode = "gateway".into();
    }
    Ok(a)
}

fn summary(r: &Flush) -> String {
    format!(
        "sent {}, waiting {}, refused by the memory {}{}{}",
        r.sent,
        r.waiting,
        r.failed,
        if r.unreachable {
            ", the memory cannot be reached (kept, it will go later)"
        } else {
            ""
        },
        if r.refused {
            ", THE KEY IS REFUSED OR REVOKED"
        } else {
            ""
        }
    )
}

fn code(r: &Flush) -> ExitCode {
    if r.refused {
        ExitCode::from(1)
    } else if r.failed > 0 {
        ExitCode::from(3)
    } else {
        ExitCode::SUCCESS
    }
}

fn run(raw: &[String]) -> Result<ExitCode, String> {
    if raw.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut a = parse(raw)?;
    if a.mode == "connect" {
        let [path] = a.rest.as_slice() else {
            return Err("connect needs the path of the bridge's socket".into());
        };
        // An address (127.0.0.1:PORT) is an HTTP gateway, on every system; a path is a Unix socket.
        if memcp_bridge::relay::is_address(path) {
            memcp_bridge::relay::relay(path, io::stdin().lock(), io::stdout().lock())
                .map_err(|e| e.to_string())?;
            return Ok(ExitCode::SUCCESS);
        }
        #[cfg(unix)]
        {
            memcp_bridge::connect_socket(std::path::Path::new(path)).map_err(|e| e.to_string())?;
            return Ok(ExitCode::SUCCESS);
        }
        #[cfg(not(unix))]
        {
            return Err(
                "connect needs an address (127.0.0.1:PORT) on this system: it has no Unix sockets"
                    .into(),
            );
        }
    }
    if a.mode == "discover" {
        let nodes = memcp_bridge::discover::discover(std::time::Duration::from_secs(
            a.seconds.clamp(1, 30),
        ));
        if nodes.is_empty() {
            eprintln!("memcp-bridge: no node found on this network or on the tailnet");
            return Ok(ExitCode::from(1));
        }
        for n in nodes {
            println!("{}\t{}\t{}\t{}", n.name, n.address, n.version, n.via);
        }
        return Ok(ExitCode::SUCCESS);
    }
    if a.mode == "join" {
        let [address] = a.rest.as_slice() else {
            return Err("join needs the address of a node (see `discover`): join http://HOST:8791 --config FILE".into());
        };
        let Some(file) = a.config.clone() else {
            return Err(
                "join needs --config FILE: where the connection is written (mode 600)".into(),
            );
        };
        if file.exists() {
            return Err(format!(
                "{} exists already: it is not overwritten",
                file.display()
            ));
        }
        let device = a.name.clone().unwrap_or_else(|| {
            std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "a computer".into())
        });
        eprintln!("memcp-bridge: asking {address} to let `{device}` join…");
        let joined = memcp_bridge::discover::join(address, &device, |phrase| {
            eprintln!(
                "memcp-bridge: on the node, check that the phrase is\n\n    {phrase}\n\nand allow it. Waiting for the answer (two minutes)…"
            );
        })?;
        let text = format!("url = {}\nkey = {}\n", joined.url, joined.key);
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new();
            f.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                f.mode(0o600);
            }
            f.open(&file)
                .and_then(|mut h| h.write_all(text.as_bytes()))
                .map_err(|e| format!("{}: {e}", file.display()))?;
        }
        eprintln!(
            "memcp-bridge: joined as {}; written to {}",
            joined.connection,
            file.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    let mut voice: (Vec<String>, memcp_bridge::VoiceSettings) =
        (vec![], memcp_bridge::VoiceSettings::default());
    let mut absorbing: (Vec<memcp_bridge::AbsorbSource>, Vec<memcp_bridge::Delivery>) =
        (vec![], vec![]);
    let (url, key) = if let Some(file) = &a.config {
        if let Some(o) = a.given.first() {
            return Err(format!(
                "{o} is not taken with --config: the settings file says it"
            ));
        }
        let c = read_config(file)?;
        if a.outbox.is_none() {
            a.outbox = c.outbox;
        }
        a.capture_dirs.extend(c.capture_dirs);
        absorbing = (c.absorb, c.deliver);
        voice = (c.transcribe, c.voice);
        (c.url, c.key)
    } else {
        let url = a.url.clone().ok_or("missing --url (or --config)")?;
        let key = match &a.key_file {
            Some(file) => read_key_file(file)?,
            None => std::env::var(&a.token_env)
                .ok()
                .filter(|t| !t.trim().is_empty())
                .or_else(|| keychain(&url))
                .ok_or_else(|| {
                    format!(
                        "no key: use --config or --key-file, set {}, or put it in the Keychain (see --help)",
                        a.token_env
                    )
                })?,
        };
        (url, key)
    };
    let outbox = a.outbox.clone().unwrap_or_else(|| {
        let root = default_outbox();
        let folder = memcp_bridge::connection_outbox(&root, &url);
        // Captures left in the folder all connections shared before, whatever vault they were made for, come here.
        match memcp_bridge::adopt_old_captures(&root, &folder) {
            Ok(0) => {}
            Ok(n) => eprintln!(
                "memcp-bridge: {n} capture(s) of the old shared outbox moved to this connection's"
            ),
            Err(e) => eprintln!("memcp-bridge: the old shared outbox cannot be read: {e}"),
        }
        folder
    });
    if memcp_bridge::in_clear(&url) {
        eprintln!(
            "memcp-bridge: WARNING: this address is neither HTTPS nor of this machine or the own network: the key travels in clear"
        );
    }
    let config = Config {
        url,
        key: key.trim().to_string(),
        outbox,
        utc_offset_minutes: a.offset.unwrap_or_else(system_offset),
    };
    match a.mode.as_str() {
        "gateway" => {
            let mut gateway = Gateway::new(config)
                .map_err(|e| e.to_string())?
                .with_capture_dirs(&a.capture_dirs);
            if let Some(path) = &a.socket {
                #[cfg(unix)]
                {
                    let listener = memcp_bridge::bind_socket(path, a.socket_group)
                        .map_err(|e| e.to_string())?;
                    eprintln!("memcp-bridge: serving the socket {}", path.display());
                    memcp_bridge::serve_socket(gateway, &listener).map_err(|e| e.to_string())?;
                }
                #[cfg(not(unix))]
                {
                    let _ = (path, gateway);
                    return Err("--socket needs a Unix socket: use --listen on this system".into());
                }
            } else if let Some(addr) = &a.listen {
                eprintln!("memcp-bridge: serving POST /mcp on {addr}, this machine only");
                serve_http(&mut gateway, addr).map_err(|e| e.to_string())?;
            } else {
                gateway
                    .serve(BufReader::new(io::stdin().lock()), &mut io::stdout().lock())
                    .map_err(|e| e.to_string())?;
            }
            Ok(ExitCode::SUCCESS)
        }
        "send" => {
            if a.rest.is_empty() {
                return Err("send needs at least one file".into());
            }
            let intake = Intake::open(config).map_err(|e| e.to_string())?;
            for file in &a.rest {
                let id = intake
                    .submit_file(std::path::Path::new(file), &a.note)
                    .map_err(|e| format!("{file}: {e}"))?;
                println!("kept {id} ({file})");
            }
            let r = intake.flush();
            println!("{}", summary(&r));
            Ok(code(&r))
        }
        "text" => {
            let text = match a.rest.as_slice() {
                [t] if t == "-" => {
                    let mut s = String::new();
                    io::stdin()
                        .read_to_string(&mut s)
                        .map_err(|e| e.to_string())?;
                    s
                }
                [] => return Err("text needs a note, or - to read it from standard input".into()),
                parts => parts.join(" "),
            };
            let intake = Intake::open(config).map_err(|e| e.to_string())?;
            let id = intake.submit_text(&text).map_err(|e| e.to_string())?;
            println!("kept {id}");
            let r = intake.flush();
            println!("{}", summary(&r));
            Ok(code(&r))
        }
        "flush" => {
            let r = Intake::open(config).map_err(|e| e.to_string())?.flush();
            println!("{}", summary(&r));
            Ok(code(&r))
        }
        "absorb" => {
            if absorbing.0.is_empty() && absorbing.1.is_empty() {
                return Err(
                    "absorb needs --config with `absorb = FOLDER -> VAULT` (or `deliver = ...`)"
                        .into(),
                );
            }
            let everything = a.rest.iter().any(|r| r == "all");
            let state_dir = config.outbox.clone();
            let mut client = memcp_bridge::client_for(&config.url, &config.key);
            let mut code = ExitCode::SUCCESS;
            for src in &absorbing.0 {
                let r = memcp_bridge::absorb(&mut client, src, &state_dir, everything)
                    .map_err(|e| e.to_string())?;
                println!(
                    "{}: files read {}, pieces sent {}, files taken stock of {}, refused {}{}",
                    src.vault,
                    r.files_read,
                    r.sent,
                    r.noted,
                    r.failed,
                    if r.unreachable {
                        ", the memory cannot be reached (next pass)"
                    } else {
                        ""
                    }
                );
                for p in &r.problems {
                    eprintln!("memcp-bridge: {p}");
                }
                if r.key_refused {
                    eprintln!("memcp-bridge: the key is refused or revoked");
                    return Ok(ExitCode::from(1));
                }
                if r.failed > 0 || !r.problems.is_empty() {
                    code = ExitCode::from(3);
                }
            }
            for d in &absorbing.1 {
                let mut r = memcp_bridge::Absorbed::default();
                memcp_bridge::propose_guide(&mut client, d, &state_dir, &mut r);
                let n = memcp_bridge::deliver(&mut client, d, &state_dir, &mut r)
                    .map_err(|e| e.to_string())?;
                println!("{} {}: {n} put down as files", d.vault, d.tag);
                if r.key_refused {
                    return Ok(ExitCode::from(1));
                }
                for p in &r.problems {
                    eprintln!("memcp-bridge: {p}");
                }
            }
            Ok(code)
        }
        "transcribe" => {
            if voice.0.is_empty() {
                return Err("transcribe needs --config with `transcribe = VAULT, VAULT` and `whisper_model` (or `whisper_api_url`)".into());
            }
            let mut client = memcp_bridge::client_for(&config.url, &config.key);
            let r = memcp_bridge::transcribe(
                &mut client,
                &voice.0,
                &voice.1,
                &config.outbox,
                config.utc_offset_minutes,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "transcribed {}, nothing audible {}, erased {}",
                r.transcribed, r.nothing_audible, r.erased
            );
            for p in &r.problems {
                eprintln!("memcp-bridge: {p}");
            }
            if r.key_refused {
                eprintln!("memcp-bridge: the key is refused or revoked");
                return Ok(ExitCode::from(1));
            }
            Ok(if r.problems.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(3)
            })
        }
        "retry" => {
            let intake = Intake::open(config).map_err(|e| e.to_string())?;
            let n = intake
                .retry(a.rest.first().map(String::as_str))
                .map_err(|e| e.to_string())?;
            println!("{n} put back in the queue");
            let r = intake.flush();
            println!("{}", summary(&r));
            Ok(code(&r))
        }
        "forget" => {
            let [id] = a.rest.as_slice() else {
                return Err("forget needs the id of one item (see status)".into());
            };
            Intake::open(config)
                .map_err(|e| e.to_string())?
                .forget(id)
                .map_err(|e| e.to_string())?;
            println!("forgot {id}");
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            let intake = Intake::open(config).map_err(|e| e.to_string())?;
            let items = intake.items();
            if items.is_empty() {
                println!("nothing waits and nothing was sent lately");
            }
            for i in items {
                let state = match i.state {
                    State::Queued => "waiting",
                    State::Sending => "sending",
                    State::Sent => "sent",
                    State::Failed => "refused",
                };
                println!(
                    "{state:8} {} {:9} {}{}",
                    i.id,
                    format!("{:?}", i.kind).to_lowercase(),
                    i.title,
                    if i.detail.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", i.detail)
                    }
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match run(&raw) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("memcp-bridge: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
