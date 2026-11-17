//! The command line for create, read, update, delete: spaces, rules, nodes, connections, people, notes.

use std::process::Command;

struct Home(std::path::PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
        let d = std::env::temp_dir().join(format!("memcp-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let h = Home(d);
        let (ok, out) = h.run(&["init", "--spaces", "home,work"]);
        assert!(ok, "{out}");
        h
    }
    fn run(&self, args: &[&str]) -> (bool, String) {
        let o = Command::new(env!("CARGO_BIN_EXE_memcp"))
            .arg("--data")
            .arg(&self.0)
            .args(args)
            .output()
            .expect("run memcp");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        (o.status.success(), text)
    }
    fn ok(&self, args: &[&str]) -> String {
        let (ok, out) = self.run(args);
        assert!(ok, "{args:?}: {out}");
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn spaces_are_created_changed_deleted_with_a_choice_and_restored() {
    let h = Home::new("space");
    let out = h.ok(&[
        "space",
        "new",
        "Cars",
        "--parent",
        "work",
        "--keywords",
        "garage,tyres",
    ]);
    assert!(out.contains("created"), "{out}");
    let tree = h.ok(&["space", "tree"]);
    assert!(tree.contains("Cars"), "{tree}");
    let name = tree
        .lines()
        .find(|l| l.contains("Cars"))
        .and_then(|l| l.split('(').nth(1))
        .and_then(|l| l.split(',').next())
        .expect("name")
        .to_string();
    h.ok(&[
        "space",
        "set",
        &name,
        "--label",
        "Vehicles",
        "--position",
        "3",
    ]);
    let shown = h.ok(&["space", "show", &name]);
    assert!(
        shown.contains("Vehicles") && shown.contains("garage"),
        "{shown}"
    );
    h.ok(&["rules", "add-keyword", &name, "winter"]);
    assert!(h.ok(&["rules", "show", &name]).contains("winter"));
    h.ok(&["rules", "remove-keyword", &name, "garage"]);
    assert!(!h.ok(&["rules", "show", &name]).contains("garage"));
    // No silent delete.
    let (ok, out) = h.run(&["space", "delete", &name]);
    assert!(!ok && out.contains("never deleted silently"), "{out}");
    let dry = h.ok(&["space", "delete", &name, "--into", "home", "--dry-run"]);
    assert!(dry.starts_with("dry run"), "{dry}");
    assert!(
        h.ok(&["space", "show", &name]).contains("Vehicles"),
        "a dry run changes nothing"
    );
    h.ok(&["space", "delete", &name, "--archive"]);
    assert!(!h.run(&["space", "show", &name]).0);
    let archived = h.ok(&["space", "archived"]);
    let archive = archived
        .split('\t')
        .next()
        .expect("archive name")
        .to_string();
    h.ok(&["space", "restore", &archive]);
    assert!(h.ok(&["space", "show", &name]).contains("Vehicles"));
}

#[test]
fn notes_connections_and_people_have_their_commands() {
    let h = Home::new("note");
    let added = h.ok(&[
        "note",
        "add",
        "home",
        "Buy light bulbs",
        "--tag",
        "shopping",
    ]);
    let id = added.split_whitespace().nth(1).expect("id").to_string();
    assert!(
        h.ok(&["note", "edit", "home", &id, "Buy LED bulbs"])
            .contains("replaced")
    );
    let list = h.ok(&["note", "list", "home"]);
    assert!(
        list.contains("LED") && !list.contains("Buy light bulbs"),
        "{list}"
    );
    let new_id = list.split('\t').next().expect("id").to_string();
    h.ok(&["note", "retire", "home", &new_id, "--reason", "mistake"]);
    assert!(!h.ok(&["note", "list", "home"]).contains("LED"));
    assert!(
        h.ok(&["note", "restore", "home", &new_id])
            .contains("restored")
    );
    assert!(h.ok(&["note", "list", "home"]).contains("LED"));
    // Connections.
    h.ok(&["connect", "helper", "--all"]);
    assert!(
        h.ok(&["connection", "rotate", "helper", "--dry-run"])
            .starts_with("dry run")
    );
    assert!(h.ok(&["connection", "rotate", "helper"]).contains("hcp_"));
    assert!(
        !h.run(&["connection", "delete", "helper"]).0,
        "revoke first"
    );
    h.ok(&["revoke", "helper"]);
    h.ok(&["connection", "delete", "helper"]);
    assert!(!h.run(&["connection", "show", "helper"]).0);
    // Nodes.
    h.ok(&["connect", "peer-laptop-ab12", "--all"]);
    assert!(h.ok(&["node", "list"]).contains("laptop"));
    h.ok(&["node", "pause", "peer-laptop-ab12"]);
    assert!(h.ok(&["node", "list"]).contains("paused"));
    h.ok(&["node", "resume", "peer-laptop-ab12"]);
    h.ok(&["node", "revoke", "peer-laptop-ab12"]);
    h.ok(&["node", "forget", "peer-laptop-ab12"]);
    assert!(!h.ok(&["node", "list"]).contains("laptop"));
    assert!(h.ok(&["node", "list", "--all"]).contains("revoked"));
}
