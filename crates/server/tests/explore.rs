//! Exploring a space on the page: the overview and its dashboard, the notes, the edit of a note,
//! and the layout of spaces. Only the signed-in owner, only their own spaces.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use memcp_core::text::Language;
use memcp_core::users;
use memcp_core::{Brain, Space};
use memcp_server::app::App;
use memcp_server::http::{Request, Response};
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);

fn sp(n: &str) -> Space {
    match Space::new(n) {
        Ok(s) => s,
        Err(e) => panic!("{n}: {e}"),
    }
}

struct Env {
    dir: PathBuf,
    app: App,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const SAM: (&str, &str) = ("ha-sam", "Sam");
const ALEX: (&str, &str) = ("ha-alex", "Alex");

fn env() -> Result<Env, Box<dyn Error>> {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-explore-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[], Language::English)?;
    let mut app = App::new(brain);
    app.ingress_admins = vec!["Sam".into()];
    app.public_url = "https://memory.example.net".into();
    Ok(Env { dir, app })
}

fn call(
    e: &Env,
    method: &str,
    path: &str,
    query: &[(&str, &str)],
    who: (&str, &str),
    body: Value,
) -> Response {
    let mut headers = vec![
        ("x-remote-user-id".to_string(), who.0.to_string()),
        ("x-remote-user-name".to_string(), who.1.to_string()),
        ("x-remote-user-display-name".to_string(), who.1.to_string()),
    ];
    if method == "POST" {
        headers.push(("x-requested-with".into(), "memcp".into()));
    }
    e.app.handle(&Request {
        method: method.into(),
        path: path.into(),
        query: query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        headers: headers.into_iter().collect(),
        body: if method == "POST" {
            body.to_string().into_bytes()
        } else {
            vec![]
        },
        ip: Some("172.30.32.2".into()),
        via_ingress: true,
        tailnet_login: None,
    })
}

fn get(e: &Env, path: &str, query: &[(&str, &str)], who: (&str, &str)) -> Response {
    call(
        e,
        "GET",
        &format!("/admin/api/{path}"),
        query,
        who,
        json!({}),
    )
}

fn post(e: &Env, path: &str, who: (&str, &str), body: Value) -> Response {
    call(e, "POST", &format!("/admin/api/{path}"), &[], who, body)
}

/// Makes a person at their first sign-in and gives them a space of their own.
fn person_with(e: &Env, who: (&str, &str), space: &str) -> Result<(), Box<dyn Error>> {
    let _ = get(e, "state", &[], who);
    if who != SAM {
        let r = post(e, "requests/approve", SAM, json!({ "ha_user": who.0 }));
        assert_eq!(r.status, 200, "{}", r.body_json());
    }
    let ctl = e.app.brain.open_control()?;
    let p = users::list_users(&ctl)?
        .into_iter()
        .find(|u| u.ha_user.as_deref() == Some(who.0))
        .ok_or("no such person")?;
    e.app.brain.add_space_for(&sp(space), &p.id)?;
    Ok(())
}

#[test]
fn an_owner_reads_a_space_and_nobody_else_does() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    // Alex joins later and has a space of their own.
    let _ = get(&e, "state", &[], ALEX);
    person_with(&e, ALEX, "private")?;
    let r = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": "notes", "text": "The boiler is serviced in March.", "tags": ["home/boiler"] }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    let ov = get(&e, "space", &[("space", "notes")], SAM);
    assert_eq!(ov.status, 200, "{}", ov.body_json());
    let v = ov.body_json();
    assert_eq!(v["space"], "notes");
    assert_eq!(v["counts"]["active"], 1);
    assert!(
        v["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x["tag"] == "home")),
        "the tree of tags has its top level: {v}"
    );
    assert_eq!(v["dashboard"]["window_days"], 30);
    // Alex cannot see Sam's space, nor read, nor write into it.
    assert_eq!(get(&e, "space", &[("space", "notes")], ALEX).status, 404);
    assert_eq!(get(&e, "facts", &[("space", "notes")], ALEX).status, 404);
    let w = post(
        &e,
        "fact/add",
        ALEX,
        json!({ "space": "notes", "text": "Not allowed." }),
    );
    assert_eq!(w.status, 404);
    Ok(())
}

#[test]
fn a_note_is_listed_searched_corrected_and_removed_with_its_history() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let add = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": "notes", "text": "The meeting is on Tuesday.", "tags": ["work/planning"] }),
    );
    let id = add.body_json()["fact"]["id"]
        .as_str()
        .ok_or("no id")?
        .to_string();
    // By tag, newest first; and by words.
    let by_tag = get(&e, "facts", &[("space", "notes"), ("tag", "work")], SAM).body_json();
    assert_eq!(by_tag["facts"].as_array().map(Vec::len), Some(1));
    let found = get(&e, "facts", &[("space", "notes"), ("q", "meeting")], SAM).body_json();
    assert_eq!(found["facts"].as_array().map(Vec::len), Some(1));
    // A correction keeps the old text in the history.
    let fix = post(
        &e,
        "fact/supersede",
        SAM,
        json!({ "space": "notes", "id": id, "text": "The meeting is on Wednesday.",
                "tags": ["work/planning"], "reason": "date moved" }),
    );
    assert_eq!(fix.status, 200, "{}", fix.body_json());
    let new_id = fix.body_json()["fact"]["id"]
        .as_str()
        .ok_or("no new id")?
        .to_string();
    let one = get(&e, "fact", &[("space", "notes"), ("id", &new_id)], SAM).body_json();
    assert_eq!(one["fact"]["text"], "The meeting is on Wednesday.");
    let old = get(&e, "fact", &[("space", "notes"), ("id", &id)], SAM).body_json();
    assert_eq!(old["fact"]["status"], "replaced");
    // A removal needs its reason.
    let no_reason = post(
        &e,
        "fact/retire",
        SAM,
        json!({ "space": "notes", "id": new_id }),
    );
    assert_eq!(no_reason.status, 400);
    let gone = post(
        &e,
        "fact/retire",
        SAM,
        json!({ "space": "notes", "id": new_id, "reason": "cancelled" }),
    );
    assert_eq!(gone.status, 200, "{}", gone.body_json());
    let after = get(&e, "facts", &[("space", "notes")], SAM).body_json();
    assert_eq!(after["facts"].as_array().map(Vec::len), Some(0));
    Ok(())
}

#[test]
fn a_write_without_the_header_is_refused() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let r = e.app.handle(&Request {
        method: "POST".into(),
        path: "/admin/api/fact/add".into(),
        query: Default::default(),
        headers: [
            ("x-remote-user-id".to_string(), SAM.0.to_string()),
            ("x-remote-user-name".to_string(), SAM.1.to_string()),
        ]
        .into_iter()
        .collect(),
        body: json!({ "space": "notes", "text": "x" })
            .to_string()
            .into_bytes(),
        ip: Some("172.30.32.2".into()),
        via_ingress: true,
        tailnet_login: None,
    });
    assert_eq!(r.status, 403);
    Ok(())
}

#[test]
fn the_layout_is_read_changed_and_a_space_is_made_with_a_label() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let made = post(
        &e,
        "layout/add",
        SAM,
        json!({ "label": "Work journal", "parent": "notes" }),
    );
    assert_eq!(made.status, 200, "{}", made.body_json());
    let name = made.body_json()["space"]
        .as_str()
        .ok_or("no space")?
        .to_string();
    let set = post(
        &e,
        "layout/set",
        SAM,
        json!({ "space": name, "label": "Pro journal", "position": 3 }),
    );
    assert_eq!(set.status, 200, "{}", set.body_json());
    let l = get(&e, "layout", &[], SAM).body_json();
    let row = l["spaces"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["space"] == name.as_str()))
        .ok_or("not listed")?;
    assert_eq!(row["label"], "Pro journal");
    assert_eq!(row["parent"], "notes");
    // Back to the top level with an empty parent.
    let up = post(
        &e,
        "layout/set",
        SAM,
        json!({ "space": name, "parent": null }),
    );
    assert_eq!(up.status, 200, "{}", up.body_json());
    // A cycle is refused.
    let cycle = post(
        &e,
        "layout/set",
        SAM,
        json!({ "space": "notes", "parent": name }),
    );
    assert_eq!(
        cycle.status, 200,
        "notes may go under it now that it is at the top"
    );
    let back = post(
        &e,
        "layout/set",
        SAM,
        json!({ "space": name, "parent": "notes" }),
    );
    assert_eq!(back.status, 400, "a space cannot go under its own child");
    Ok(())
}

#[test]
fn a_health_space_counts_its_signs_and_lists_its_targets() -> Res {
    let e = env()?;
    let _ = get(&e, "state", &[], SAM);
    let ctl = e.app.brain.open_control()?;
    let me = users::list_users(&ctl)?
        .into_iter()
        .find(|u| u.ha_user.as_deref() == Some(SAM.0))
        .ok_or("no person")?;
    let health = e.app.brain.add_vault(&me.id, &me.name, "health")?;
    let today = memcp_core::util::iso(memcp_core::util::now());
    let day = &today[..10];
    let entry = format!(
        "[App] day {day}: {}",
        json!({ "date": day, "notes": [
            { "manifestations": [ { "code": "fatigue" }, { "code": "anxiety" } ] },
            { "manifestations": [ { "code": "fatigue" } ] }
        ]})
    );
    let r = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": health.as_str(), "text": entry, "tags": ["dossier/app"] }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    let t = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": health.as_str(), "text": "Sleep: wake at 4 am, follow for a month.",
                "tags": ["cibles/sleep"] }),
    );
    assert_eq!(t.status, 200, "{}", t.body_json());
    let v = get(&e, "space", &[("space", health.as_str())], SAM).body_json();
    assert_eq!(v["kind"], "health");
    let top = &v["dashboard"]["signs"]["top"];
    assert_eq!(top[0]["code"], "fatigue");
    assert_eq!(top[0]["count"], 2);
    assert_eq!(v["dashboard"]["signs"]["days_with_entries"], 1);
    assert_eq!(v["dashboard"]["targets"].as_array().map(Vec::len), Some(1));
    Ok(())
}
