//! The Linux settings answer to the Home Assistant add-on's options (docs/PARITY.md), the example file is the
//! documented one, and every `doctor` check the documentation lists exists.

use std::collections::BTreeSet;
use std::path::PathBuf;

use memcp_server::config::{self, Settings, Value};
use memcp_server::doctor;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    match std::fs::read_to_string(root().join(rel)) {
        Ok(t) => t,
        Err(e) => panic!("{rel}: {e}"),
    }
}

/// The options of `ha-addon/memcp/config.yaml`: the keys under `options:`.
fn addon_options() -> Vec<String> {
    let yaml = read("ha-addon/memcp/config.yaml");
    let mut keys = vec![];
    let mut inside = false;
    for line in yaml.lines() {
        if line.starts_with("options:") {
            inside = true;
            continue;
        }
        if inside {
            if !line.starts_with(' ') {
                break;
            }
            if let Some(k) = line.strip_prefix("  ").filter(|l| !l.starts_with(' '))
                && let Some((key, _)) = k.split_once(':')
            {
                keys.push(key.trim().to_string());
            }
        }
    }
    keys
}

#[test]
fn every_addon_option_has_a_memcp_toml_counterpart_and_a_row_in_the_parity_table() {
    let addon = addon_options();
    assert!(addon.len() >= 20, "the options were not read: {addon:?}");
    let table = read("docs/PARITY.md");
    for key in &addon {
        let opt = config::all()
            .into_iter()
            .find(|o| o.addon == Some(key.as_str()))
            .unwrap_or_else(|| {
                panic!("add-on option `{key}` has no counterpart in memcp.toml (crates/server/src/config.rs)")
            });
        let row = format!("| `{key}` | `{}` |", opt.key);
        assert!(
            table.contains(&row),
            "docs/PARITY.md has no row {row:?} for the add-on option `{key}`"
        );
    }
    // And nothing claims a counterpart for an option the add-on does not have.
    let known: BTreeSet<&str> = addon.iter().map(String::as_str).collect();
    for opt in config::all() {
        if let Some(a) = opt.addon {
            assert!(
                known.contains(a),
                "{} answers to unknown add-on option {a}",
                opt.key
            );
        }
    }
}

#[test]
fn the_options_that_are_only_for_linux_are_listed_as_such() {
    let table = read("docs/PARITY.md");
    for opt in config::all().into_iter().filter(|o| o.addon.is_none()) {
        assert!(
            table.contains(&format!("`{}`", opt.key)),
            "docs/PARITY.md does not mention the Linux-only option `{}`",
            opt.key
        );
    }
}

#[test]
fn the_shipped_memcp_toml_is_the_documented_one_with_every_option() {
    // A checkout on Windows may turn the line ends into CRLF: the content is what counts.
    let shipped = read("packaging/linux/memcp.toml").replace("\r\n", "\n");
    assert_eq!(
        shipped,
        Settings::default().render(),
        "packaging/linux/memcp.toml is out of date: regenerate it with `memcp config example`"
    );
    for opt in config::all() {
        assert!(shipped.contains(&format!("{} = ", opt.key)), "{}", opt.key);
    }
    // The file as shipped, with nothing uncommented, reads; and the same file with every default uncommented too.
    let s = Settings::parse(&shipped).unwrap_or_else(|e| panic!("{e}"));
    assert!(!s.is_set("data"));
    let all_on: String = shipped
        .lines()
        .map(|l| {
            l.strip_prefix("# ")
                .filter(|r| r.contains(" = "))
                .unwrap_or(l)
        })
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = Settings::parse(&all_on) {
        panic!("the defaults do not read back: {e}");
    }
}

#[test]
fn the_file_reads_values_of_every_kind_and_refuses_the_wrong_ones() {
    let s = Settings::parse(
        "# a comment\nnode_name = \"Mine # not a comment\" # a comment\nthreads = 8\nadvertise = true\n\
         admins = [\"a\", \"b\"]\ntailnet_users = [\n  \"x@example.org=owner\", # first\n  \"y@example.org=guest\",\n]\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.text("node_name"), Some("Mine # not a comment"));
    assert_eq!(s.int("threads"), Some(8));
    assert!(s.flag("advertise"));
    assert_eq!(s.list("admins"), ["a", "b"]);
    assert_eq!(s.list("tailnet_users").len(), 2);
    for bad in [
        "threads = \"4\"",
        "threads = 0",
        "advertise = yes",
        "language = \"de\"",
        "tailnet_users = [\"nobody\"]",
        "janitor_night = \"25:00\"",
        "public_url = \"https://x.example/\"",
        "unknown = 1",
        "threads = 1\nthreads = 2",
        "[section]",
        "node_name = \"open",
    ] {
        assert!(Settings::parse(bad).is_err(), "{bad:?} was accepted");
    }
    let mut s = Settings::default();
    assert!(s.set("language", Value::Text("fr".into())).is_ok());
    assert!(s.set("language", Value::Text("xx".into())).is_err());
    assert!(s.render().contains("language = \"fr\""));
}

#[test]
fn every_option_with_a_flag_reaches_the_command_line() {
    let mut flags = std::collections::HashMap::new();
    let mut switches = vec![];
    flags.insert("--listen".to_string(), "127.0.0.1:1".to_string());
    let s = Settings::parse(
        "listen = \"0.0.0.0:9\"\nadvertise = true\ntailnet_users = [\"a=b\", \"c=d\"]\npublic_url = \"https://m.example\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    config::fill_flags(&s, &mut flags, &mut switches);
    assert_eq!(flags["--listen"], "127.0.0.1:1", "the command line wins");
    assert_eq!(flags["--tailnet-user"], "a=b,c=d");
    assert_eq!(flags["--public-url"], "https://m.example");
    assert_eq!(switches, ["--advertise"]);
}

#[test]
fn every_doctor_check_the_documentation_lists_exists() {
    let doc = read("docs/LINUX.md");
    let section = doc
        .split("## What `memcp doctor` checks")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .unwrap_or_else(|| panic!("docs/LINUX.md has no doctor section"));
    let listed: BTreeSet<String> = section
        .lines()
        .filter_map(|l| l.strip_prefix("- `"))
        .filter_map(|l| l.split('`').next())
        .map(String::from)
        .collect();
    let coded: BTreeSet<String> = doctor::CHECKS.iter().map(|c| c.0.to_string()).collect();
    assert_eq!(listed, coded, "docs/LINUX.md and doctor::CHECKS differ");
    // The phrase each check writes is in the program.
    let source = read("crates/server/src/doctor.rs");
    for (id, phrase, _) in doctor::CHECKS.iter() {
        let hits = source.matches(phrase).count();
        assert!(
            hits >= 2,
            "check {id}: the phrase {phrase:?} is only in the list, nothing writes it"
        );
    }
}

#[test]
fn the_doctor_says_what_a_settings_file_and_the_backups_add() {
    let dir = std::env::temp_dir().join(format!("memcp-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let mut s = Settings::default();
    let backups = dir.join("backups");
    s.set("backup_dir", Value::Text(backups.display().to_string()))
        .unwrap_or_else(|e| panic!("{e}"));
    s.set("janitor", Value::Bool(true))
        .unwrap_or_else(|e| panic!("{e}"));
    let extra = doctor::Extra {
        settings_file: Some(dir.as_path()),
        settings: &s,
    };
    let mut r = doctor::inspect(&dir.join("nowhere"), None, None);
    doctor::linux(&mut r, &extra);
    let lines: Vec<(&str, &str)> = r.lines().collect();
    assert!(
        lines
            .iter()
            .any(|(l, t)| *l == "warn" && t.contains("backup: none"))
    );
    assert!(
        lines
            .iter()
            .any(|(l, t)| *l == "warn" && t.contains("settings file: janitor is on"))
    );
    std::fs::create_dir_all(backups.join("backup-20260101-000000"))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut r = doctor::inspect(&dir.join("nowhere"), None, None);
    doctor::linux(&mut r, &extra);
    assert!(
        r.lines()
            .any(|(l, t)| l == "ok" && t.contains("backup: the newest"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}
