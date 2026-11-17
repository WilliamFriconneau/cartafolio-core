//! The owner's commands for create, read, update and delete (see docs/CRUD.md): `space`, `rules`, `node`,
//! `connection`, `person`, `note`, plus `library remove` and `vocabulary term-add|term-remove`.
//! Every destructive command takes `--dry-run` and says what it would do; every change is audited by the core.

use std::path::Path;

use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::layout::Change;
use memcp_core::lifecycle::{Children, Disposition, SpaceNew, SpaceUpdate};
use memcp_core::util::{iso, now};
use memcp_core::{Brain, Space, access, peers, people, rules, users};

use crate::cli::{Failure, bad, fail};

/// What the command line gave: positional words and options.
pub struct Cmd<'a> {
    pub pos: &'a [String],
    pub flag: &'a dyn Fn(&str) -> Option<&'a str>,
    pub sw: &'a dyn Fn(&str) -> bool,
}

impl Cmd<'_> {
    fn arg(&self, i: usize, what: &str) -> Result<&str, Failure> {
        self.pos
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| bad(&format!("missing {what}")))
    }
    fn need(&self, name: &str) -> Result<&str, Failure> {
        (self.flag)(name).ok_or_else(|| bad(&format!("missing {name}")))
    }
    fn list(&self, name: &str) -> Vec<String> {
        (self.flag)(name)
            .map(|v| {
                v.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn dry(&self) -> bool {
        (self.sw)("--dry-run")
    }
}

fn open(data: &Path) -> Result<Brain, Failure> {
    let brain = Brain::open(data).map_err(fail)?;
    memcp_core::text::set_language(brain.language().map_err(fail)?);
    Ok(brain)
}

/// One tool of the memory, called with the person's own rights (the page does the same): what an assistant could do.
fn tool(
    brain: &Brain,
    who: &users::User,
    name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, Failure> {
    let ctl = brain.open_control().map_err(fail)?;
    let grants = users::access_of(&ctl, &who.id).map_err(fail)?;
    let caller = access::Caller {
        id: "cli".into(),
        agent: "cli".into(),
        user_id: who.id.clone(),
        all_spaces: false,
        known: grants.keys().cloned().collect(),
        grants,
        paths: Default::default(),
        read_only_forced: false,
        no_person_guide: true,
        rate_per_min: access::DEFAULT_RATE_PER_MIN,
        max_text: access::DEFAULT_MAX_TEXT,
    };
    memcp_core::tools::call(brain, &caller, name, args).map_err(fail)
}

fn me(brain: &Brain, c: &Cmd) -> Result<users::User, Failure> {
    let ctl = brain.open_control().map_err(fail)?;
    users::resolve(&ctl, (c.flag)("--user").unwrap_or(users::OWNER)).map_err(fail)
}

fn space(s: &str) -> Result<Space, Failure> {
    Space::new(s).map_err(fail)
}

fn parent(v: &str) -> Result<Option<Space>, Failure> {
    if v.is_empty() || v == "none" {
        Ok(None)
    } else {
        space(v).map(Some)
    }
}

/// Runs one of these commands; `None` when the words are not one of them.
pub fn run(data: &Path, c: &Cmd) -> Option<Result<(), Failure>> {
    let (a, b) = (c.pos.first()?.as_str(), c.pos.get(1).map(String::as_str)?);
    Some(match (a, b) {
        ("space", "new") => space_new(data, c),
        ("space", "show") => space_show(data, c),
        ("space", "tree") => space_tree(data, c),
        ("space", "set") => space_set(data, c),
        ("space", "delete") => space_delete(data, c),
        ("space", "archived") => space_archived(data, c),
        ("space", "restore") => space_restore(data, c),
        ("rules", _) => rules_cmd(data, c, b),
        ("node", _) => node(data, c, b),
        ("connection", _) => connection(data, c, b),
        ("person", _) => person(data, c, b),
        ("note", _) => note(data, c, b),
        ("library", "remove") => library_remove(data, c),
        ("vocabulary", "term-add" | "term-remove") => vocabulary_terms(data, c, b == "term-add"),
        _ => return None,
    })
}

fn space_new(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let s = brain
        .create_space_with(
            &who.id,
            &who.name,
            &SpaceNew {
                kind: (c.flag)("--kind").map(String::from),
                label: c.arg(2, "the label")?.to_string(),
                parent: (c.flag)("--parent").map(parent).transpose()?.flatten(),
                keywords: c.list("--keywords"),
                tags: c.list("--tags"),
            },
        )
        .map_err(fail)?;
    sayln!("space {s} created ({}).", c.arg(2, "")?);
    Ok(())
}

fn space_show(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let i = brain
        .space_info(&who.id, &space(c.arg(2, "the space")?)?)
        .map_err(fail)?;
    sayln!(
        "{}\t{}\tkind {}\tparent {}\t{} notes, {} files",
        i.space,
        i.label,
        i.kind,
        i.parent.as_ref().map_or("-", Space::as_str),
        i.facts,
        i.files
    );
    sayln!("keywords: {}", i.rules.keywords.join(", "));
    sayln!("tags: {}", i.rules.tags.join(", "));
    Ok(())
}

fn space_tree(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let places = brain.layout(&who.id).map_err(fail)?;
    for p in &places {
        let mut depth = 0;
        let mut up = p.parent.clone();
        while let Some(u) = up {
            depth += 1;
            up = places
                .iter()
                .find(|x| x.space == u)
                .and_then(|x| x.parent.clone());
            if depth > 9 {
                break;
            }
        }
        sayln!(
            "{}{}  ({}, {})",
            "  ".repeat(depth),
            p.label,
            p.space,
            p.kind
        );
    }
    Ok(())
}

fn space_set(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let s = space(c.arg(2, "the space")?)?;
    let u = SpaceUpdate {
        layout: Change {
            label: (c.flag)("--label").map(String::from),
            parent: if (c.sw)("--top") {
                Some(None)
            } else {
                (c.flag)("--parent").map(parent).transpose()?
            },
            position: (c.flag)("--position").and_then(|p| p.parse().ok()),
        },
        kind: (c.flag)("--kind").map(String::from),
        rules: rules::Edit::default(),
    };
    let i = brain
        .update_space(&who.id, &who.name, &s, &u)
        .map_err(fail)?;
    sayln!(
        "space {s}: {} (kind {}, parent {}, position {}).",
        i.label,
        i.kind,
        i.parent.as_ref().map_or("-", Space::as_str),
        i.position
    );
    Ok(())
}

fn space_delete(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let s = space(c.arg(2, "the space")?)?;
    let disposition = match ((c.flag)("--into"), (c.sw)("--archive")) {
        (Some(t), false) => Disposition::Into(space(t)?),
        (None, true) => Disposition::Archive,
        _ => {
            return Err(bad(
                "a space is never deleted silently: choose --into SPACE or --archive",
            ));
        }
    };
    let children = (c.flag)("--children").map_or(Ok(Children::default()), |v| {
        Children::parse(v).map_err(fail)
    })?;
    let r = brain
        .delete_space(&who.id, &s, &disposition, &children, c.dry(), now())
        .map_err(fail)?;
    sayln!(
        "{}{}",
        if r.dry_run { "dry run: " } else { "" },
        r.sentence()
    );
    for (k, p) in &r.children {
        sayln!(
            "  sub-space {k} goes {}",
            p.as_deref()
                .map_or("to the top".into(), |p| format!("under {p}"))
        );
    }
    if !r.connections.is_empty() {
        sayln!("  connections losing access: {}", r.connections.join(", "));
    }
    if let (false, Some(a)) = (r.dry_run, &r.archive) {
        sayln!(
            "  archive: {}  (restore with: space restore {})",
            a.display(),
            a.file_name()
                .map_or(String::new(), |n| n.to_string_lossy().to_string())
        );
    }
    Ok(())
}

fn space_archived(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    for a in brain.archived_spaces(&who.id).map_err(fail)? {
        sayln!(
            "{}\t{}\t{} notes\tarchived {}{}",
            a.name,
            a.label,
            a.facts,
            iso(a.at),
            a.into.map_or(String::new(), |t| format!("\tinto {t}"))
        );
    }
    Ok(())
}

fn space_restore(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let s = brain
        .restore_space(&who.id, c.arg(2, "the archive name")?, c.dry())
        .map_err(fail)?;
    sayln!(
        "{}space {s} {}.",
        if c.dry() { "dry run: " } else { "" },
        if c.dry() {
            "would come back"
        } else {
            "restored"
        }
    );
    Ok(())
}

fn rules_cmd(data: &Path, c: &Cmd, op: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let s = space(c.arg(2, "the space")?)?;
    brain.space_info(&who.id, &s).map_err(fail)?;
    let conn = brain.open_space(&s).map_err(fail)?;
    let word = || c.arg(3, "the word");
    let edit = match op {
        "show" => {
            let r = rules::read(&conn).1;
            sayln!(
                "keywords: {}\ntags: {}",
                r.keywords.join(", "),
                r.tags.join(", ")
            );
            return Ok(());
        }
        "add-keyword" => rules::Edit {
            add_keywords: vec![word()?.into()],
            ..Default::default()
        },
        "remove-keyword" => rules::Edit {
            remove_keywords: vec![word()?.into()],
            ..Default::default()
        },
        "add-tag" => rules::Edit {
            add_tags: vec![word()?.into()],
            ..Default::default()
        },
        "remove-tag" => rules::Edit {
            remove_tags: vec![word()?.into()],
            ..Default::default()
        },
        _ => {
            return Err(bad(
                "rules show|add-keyword|remove-keyword|add-tag|remove-tag SPACE [WORD]",
            ));
        }
    };
    let r = rules::edit(&conn, &s, &who.name, &edit).map_err(fail)?;
    sayln!(
        "keywords: {}\ntags: {}",
        r.keywords.join(", "),
        r.tags.join(", ")
    );
    Ok(())
}

fn node(data: &Path, c: &Cmd, op: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let t = now();
    if op == "list" {
        for n in peers::list(&brain, &who, t, (c.sw)("--all")).map_err(fail)? {
            sayln!(
                "{:<26} {:<18} {:<8} last seen {}  {}",
                n.peer,
                n.name,
                n.state,
                n.last_seen().map_or("never".into(), iso),
                n.note
            );
        }
        return Ok(());
    }
    let peer = c.arg(2, "the peer (peer-...)")?;
    let n = peers::get(&brain, &who, peer, t).map_err(fail)?;
    if c.dry() {
        sayln!("dry run: {} ({}) would be {op}d", n.name, n.state);
        return Ok(());
    }
    match op {
        "rename" => {
            peers::update(&brain, &who, peer, Some(c.arg(3, "the new name")?), None)
                .map_err(fail)?;
        }
        "note" => {
            peers::update(&brain, &who, peer, None, Some(c.arg(3, "the note")?)).map_err(fail)?;
        }
        "pause" => {
            peers::set_paused(&brain, &who, peer, true).map_err(fail)?;
        }
        "resume" => {
            peers::set_paused(&brain, &who, peer, false).map_err(fail)?;
        }
        "revoke" => {
            peers::revoke(&brain, &who, peer).map_err(fail)?;
        }
        "forget" => peers::forget(&brain, &who, peer, t).map_err(fail)?,
        _ => {
            return Err(bad(
                "node list|rename|note|pause|resume|revoke|forget PEER [TEXT]",
            ));
        }
    }
    sayln!("{peer}: {op} done.");
    Ok(())
}

fn connection(data: &Path, c: &Cmd, op: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let ctl = brain.open_control().map_err(fail)?;
    let agent = c.arg(2, "the connection")?;
    let info = access::get(&ctl, agent)
        .map_err(fail)?
        .filter(|i| i.user_id == who.id)
        .ok_or_else(|| fail(format!("no connection {agent:?} for {}", who.name)))?;
    match op {
        "show" => sayln!(
            "{}\t{}\t{}\tlast used {}",
            info.agent,
            info.rights,
            if info.revoked_at.is_some() {
                "revoked"
            } else {
                "active"
            },
            info.last_used_at.map_or("never".into(), iso)
        ),
        "rename" => {
            access::rename(&ctl, agent, c.arg(3, "the new name")?).map_err(fail)?;
            sayln!("{agent} renamed.");
        }
        "rotate" => {
            if c.dry() {
                sayln!(
                    "dry run: {agent} would get a new key at the same address; the old key stops at once."
                );
                return Ok(());
            }
            let t = access::rotate(&ctl, agent, &who.id)
                .map_err(fail)?
                .ok_or_else(|| fail("the connection is revoked"))?;
            sayln!("New key for {agent} (shown ONCE, the old one stopped):\n  {t}");
        }
        "delete" => {
            if c.dry() {
                sayln!(
                    "dry run: {agent} would be removed from the list (revoked: {}).",
                    info.revoked_at.is_some()
                );
                return Ok(());
            }
            access::delete(&ctl, agent, &who.id).map_err(fail)?;
            sayln!("{agent} removed from the list.");
        }
        _ => return Err(bad("connection show|rename|rotate|delete AGENT [NAME]")),
    }
    Ok(())
}

fn person(data: &Path, c: &Cmd, op: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let actor = me(&brain, c)?;
    let ctl = brain.open_control().map_err(fail)?;
    if op == "list" {
        for u in users::list_users(&ctl).map_err(fail)? {
            sayln!(
                "{}\t{}\t{}\t{}",
                u.name,
                u.display,
                if u.admin { "admin" } else { "member" },
                if u.disabled { "disabled" } else { "active" }
            );
        }
        return Ok(());
    }
    let name = c.arg(2, "the person")?;
    if op == "new" {
        let admin = match (c.flag)("--role").unwrap_or("member") {
            "admin" => true,
            "member" => false,
            _ => return Err(bad("--role: admin or member")),
        };
        let u = users::create_user(
            &ctl,
            &actor.id,
            name,
            (c.flag)("--display").unwrap_or(name),
            admin,
        )
        .map_err(fail)?;
        sayln!(
            "{} created ({}). Make them a space with `space make KIND --user {}`.",
            u.name,
            if u.admin { "admin" } else { "member" },
            u.name
        );
        return Ok(());
    }
    let target = users::user(&ctl, name)
        .map_err(fail)?
        .or(users::user_by_name(&ctl, name).map_err(fail)?)
        .ok_or_else(|| fail(format!("unknown person {name}")))?;
    match op {
        "show" => sayln!(
            "{}\t{}\t{}\their heir: {}\trecovery key: {}",
            target.name,
            target.display,
            if target.admin { "admin" } else { "member" },
            users::heir_of(&ctl, &target.id)
                .map_err(fail)?
                .unwrap_or_else(|| "none".into()),
            if users::has_recovery_key(&ctl, &target.id).map_err(fail)? {
                "yes"
            } else {
                "no"
            }
        ),
        "set" => {
            if let Some(d) = (c.flag)("--display") {
                users::set_display(&ctl, &target.id, d).map_err(fail)?;
            }
            if let Some(r) = (c.flag)("--role") {
                users::set_admin(&ctl, &actor.id, &target.id, r == "admin").map_err(fail)?;
            }
            sayln!("{} updated.", target.name);
        }
        "heir" => {
            users::set_heir(&ctl, &target.id, c.pos.get(3).map(String::as_str)).map_err(fail)?;
            sayln!("{}: heir set.", target.name);
        }
        "recovery-key" => {
            if (c.sw)("--clear") {
                users::clear_recovery_key(&ctl, &target.id).map_err(fail)?;
                sayln!("{}: recovery key cleared.", target.name);
            } else {
                let k = users::make_recovery_key(&ctl, &target.id).map_err(fail)?;
                sayln!(
                    "Recovery key for {} (shown ONCE, replaces any earlier one):\n  {k}",
                    target.name
                );
            }
        }
        "disable" | "enable" => {
            if c.dry() {
                sayln!(
                    "dry run: {} would be {}d; nothing is deleted.",
                    target.name,
                    op
                );
                return Ok(());
            }
            users::set_disabled(&ctl, &actor.id, &target.id, op == "disable").map_err(fail)?;
            sayln!("{}: {op}d.", target.name);
        }
        "delete" => {
            let r =
                people::retire_person(&brain, &actor, &target.id, c.dry(), now()).map_err(fail)?;
            sayln!(
                "{}{}",
                if r.dry_run { "dry run: " } else { "" },
                r.sentence()
            );
            if let (false, Some(p)) = (r.dry_run, &r.export) {
                sayln!("  export: {}", p.display());
            }
        }
        _ => {
            return Err(bad(
                "person list|show|new|set|heir|recovery-key|disable|enable|delete NAME",
            ));
        }
    }
    Ok(())
}

fn note(data: &Path, c: &Cmd, op: &str) -> Result<(), Failure> {
    let brain = open(data)?;
    let who = me(&brain, c)?;
    let s = space(c.arg(2, "the space")?)?;
    brain.space_info(&who.id, &s).map_err(fail)?;
    let conn = brain.open_space(&s).map_err(fail)?;
    let author = format!("{}-cli", who.name);
    let tags = c.list("--tag");
    match op {
        "add" => {
            let mut new = NewFact {
                tags,
                ..NewFact::agent(c.arg(3, "the text")?, &author)
            };
            let text = new.text.clone();
            memcp_core::autotag::add_to(&brain, &who.id, &s, &conn, &text, &mut new.tags);
            match facts::remember(&conn, &s, &author, new, &[]).map_err(fail)? {
                Remembered::Created(f) => sayln!("note {} created.", f.id),
                Remembered::Duplicate(f) => sayln!("already there: {}.", f.id),
                Remembered::NeedsDecision(cands) => {
                    for x in cands {
                        sayln!("close to {}: {}", x.id, x.text);
                    }
                    return Err(bad("close notes exist: correct one with `note edit`"));
                }
            }
        }
        "show" => {
            for f in facts::history(&conn, c.arg(3, "the note id")?).map_err(fail)? {
                sayln!("{}\t{}\t{}\t{}", f.id, f.status, iso(f.created_at), f.text);
            }
        }
        "list" => {
            let mut st = conn
                .prepare("SELECT id, text FROM facts WHERE status = 'active' ORDER BY created_at DESC, rowid DESC LIMIT 50")
                .map_err(fail)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(fail)?;
            for row in rows {
                let (id, text) = row.map_err(fail)?;
                sayln!("{id}\t{text}");
            }
        }
        "recall" => {
            let query = c.arg(3, "the words to look for")?;
            let opts = facts::RecallOptions {
                limit: (c.flag)("--limit")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(10),
                tags: tags.clone(),
                ..Default::default()
            };
            for h in facts::recall(&conn, query, &opts, now()).map_err(fail)? {
                sayln!("{}\t{}", h.fact.id, h.fact.text);
            }
        }
        "tags" => {
            let given = |name: &str| (c.flag)(name).map(|_| c.list(name));
            let args = serde_json::json!({
                "id": c.arg(3, "the note id")?,
                "tags": given("--tag"),
                "add": given("--add"),
                "remove": given("--remove"),
            });
            let out = tool(&brain, &who, "tags_set", &args)?;
            let list: Vec<&str> = out["tags"]
                .as_array()
                .map(|t| t.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            sayln!("note {}: tags {}.", c.arg(3, "")?, list.join(", "));
        }
        "edit" => {
            let new = NewFact {
                tags,
                ..NewFact::agent(c.arg(4, "the new text")?, &author)
            };
            let f = facts::supersede(
                &conn,
                &s,
                &author,
                c.arg(3, "the note id")?,
                new,
                (c.flag)("--reason"),
            )
            .map_err(fail)?;
            sayln!("note replaced by {}.", f.id);
        }
        "retire" => {
            let f = facts::retire(
                &conn,
                &author,
                c.arg(3, "the note id")?,
                c.need("--reason")?,
            )
            .map_err(fail)?;
            sayln!(
                "note {} retired (it stays in the history; `note restore` brings it back).",
                f.id
            );
        }
        "restore" => {
            let f = facts::restore(
                &conn,
                &s,
                &author,
                c.arg(3, "the note id")?,
                (c.flag)("--reason"),
            )
            .map_err(fail)?;
            sayln!("note restored as {}.", f.id);
        }
        _ => {
            return Err(bad(
                "note add|show|list|recall|tags|edit|retire|restore SPACE ...",
            ));
        }
    }
    Ok(())
}

fn library_remove(data: &Path, c: &Cmd) -> Result<(), Failure> {
    let brain = open(data)?;
    let s = space(c.need("--space")?)?;
    let conn = brain.open_space(&s).map_err(fail)?;
    let want = c.arg(2, "the folder")?;
    let canon = std::fs::canonicalize(want)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| want.to_string());
    let root = memcp_core::library::roots(&conn)
        .map_err(fail)?
        .into_iter()
        .find(|r| r.root == canon || r.root == want)
        .ok_or_else(|| fail(format!("{want} is not mounted on {s}")))?;
    if c.dry() {
        sayln!(
            "dry run: {} would be unmounted from {s}: its index is forgotten, the folder is never touched.",
            root.root
        );
        return Ok(());
    }
    memcp_core::library::remove_root(&conn, root.id).map_err(fail)?;
    sayln!(
        "{} unmounted from {s}. The folder itself was not touched.",
        root.root
    );
    Ok(())
}

fn vocabulary_terms(data: &Path, c: &Cmd, add: bool) -> Result<(), Failure> {
    let brain = open(data)?;
    let s = space(c.need("--space")?)?;
    let mut conn = brain.open_space(&s).map_err(fail)?;
    let name = c.arg(2, "the vocabulary")?;
    let terms: Vec<String> = c.pos[3..].to_vec();
    if terms.is_empty() {
        return Err(bad("give the terms after the vocabulary name"));
    }
    let n = if add {
        memcp_core::vocabulary::add_terms(&mut conn, name, &terms)
    } else {
        memcp_core::vocabulary::remove_terms(&mut conn, name, &terms)
    }
    .map_err(fail)?;
    sayln!(
        "{name} on {s}: {n} term(s) {}.",
        if add { "added" } else { "removed" }
    );
    Ok(())
}
