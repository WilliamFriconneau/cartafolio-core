//! Every tool against every kind of connection: which tools it is shown, which calls are answered, which are refused, and
//! that a refused call changes nothing.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::tools::{self, ToolError};
use memcp_core::triage;
use memcp_core::users::OWNER;
use memcp_core::{Brain, util};
use serde_json::{Value, json};

const READ: [&str; 16] = [
    "triage_waiting",
    "tags_suggest",
    "health_referential",
    "living_get",
    "peers",
    "rules_get",
    "library_search",
    "folder_get",
    "links",
    "recall",
    "read_fact",
    "history",
    "stats",
    "spaces",
    "file_list",
    "file_get",
];
const WRITE: [&str; 12] = [
    "living_set",
    "tags_set",
    "rules_set",
    "library_raise",
    "folder_set",
    "remember",
    "deposit",
    "supersede",
    "retire",
    "file_put",
    "file_retire",
    "file_erase",
];
const SORT: [&str; 5] = [
    "tidy",
    "triage_claim",
    "triage_release",
    "triage_send",
    "transcription_failed",
];
const GUIDE: [&str; 2] = ["guide_set", "guide_propose"];

fn every_tool() -> Vec<&'static str> {
    READ.iter()
        .chain(&WRITE)
        .chain(&SORT)
        .chain(&GUIDE)
        .copied()
        .collect()
}

struct World {
    t: Temp,
    brain: Brain,
    /// A fact of `work`, one of `health`, and one of `work` filed under `sources/app`.
    work_fact: String,
    health_fact: String,
    app_fact: String,
}

fn put(
    b: &Brain,
    space: &str,
    text: &str,
    tags: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let s = sp(space);
    let conn = b.open_space(&s)?;
    let mut n = NewFact::agent(text, "agent:setup");
    n.tags = tags.iter().map(|t| (*t).to_string()).collect();
    match facts::remember(&conn, &s, "setup", n, &[])? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn world() -> Result<World, Box<dyn std::error::Error>> {
    let t = Temp::new("matrix");
    let b = brain(&t)?;
    triage::ensure(&b, OWNER, "owner")?;
    let work_fact = put(&b, "work", "Quarterly review is on the first Monday.", &[])?;
    let health_fact = put(&b, "health", "Blood pressure was normal at the visit.", &[])?;
    let app_fact = put(
        &b,
        "work",
        "Build number nine hundred passed on the runner.",
        &["sources/app"],
    )?;
    Ok(World {
        t,
        brain: b,
        work_fact,
        health_fact,
        app_fact,
    })
}

/// A connection used from the node's own machine (`local`), as most of the matrix is; [`afar`] is the same key used
/// from elsewhere.
fn caller(
    w: &World,
    name: &str,
    rights: &str,
    read_only: bool,
) -> Result<Caller, Box<dyn std::error::Error>> {
    let mut c = afar(w, name, rights, read_only)?;
    c.local = true;
    Ok(c)
}

fn afar(
    w: &World,
    name: &str,
    rights: &str,
    read_only: bool,
) -> Result<Caller, Box<dyn std::error::Error>> {
    let ctl = w.brain.open_control()?;
    let tok = (if name.starts_with("peer-") {
        access::connect_peer(&ctl, OWNER, name, &Rights::parse(rights)?, false)
    } else {
        access::connect_for(
            &ctl,
            OWNER,
            name,
            &Rights::parse(rights)?,
            None,
            read_only,
            false,
        )
    })?;
    Ok(access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?)
}

/// What a call came to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Out {
    Ok,
    /// The tool is not shown to this connection: `unknown tool`.
    Hidden,
    /// Shown, but this call is refused or invalid.
    Refused,
}

fn run(w: &World, c: &Caller, tool: &str, args: Value) -> Out {
    match tools::call(&w.brain, c, tool, &args) {
        Ok(_) => Out::Ok,
        Err(ToolError::Unknown) => Out::Hidden,
        Err(_) => Out::Refused,
    }
}

/// The tools each kind of connection is shown.
fn shown(kind: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = vec![];
    match kind {
        "all-rw" | "peer" | "one-rw" => {
            v.extend(READ);
            v.extend(WRITE);
            v.extend(SORT);
            v.push("guide_set");
        }
        // A key used from another machine is never offered to point a space at a folder of this one.
        "all-rw-afar" => {
            v.extend(READ);
            v.extend(WRITE.iter().filter(|t| **t != "folder_set"));
            v.extend(SORT);
            v.push("guide_set");
        }
        "one-r" | "forced-read-only" => v.extend(READ),
        "path-rw" => v.extend([
            "recall", "remember", "deposit", "retire", "file_put", "stats",
        ]),
        "path-r" => v.extend(["recall", "stats"]),
        "guide-path" => v.extend([
            "recall",
            "remember",
            "deposit",
            "retire",
            "file_put",
            "stats",
            "guide_propose",
        ]),
        other => panic!("unknown kind {other}"),
    }
    v
}

fn kinds(w: &World) -> Result<Vec<(&'static str, Caller)>, Box<dyn std::error::Error>> {
    Ok(vec![
        ("all-rw", caller(w, "k-all", "all", false)?),
        ("all-rw-afar", afar(w, "k-all-afar", "all", false)?),
        ("one-r", caller(w, "k-one-r", "work:r", false)?),
        ("one-rw", caller(w, "k-one-rw", "work:rw", false)?),
        (
            "path-rw",
            caller(w, "k-path-rw", "work/sources/app:rw", false)?,
        ),
        (
            "path-r",
            caller(w, "k-path-r", "work/sources/app:r", false)?,
        ),
        (
            "guide-path",
            caller(w, "k-guide", "work/sources/app:rw,work/guide:rw", false)?,
        ),
        ("forced-read-only", caller(w, "k-forced", "all", true)?),
        ("peer", caller(w, "peer-laptop-ab12", "all", false)?),
    ])
}

/// A call to `tool` aimed at the space `space`, with plausible arguments.
fn args_for(w: &World, tool: &str, space: &str, fact: &str) -> Value {
    match tool {
        "remember" => {
            json!({"space": space, "text": "A new note about the garden fence.", "tags": ["sources/app"]})
        }
        "deposit" => {
            json!({"space": space, "records": [{"text": "Deposited line about rain.", "origin": {"app": "app", "id": "r1", "at": "2026-11-01"}}]})
        }
        "supersede" => json!({"id": fact, "text": "Quarterly review moved to the second Monday."}),
        "retire" => json!({"id": fact, "reason": "test"}),
        "tags_set" => json!({"id": fact, "add": ["checked"]}),
        "tags_suggest" => json!({"space": space, "text": "douleur 2/10, tension 12/8"}),
        "health_referential" => json!({"kind": "vitals"}),
        "recall" => json!({"query": "review", "space": space}),
        "read_fact" | "history" => json!({"id": fact}),
        "tidy" | "triage_claim" | "triage_release" | "triage_waiting" | "peers" | "spaces"
        | "stats" => {
            json!({})
        }
        "triage_send" => json!({"id": "nothing", "to": space}),
        "transcription_failed" => json!({"id": "nothing"}),
        "living_get" => json!({"space": space, "page": "next"}),
        "living_set" => {
            json!({"space": space, "page": "next", "add": [{"column": "now", "text": "Call"}]})
        }
        "rules_get" => json!({"space": space}),
        // A different word each time: writing the same rules twice has its own test (rules_tools.rs).
        "rules_set" => {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            json!({"space": space, "keywords": [format!("boiler{n}")]})
        }
        "library_search" => json!({"query": "anything", "space": space}),
        "library_raise" => {
            json!({"space": space, "path": "/nowhere/a.md", "summary": "A summary."})
        }
        "folder_get" => json!({"space": space}),
        "folder_set" => json!({"space": space, "path": null}),
        "links" => json!({"space": space}),
        "file_put" => {
            json!({"space": space, "name": "a.txt", "data_base64": "aGVsbG8=", "tags": ["sources/app"]})
        }
        "file_list" => json!({"space": space}),
        "file_get" | "file_retire" | "file_erase" => {
            json!({"id": "unknown", "reason": "x", "transcript": fact})
        }
        "guide_set" | "guide_propose" => json!({"space": space, "text": "Be brief."}),
        other => panic!("no arguments for {other} {}", w.t.path().display()),
    }
}

#[test]
fn every_kind_of_connection_is_shown_exactly_its_tools() -> Res {
    let w = world()?;
    for (kind, c) in kinds(&w)? {
        let defs: Vec<&str> = tools::definitions(&c).iter().map(|d| d.name).collect();
        let want = shown(kind);
        for tool in every_tool() {
            let listed = defs.contains(&tool);
            assert_eq!(
                listed,
                want.contains(&tool),
                "{kind}: {tool} listed={listed}"
            );
            // What is not listed is not callable, whatever the arguments, and says the same thing as a tool that does not exist.
            if !listed {
                let out = run(&w, &c, tool, args_for(&w, tool, "work", &w.work_fact));
                assert_eq!(out, Out::Hidden, "{kind}: {tool} is hidden");
            }
        }
        assert!(
            matches!(
                tools::call(&w.brain, &c, "no_such_tool", &json!({})),
                Err(ToolError::Unknown)
            ),
            "{kind}: a tool that does not exist"
        );
    }
    Ok(())
}

#[test]
fn a_listed_tool_is_never_hidden_when_called_on_a_space_the_connection_has() -> Res {
    let w = world()?;
    for (kind, c) in kinds(&w)? {
        for tool in shown(kind) {
            let args = args_for(&w, tool, "work", &w.work_fact);
            let out = run(&w, &c, tool, args);
            assert_ne!(out, Out::Hidden, "{kind}: {tool}");
        }
    }
    Ok(())
}

#[test]
fn reads_answer_on_a_granted_space_and_refuse_every_other_in_the_same_words() -> Res {
    let w = world()?;
    let read_with_space = [
        "recall",
        "living_get",
        "rules_get",
        "library_search",
        "folder_get",
        "links",
        "file_list",
        "tags_suggest",
    ];
    for (kind, c) in kinds(&w)? {
        for tool in read_with_space.iter().filter(|t| shown(kind).contains(t)) {
            let granted = tools::call(
                &w.brain,
                &c,
                tool,
                &args_for(&w, tool, "work", &w.work_fact),
            );
            let scoped = kind.starts_with("path") || kind == "guide-path";
            if scoped {
                // A vault reached by paths answers `recall` (inside the paths) and nothing else of this list.
                assert!(
                    *tool == "recall" && granted.is_ok(),
                    "{kind}: {tool}: {granted:?}"
                );
                continue;
            }
            // Spaces that are forbidden and spaces that do not exist get one answer.
            if matches!(kind, "all-rw" | "all-rw-afar" | "peer" | "forced-read-only") {
                // These reach `health` too: the question is only about a space that does not exist.
                let ghost = tools::call(
                    &w.brain,
                    &c,
                    tool,
                    &args_for(&w, tool, "nowhere", &w.work_fact),
                );
                assert!(
                    ghost.is_err(),
                    "{kind}: {tool} on a space that does not exist"
                );
                continue;
            }
            assert!(granted.is_ok(), "{kind}: {tool} on work: {granted:?}");
            let denied = tools::call(
                &w.brain,
                &c,
                tool,
                &args_for(&w, tool, "health", &w.health_fact),
            );
            let ghost = tools::call(
                &w.brain,
                &c,
                tool,
                &args_for(&w, tool, "nowhere", &w.health_fact),
            );
            let (d, g) = (
                format!("{:?}", denied.as_ref().err()).replace("health", "X"),
                format!("{:?}", ghost.as_ref().err()).replace("nowhere", "X"),
            );
            assert!(denied.is_err() && ghost.is_err(), "{kind}: {tool}");
            assert_eq!(
                d, g,
                "{kind}: {tool}: a forbidden space and an unknown one answer alike"
            );
        }
    }
    Ok(())
}

#[test]
fn a_fact_of_a_space_the_connection_does_not_reach_is_unknown_to_it() -> Res {
    let w = world()?;
    for (kind, c) in kinds(&w)? {
        if matches!(kind, "all-rw" | "all-rw-afar" | "peer" | "forced-read-only") {
            continue;
        }
        for tool in ["read_fact", "history"] {
            if !shown(kind).contains(&tool) {
                continue;
            }
            let r = tools::call(&w.brain, &c, tool, &json!({"id": w.health_fact}));
            let ghost = tools::call(&w.brain, &c, tool, &json!({"id": "f_does_not_exist"}));
            assert_eq!(
                format!("{:?}", r.err()),
                format!("{:?}", ghost.err()),
                "{kind}: {tool}"
            );
        }
        for tool in ["supersede", "retire", "tags_set"] {
            if shown(kind).contains(&tool) {
                let r = run(&w, &c, tool, args_for(&w, tool, "health", &w.health_fact));
                assert_eq!(r, Out::Refused, "{kind}: {tool} on a fact of health");
            }
        }
    }
    Ok(())
}

#[test]
fn writes_succeed_where_the_connection_writes_and_nowhere_else() -> Res {
    let w = world()?;
    let writes = ["remember", "deposit", "file_put", "living_set", "rules_set"];
    for (kind, c) in kinds(&w)? {
        for tool in writes.iter().filter(|t| shown(kind).contains(t)) {
            let here = run(&w, &c, tool, args_for(&w, tool, "work", &w.work_fact));
            let writes_work = matches!(
                kind,
                "one-rw" | "all-rw" | "all-rw-afar" | "peer" | "path-rw" | "guide-path"
            );
            // A path key writes under its path only: `rules_set` and `living_set` are not among its tools.
            assert_eq!(
                here == Out::Ok,
                writes_work,
                "{kind}: {tool} on work: {here:?}"
            );
            if matches!(kind, "one-rw") {
                let there = run(&w, &c, tool, args_for(&w, tool, "health", &w.health_fact));
                assert_eq!(there, Out::Refused, "{kind}: {tool} on health");
            }
        }
    }
    // The refused writes left health alone: its only note is the one put there at the start.
    let conn = w.brain.open_space(&sp("health"))?;
    let n: i64 = conn.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    let from_all: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE author NOT IN ('setup')",
        [],
        |r| r.get(0),
    )?;
    // Only the connections that write `health` (all-rw, peer) may have added to it.
    assert!(n >= 1);
    assert!(
        conn.query_row(
            "SELECT count(*) FROM facts WHERE author IN ('k-one-rw','k-one-r','k-path-rw','k-path-r','k-guide','k-forced')",
            [],
            |r| r.get::<_, i64>(0)
        )? == 0,
        "no refused connection left anything in health ({from_all} notes from the others)"
    );
    Ok(())
}

#[test]
fn a_connection_that_reads_only_cannot_change_anything() -> Res {
    let w = world()?;
    for (kind, c) in kinds(&w)? {
        if !matches!(kind, "one-r" | "forced-read-only" | "path-r") {
            continue;
        }
        for tool in WRITE.iter().chain(&SORT).chain(&GUIDE) {
            let out = run(&w, &c, tool, args_for(&w, tool, "work", &w.work_fact));
            assert_eq!(out, Out::Hidden, "{kind}: {tool}");
        }
    }
    // Nothing moved.
    let conn = w.brain.open_space(&sp("work"))?;
    let active: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(active, 2);
    Ok(())
}

#[test]
fn a_connection_limited_to_a_path_sees_and_changes_only_that_path() -> Res {
    let w = world()?;
    let c = caller(&w, "k-path", "work/sources/app:rw", false)?;
    // Reads: only what is filed under its path.
    let r = tools::call(
        &w.brain,
        &c,
        "recall",
        &json!({"query": "review", "space": "work"}),
    )?;
    assert_eq!(
        r["results"].as_array().map(Vec::len),
        Some(0),
        "the review note is outside its path"
    );
    let r = tools::call(
        &w.brain,
        &c,
        "recall",
        &json!({"query": "build", "space": "work"}),
    )?;
    assert_eq!(r["results"].as_array().map(Vec::len), Some(1));
    // Writes outside the path are refused; with no tag, the note goes under the one write path.
    let outside = tools::call(
        &w.brain,
        &c,
        "remember",
        &json!({"space": "work", "text": "A note outside the path.", "tags": ["journal"]}),
    );
    assert!(matches!(outside, Err(ToolError::Denied(_))), "{outside:?}");
    let inside = tools::call(
        &w.brain,
        &c,
        "remember",
        &json!({"space": "work", "text": "A note with no tag at all."}),
    )?;
    assert_eq!(inside["fact"]["tags"], json!(["sources/app"]));
    // A fact of the vault outside its path cannot be retired, and is not said to exist.
    let out = tools::call(
        &w.brain,
        &c,
        "retire",
        &json!({"id": w.work_fact, "reason": "x"}),
    );
    assert!(
        matches!(&out, Err(ToolError::Invalid(m)) if m == "unknown fact"),
        "{out:?}"
    );
    // One inside can be.
    tools::call(
        &w.brain,
        &c,
        "retire",
        &json!({"id": w.app_fact, "reason": "x"}),
    )?;
    // Another vault is refused outright.
    let other = tools::call(
        &w.brain,
        &c,
        "remember",
        &json!({"space": "health", "text": "Not here.", "tags": ["sources/app"]}),
    );
    assert!(matches!(other, Err(ToolError::Denied(_))), "{other:?}");
    // Stats show no counts for the vault, only that it is limited.
    let s = tools::call(&w.brain, &c, "stats", &json!({}))?;
    assert!(s.to_string().contains("restricted_to"), "{s}");
    Ok(())
}

#[test]
fn the_tools_added_late_follow_the_same_rules() -> Res {
    let w = world()?;
    let all = caller(&w, "late-all", "all", false)?;
    let one = caller(&w, "late-one", "work:rw", false)?;
    let ro = caller(&w, "late-ro", "work:r", false)?;
    // spaces: only what is readable; `writable` says what a write would do.
    let s = tools::call(&w.brain, &ro, "spaces", &json!({}))?;
    let names: Vec<&str> = s["spaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert_eq!(names, ["work"]);
    assert_eq!(s["spaces"][0]["writable"], json!(false));
    let s = tools::call(&w.brain, &one, "spaces", &json!({}))?;
    assert_eq!(s["spaces"][0]["writable"], json!(true));
    let s = tools::call(&w.brain, &all, "spaces", &json!({}))?;
    assert!(s["spaces"].as_array().is_some_and(|a| a.len() >= 5));
    // rules_get / rules_set: write needs the write right, read needs the read right.
    assert!(
        tools::call(
            &w.brain,
            &one,
            "rules_set",
            &json!({"space": "work", "keywords": ["x"]})
        )
        .is_ok()
    );
    assert!(matches!(
        tools::call(
            &w.brain,
            &one,
            "rules_set",
            &json!({"space": "health", "keywords": ["x"]})
        ),
        Err(ToolError::Invalid(_))
    ));
    assert!(matches!(
        tools::call(&w.brain, &one, "rules_get", &json!({"space": "health"})),
        Err(ToolError::Invalid(_))
    ));
    assert_eq!(
        tools::call(&w.brain, &ro, "rules_get", &json!({"space": "work"}))?["rules"]["keywords"],
        json!(["x"])
    );
    // living_get / living_set likewise.
    assert!(
        tools::call(
            &w.brain,
            &ro,
            "living_get",
            &json!({"space": "work", "page": "next"})
        )
        .is_ok()
    );
    assert!(matches!(
        tools::call(
            &w.brain,
            &ro,
            "living_set",
            &json!({"space": "work", "page": "next"})
        ),
        Err(ToolError::Unknown)
    ));
    assert!(matches!(
        tools::call(
            &w.brain,
            &one,
            "living_get",
            &json!({"space": "health", "page": "next"})
        ),
        Err(ToolError::Invalid(_))
    ));
    // links on a connection that reads one space: that space's tags only.
    let l = tools::call(&w.brain, &ro, "links", &json!({}))?;
    assert!(!l.to_string().contains("space:health"), "{l}");
    // library_raise follows the write right.
    assert!(matches!(
        tools::call(
            &w.brain,
            &one,
            "library_raise",
            &json!({"space": "health", "path": "/x/a.md", "summary": "S."})
        ),
        Err(ToolError::Denied(_))
    ));
    Ok(())
}

#[test]
fn the_sorting_tools_only_touch_the_triage_space_of_a_connection_that_writes_it() -> Res {
    let w = world()?;
    let tri = triage::ensure(&w.brain, OWNER, "owner")?;
    put(
        &w.brain,
        tri.as_str(),
        "A loose idea about the shed roof.",
        &["inbox"],
    )?;
    // Writes work, not Inbox: it claims nothing (and is not an error).
    let one = caller(&w, "sorter-work", "work:rw", false)?;
    let got = tools::call(&w.brain, &one, "triage_claim", &json!({}))?;
    assert_eq!(got["claimed"].as_array().map(Vec::len), Some(0));
    // Reads Inbox only: the sorting tools are not shown.
    let ro = caller(&w, "sorter-ro", &format!("{}:r", tri.as_str()), false)?;
    assert!(matches!(
        tools::call(&w.brain, &ro, "triage_claim", &json!({})),
        Err(ToolError::Unknown)
    ));
    assert!(matches!(
        tools::call(&w.brain, &ro, "triage_release", &json!({})),
        Err(ToolError::Unknown)
    ));
    // Writes Inbox: it claims, and releasing gives it back.
    let rw = caller(&w, "sorter-rw", &format!("{}:rw", tri.as_str()), false)?;
    let got = tools::call(&w.brain, &rw, "triage_claim", &json!({}))?;
    assert_eq!(got["claimed"].as_array().map(Vec::len), Some(1));
    let rel = tools::call(&w.brain, &rw, "triage_release", &json!({}))?;
    assert_eq!(rel["released"], json!(1));
    // Only its own leases are released.
    let other = caller(&w, "sorter-rw2", &format!("{}:rw", tri.as_str()), false)?;
    tools::call(&w.brain, &rw, "triage_claim", &json!({}))?;
    let rel = tools::call(&w.brain, &other, "triage_release", &json!({}))?;
    assert_eq!(rel["released"], json!(0));
    tools::call(&w.brain, &rw, "triage_release", &json!({}))?;
    Ok(())
}

#[test]
fn peers_lists_only_the_peers_of_the_person_and_is_open_to_any_reader() -> Res {
    let w = world()?;
    let ctl = w.brain.open_control()?;
    let other = memcp_core::users::create_user(&ctl, OWNER, "ann", "Ann", false)?.id;
    w.brain.add_vault(&other, "ann", "me")?;
    access::connect_peer(
        &ctl,
        &other,
        "peer-ann-phone-1",
        &Rights::parse("ann-me:rw")?,
        false,
    )?;
    let _peer = caller(&w, "peer-laptop-ab12", "all", false)?;
    let reader = caller(&w, "reader", "work:r", false)?;
    let got = tools::call(&w.brain, &reader, "peers", &json!({}))?;
    let names: Vec<&str> = got["peers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(
        names,
        ["peer-laptop-ab12"],
        "Ann's peer is not listed to the owner's connection"
    );
    assert_eq!(got["peers"][0]["allowed"], json!(true));
    access::revoke(&ctl, "peer-laptop-ab12")?;
    let got = tools::call(&w.brain, &reader, "peers", &json!({}))?;
    assert_eq!(got["peers"][0]["allowed"], json!(false));
    Ok(())
}

#[test]
fn guide_tools_are_given_to_a_whole_vault_writer_or_to_the_path_named_guide_and_to_no_one_else()
-> Res {
    let w = world()?;
    let whole = caller(&w, "g-whole", "work:rw", false)?;
    let ok = tools::call(
        &w.brain,
        &whole,
        "guide_set",
        &json!({"space": "work", "text": "Be brief."}),
    );
    assert!(ok.is_ok(), "{ok:?}");
    let denied = tools::call(
        &w.brain,
        &whole,
        "guide_set",
        &json!({"space": "health", "text": "Be brief."}),
    );
    assert!(matches!(denied, Err(ToolError::Denied(_))), "{denied:?}");
    let app = caller(&w, "g-app", "work/guide:rw,work/sources/app:rw", false)?;
    assert!(
        tools::call(
            &w.brain,
            &app,
            "guide_propose",
            &json!({"space": "work", "text": "Use the app tags."})
        )
        .is_ok()
    );
    assert!(matches!(
        tools::call(
            &w.brain,
            &app,
            "guide_set",
            &json!({"space": "work", "text": "x"})
        ),
        Err(ToolError::Unknown)
    ));
    let only_path = caller(&w, "g-path", "work/sources/app:rw", false)?;
    assert!(matches!(
        tools::call(
            &w.brain,
            &only_path,
            "guide_propose",
            &json!({"space": "work", "text": "x"})
        ),
        Err(ToolError::Unknown)
    ));
    Ok(())
}

#[test]
fn a_revoked_an_expired_or_a_rotated_key_is_refused_without_saying_why() -> Res {
    let w = world()?;
    let ctl = w.brain.open_control()?;
    let rights = Rights::parse("work:rw")?;
    let now = util::now();
    // Revoked.
    let tok = access::connect_for(&ctl, OWNER, "to-revoke", &rights, None, false, false)?;
    assert!(access::authenticate(&ctl, &tok, now)?.is_some());
    assert!(access::revoke(&ctl, "to-revoke")?);
    assert!(access::authenticate(&ctl, &tok, now)?.is_none());
    // Expired: valid for a day, refused two days later.
    let tok = access::connect_for(&ctl, OWNER, "to-expire", &rights, Some(1), false, false)?;
    assert!(access::authenticate(&ctl, &tok, now + 3600)?.is_some());
    assert!(access::authenticate(&ctl, &tok, now + 2 * 86_400)?.is_none());
    // Rotated: the old key stops, the new one works.
    let old = access::connect_for(&ctl, OWNER, "to-rotate", &rights, None, false, false)?;
    let new = access::connect_for(&ctl, OWNER, "to-rotate", &rights, None, false, true)?;
    assert!(access::authenticate(&ctl, &old, now)?.is_none());
    assert!(access::authenticate(&ctl, &new, now)?.is_some());
    // A peer is no different from another connection: revoked, it is refused.
    let tok = access::connect_peer(&ctl, OWNER, "peer-box-0001", &Rights::parse("all")?, false)?;
    assert!(access::revoke(&ctl, "peer-box-0001")?);
    assert!(access::authenticate(&ctl, &tok, now)?.is_none());
    // A key that never existed.
    assert!(access::authenticate(&ctl, "hcp_0000_nothing", now)?.is_none());
    Ok(())
}

#[test]
fn rights_never_go_beyond_the_person_and_a_regrant_takes_effect_at_once() -> Res {
    let w = world()?;
    let ctl = w.brain.open_control()?;
    let ann = memcp_core::users::create_user(&ctl, OWNER, "ann", "Ann", false)?.id;
    w.brain.add_vault(&ann, "ann", "me")?;
    // Ann asks for `work` (Owner's space) and for her own: only her own is reached.
    let tok = access::connect_for(
        &ctl,
        &ann,
        "ann-key",
        &Rights::parse("ann-me:rw,work:rw")?,
        None,
        false,
        false,
    );
    if let Ok(tok) = tok {
        let c = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
        assert!(c.can_write(&sp("ann-me")));
        assert!(!c.can_read(&sp("work")));
        assert!(!c.can_write(&sp("work")));
        assert_eq!(
            run(
                &w,
                &c,
                "remember",
                json!({"space": "work", "text": "Ann writes into the owner's space."})
            ),
            Out::Refused
        );
    }
    // Narrowing a connection takes effect on its next call.
    let tok = access::connect_for(
        &ctl,
        OWNER,
        "narrow",
        &Rights::parse("work:rw,health:rw")?,
        None,
        false,
        false,
    )?;
    let before = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
    assert!(before.can_write(&sp("health")));
    access::regrant(&ctl, "narrow", &Rights::parse("work:r")?)?;
    let after = access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?;
    assert!(!after.can_write(&sp("work")) && !after.can_read(&sp("health")));
    Ok(())
}
