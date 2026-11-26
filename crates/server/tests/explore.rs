//! Exploring a space on the page: the overview and its dashboard, the notes, the edit of a note,
//! and the layout of spaces. Only the signed-in owner, only their own spaces.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use pensine_core::text::Language;
use pensine_core::users;
use pensine_core::{Brain, Space};
use pensine_server::app::App;
use pensine_server::http::{Request, Response};
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
    assert_eq!(v["dashboard"]["source"], "default");
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
    let refused = post(
        &e,
        "layout/set",
        SAM,
        json!({ "space": name, "label": " NÓTES ", "parent": null }),
    );
    assert_eq!(refused.status, 400, "{}", refused.body_json());
    assert_eq!(
        refused.body_json()["error"],
        "A space called NÓTES already exists."
    );
    let unchanged = get(&e, "layout", &[], SAM).body_json();
    assert_eq!(unchanged, l, "the move and label change were both refused");
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
fn a_space_shows_the_notes_day_by_day_until_it_has_a_dashboard() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let v = get(&e, "space", &[("space", "notes")], SAM).body_json();
    assert_eq!(v["dashboard"]["source"], "default");
    assert_eq!(v["dashboard"]["sections"][0]["type"], "daily");
    Ok(())
}

#[test]
fn a_stored_dashboard_counts_the_signs_and_lists_the_targets() -> Res {
    let e = env()?;
    let _ = get(&e, "state", &[], SAM);
    let ctl = e.app.brain.open_control()?;
    let me = users::list_users(&ctl)?
        .into_iter()
        .find(|u| u.ha_user.as_deref() == Some(SAM.0))
        .ok_or("no person")?;
    let health = e.app.brain.add_vault(&me.id, &me.name, "health")?;
    let today = pensine_core::util::iso(pensine_core::util::now());
    let day = &today[..10];
    let entry = format!(
        "[App] day {day}: {}",
        json!({ "date": day, "notes": [
            { "manifestations": [ { "code": "fatigue" }, { "code": "anxiety" } ] },
            { "manifestations": [ { "code": "fatigue" } ] }
        ]})
    );
    let add = |text: String, tag: &str| {
        let r = post(
            &e,
            "fact/add",
            SAM,
            json!({ "space": health.as_str(), "text": text, "tags": [tag] }),
        );
        assert_eq!(r.status, 200, "{}", r.body_json());
    };
    add(entry, "dossier/app");
    add(
        "Sleep: wake at 4 am, follow for a month.".into(),
        "cibles/sleep",
    );
    // Without a document, only the plain default.
    let v = get(&e, "space", &[("space", health.as_str())], SAM).body_json();
    assert_eq!(v["dashboard"]["source"], "default");
    // With one, each section is computed from the notes; a section the catalogue does not know is an error.
    add(
        json!({ "version": 1, "title": "Health", "sections": [
            { "type": "counts", "from": "journal-manifestations", "window_days": 30, "top": 5 },
            { "type": "targets", "tag": "cibles" },
            { "type": "text", "body": "A line for myself." },
            { "type": "script", "body": "alert(1)" }
        ]})
        .to_string(),
        "dashboard",
    );
    let v = get(&e, "space", &[("space", health.as_str())], SAM).body_json();
    let d = &v["dashboard"];
    assert_eq!(d["source"], "stored");
    assert_eq!(d["title"], "Health");
    assert_eq!(d["sections"][0]["top"][0]["code"], "fatigue");
    assert_eq!(d["sections"][0]["top"][0]["count"], 2);
    assert_eq!(d["sections"][0]["days_with_entries"], 1);
    assert_eq!(d["sections"][1]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(d["sections"][2]["body"], "A line for myself.");
    assert_eq!(d["sections"][3]["error"], "unknown section type");
    Ok(())
}

#[test]
fn any_space_can_count_what_is_cited_in_it() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let add = |text: &str, tag: &str| {
        let r = post(
            &e,
            "fact/add",
            SAM,
            json!({ "space": "notes", "text": text, "tags": [tag] }),
        );
        assert_eq!(r.status, 200, "{}", r.body_json());
    };
    add(
        "Call with the Alpha team about the schedule.",
        "work/projects/alpha",
    );
    add(
        "Alpha budget approved, Beta still waiting.",
        "work/projects/alpha",
    );
    add("Beta needs a review next week.", "work/projects/beta");
    add(
        &json!({ "version": 1, "sections": [
            { "type": "counts", "from": "keywords", "terms": ["Alpha", "beta", "gamma"], "window_days": 30 },
            { "type": "counts", "from": "tags", "tag": "work/projects", "window_days": 30 }
        ]})
        .to_string(),
        "dashboard",
    );
    let v = get(&e, "space", &[("space", "notes")], SAM).body_json();
    let by_terms = &v["dashboard"]["sections"][0]["top"];
    assert_eq!(by_terms[0]["code"], "alpha");
    assert_eq!(by_terms[0]["count"], 2);
    assert_eq!(by_terms[1]["code"], "beta");
    assert_eq!(by_terms[1]["count"], 2);
    assert_eq!(
        by_terms.as_array().map(Vec::len),
        Some(2),
        "gamma is never cited"
    );
    let by_tags = &v["dashboard"]["sections"][1]["top"];
    assert_eq!(by_tags[0]["code"], "alpha");
    assert_eq!(by_tags[0]["count"], 2);
    assert_eq!(by_tags[1]["code"], "beta");
    assert_eq!(by_tags[1]["count"], 1);
    Ok(())
}

#[test]
fn an_attachment_and_a_voice_note_show_up_as_notes_with_their_file() -> Res {
    use base64::Engine as _;
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let r = post(
        &e,
        "files",
        SAM,
        json!({ "space": "notes", "name": "minutes.txt", "note": "Minutes of the meeting",
                "tags": ["work/meetings"], "data": b64(b"Decisions: ship on Friday.") }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    // What a browser records: WebM with Opus, named as a recording, filed to be transcribed.
    let webm = [
        0x1A, 0x45, 0xDF, 0xA3, 1, 0, 0, 0, 0, 0, 0, 0x1F, 0x42, 0x86, 0x81, 1,
    ];
    let v = post(
        &e,
        "files",
        SAM,
        json!({ "space": "notes", "name": "dictation-2026-11-16-18h30.webm",
                "note": "Voice note recorded on the page", "tags": ["inbox/audio"], "data": b64(&webm) }),
    );
    assert_eq!(v.status, 200, "{}", v.body_json());
    assert_eq!(v.body_json()["file"]["mime"], "audio/webm");
    let all = get(&e, "facts", &[("space", "notes")], SAM).body_json();
    let rows = all["facts"].as_array().ok_or("no facts")?;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|f| f["file"]["id"].is_string()),
        "each note carries its file: {all}"
    );
    let audio = get(
        &e,
        "facts",
        &[("space", "notes"), ("tag", "inbox/audio")],
        SAM,
    )
    .body_json();
    assert_eq!(audio["facts"][0]["file"]["mime"], "audio/webm");
    let fid = audio["facts"][0]["file"]["id"].as_str().ok_or("no id")?;
    let bytes = get(&e, "file", &[("space", "notes"), ("id", fid)], SAM);
    assert_eq!(bytes.status, 200);
    assert_eq!(bytes.body, webm.to_vec());
    // Another person cannot read it.
    person_with(&e, ALEX, "private")?;
    assert_eq!(
        get(&e, "file", &[("space", "notes"), ("id", fid)], ALEX).status,
        404
    );
    Ok(())
}

#[test]
fn a_voice_note_shows_whether_it_waits_was_read_or_failed() -> Res {
    use base64::Engine as _;
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let webm = [
        0x1A, 0x45, 0xDF, 0xA3, 1, 0, 0, 0, 0, 0, 0, 0x1F, 0x42, 0x86, 0x81, 1,
    ];
    let v = post(
        &e,
        "files",
        SAM,
        json!({ "space": "notes", "name": "dictation-2026-11-16-18h30.webm", "note": "Voice note",
                "tags": ["inbox/audio"], "data": base64::engine::general_purpose::STANDARD.encode(webm) }),
    );
    assert_eq!(v.status, 200, "{}", v.body_json());
    let id = v.body_json()["file"]["id"]
        .as_str()
        .ok_or("id")?
        .to_string();
    let state = |e: &Env| -> Result<Value, Box<dyn std::error::Error>> {
        let all = get(
            e,
            "facts",
            &[("space", "notes"), ("tag", "inbox/audio")],
            SAM,
        )
        .body_json();
        Ok(all["facts"]
            .as_array()
            .ok_or("facts")?
            .iter()
            .find(|f| f["file"]["id"].is_string())
            .ok_or("no recording")?["voice"]
            .clone())
    };
    assert_eq!(state(&e)?["state"], "waiting");
    let m = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": "notes", "text": format!("The recording {id} could not be transcribed after 3 tries: broken"),
                "tags": ["inbox/audio/failed"] }),
    );
    assert_eq!(m.status, 200, "{}", m.body_json());
    let failed = state(&e)?;
    assert_eq!(failed["state"], "failed", "{failed}");
    let marker = failed["marker"].as_str().ok_or("marker")?.to_string();
    // Retry: the page retires the failure note, and the recording waits again.
    let r = post(
        &e,
        "fact/retire",
        SAM,
        json!({ "space": "notes", "id": marker, "reason": "retry" }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    assert_eq!(state(&e)?["state"], "waiting");
    let t = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": "notes", "text": format!("Dictée du 2026-11-16, audio dictation-2026-11-16-18h30.webm ({id}) : hello"),
                "tags": ["inbox", "dictee"] }),
    );
    assert_eq!(t.status, 200, "{}", t.body_json());
    assert_eq!(state(&e)?["state"], "transcribed");
    Ok(())
}

#[test]
fn joining_a_network_gives_a_link_and_a_qr_code_to_scan() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let r = post(&e, "pairing", SAM, json!({}));
    assert_eq!(r.status, 200, "{}", r.body_json());
    let v = r.body_json();
    let code = v["code"].as_str().ok_or("no code")?;
    let link = v["link"].as_str().ok_or("no link")?;
    assert!(link.starts_with("memcp://join?server="), "{link}");
    assert!(link.ends_with(&format!("&code={code}")), "{link}");
    assert!(
        v["qr"].as_str().is_some_and(|s| s.contains("<svg")),
        "an image the page draws as it is"
    );
    Ok(())
}

#[test]
fn a_tag_dragged_onto_a_sub_space_moves_its_notes_after_a_preview_and_files_there_from_now_on()
-> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    let child = post(
        &e,
        "layout/add",
        SAM,
        json!({ "label": "Cars", "parent": "notes" }),
    )
    .body_json()["space"]
        .as_str()
        .ok_or("no child")?
        .to_string();
    for (t, tag) in [
        ("The ID4 charges at 11 kW.", "home/voitures/id4"),
        ("Winter tyres in November.", "home/voitures"),
        ("The boiler is serviced in March.", "home/chauffage"),
    ] {
        let r = post(
            &e,
            "fact/add",
            SAM,
            json!({ "space": "notes", "text": t, "tags": [tag] }),
        );
        assert_eq!(r.status, 200, "{}", r.body_json());
    }
    let mv = |dry: bool| {
        post(
            &e,
            "space/move-tag",
            SAM,
            json!({ "from": "notes", "to": child, "tag": "home/voitures", "dry_run": dry }),
        )
    };
    // The preview changes nothing.
    let pre = mv(true).body_json();
    assert_eq!(
        (pre["matched"].as_i64(), pre["moved"].as_i64()),
        (Some(2), Some(0)),
        "{pre}"
    );
    assert_eq!(
        get(&e, "facts", &[("space", child.as_str())], SAM).body_json()["facts"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    // The move does it, and the target now files that tag.
    let done = mv(false).body_json();
    assert_eq!(done["moved"], 2, "{done}");
    let target = get(&e, "facts", &[("space", child.as_str())], SAM).body_json();
    assert_eq!(
        target["facts"].as_array().map(Vec::len),
        Some(2),
        "the settings note is not listed among the notes: {target}"
    );
    let ov = get(&e, "space", &[("space", child.as_str())], SAM).body_json();
    assert_eq!(ov["rules"]["tags"][0], "home/voitures");
    let left = get(&e, "facts", &[("space", "notes")], SAM).body_json();
    assert_eq!(
        left["facts"].as_array().map(Vec::len),
        Some(1),
        "the boiler stays"
    );
    // Keywords are set apart from tags, and cleaned.
    let r = post(
        &e,
        "space/rules",
        SAM,
        json!({ "space": child, "keywords": ["#Voiture", " id4 ", "id4"] }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    let ov = get(&e, "space", &[("space", child.as_str())], SAM).body_json();
    assert_eq!(ov["rules"]["keywords"], json!(["id4", "voiture"]));
    assert_eq!(
        ov["rules"]["tags"][0], "home/voitures",
        "tags are kept when only keywords change"
    );
    // A note cannot move to the space it is in, nor to someone else's.
    let same = post(
        &e,
        "space/move-tag",
        SAM,
        json!({ "from": "notes", "to": "notes", "tag": "x" }),
    );
    assert_eq!(same.status, 400);
    Ok(())
}

/// Gives the person a second space.
fn second_space(e: &Env, who: (&str, &str), space: &str) -> Result<(), Box<dyn Error>> {
    let ctl = e.app.brain.open_control()?;
    let p = users::list_users(&ctl)?
        .into_iter()
        .find(|u| u.ha_user.as_deref() == Some(who.0))
        .ok_or("no such person")?;
    e.app.brain.add_space_for(&sp(space), &p.id)?;
    Ok(())
}

#[test]
fn a_note_written_on_the_page_is_tagged_and_the_journal_shows_its_tags_and_fields() -> Res {
    let e = env()?;
    person_with(&e, SAM, "health")?;
    let r = post(
        &e,
        "fact/add",
        SAM,
        json!({ "space": "health", "text": "Douleur 2/10 ce matin, tension 12/8", "tags": ["journal"] }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    let fact = &r.body_json()["fact"];
    let tags: Vec<&str> = fact["tags"]
        .as_array()
        .ok_or("tags")?
        .iter()
        .filter_map(|t| t.as_str())
        .collect();
    for t in ["journal", "health", "signe/douleur", "mesure/tension"] {
        assert!(tags.contains(&t), "{t}: {tags:?}");
    }
    assert_eq!(fact["fields"]["level"], "2/10");
    let all = get(&e, "journal", &[], SAM).body_json();
    let row = &all["facts"][0];
    assert!(row["tags"].to_string().contains("signe/douleur"), "{all}");
    assert_eq!(row["fields"]["tension"], "12/8");
    // The tag cloud shows the tags, not the plumbing of the fields.
    let cloud = all["tags"].to_string();
    assert!(
        cloud.contains("mesure/tension") && !cloud.contains("field"),
        "{cloud}"
    );
    Ok(())
}

#[test]
fn the_journal_reads_every_space_of_the_person_newest_first_and_nobody_elses() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    second_space(&e, SAM, "garden")?;
    person_with(&e, ALEX, "private")?;
    for (space, text, tag) in [
        ("notes", "The boiler is serviced in March.", "home/boiler"),
        ("garden", "Planted the tomatoes by the wall.", "plants"),
        ("notes", "Call the plumber about the boiler.", "home/boiler"),
    ] {
        let r = post(
            &e,
            "fact/add",
            SAM,
            json!({ "space": space, "text": text, "tags": [tag] }),
        );
        assert_eq!(r.status, 200, "{}", r.body_json());
    }
    post(
        &e,
        "fact/add",
        ALEX,
        json!({ "space": "private", "text": "A secret of Alex.", "tags": ["secret"] }),
    );
    let all = get(&e, "journal", &[], SAM).body_json();
    let facts = all["facts"].as_array().ok_or("no facts")?;
    assert_eq!(facts.len(), 3, "{all}");
    let spaces: Vec<&str> = facts.iter().filter_map(|f| f["space"].as_str()).collect();
    assert!(spaces.contains(&"notes") && spaces.contains(&"garden"));
    assert!(
        !all.to_string().contains("secret"),
        "never another person's"
    );
    let stamps: Vec<i64> = facts
        .iter()
        .filter_map(|f| f["created_at"].as_i64())
        .collect();
    assert!(stamps.windows(2).all(|w| w[0] >= w[1]), "newest first");
    let tags: Vec<&str> = all["tags"]
        .as_array()
        .ok_or("no tags")?
        .iter()
        .filter_map(|t| t["tag"].as_str())
        .collect();
    assert!(tags.contains(&"plants") && tags.contains(&"home/boiler"));
    // Filter by space, by tag (a parent path takes its children), by words.
    let one = get(&e, "journal", &[("space", "garden")], SAM).body_json();
    assert_eq!(one["facts"].as_array().map(Vec::len), Some(1));
    let by_tag = get(&e, "journal", &[("tag", "home")], SAM).body_json();
    assert_eq!(by_tag["facts"].as_array().map(Vec::len), Some(2));
    let found = get(&e, "journal", &[("q", "tomatoes")], SAM).body_json();
    assert_eq!(found["facts"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["facts"][0]["space"], "garden");
    // Another person's space is unknown, not forbidden.
    assert_eq!(get(&e, "journal", &[("space", "private")], SAM).status, 404);
    // Pages continue where the last one stopped, without a repeat or a gap.
    let first = get(&e, "journal", &[("limit", "2")], SAM).body_json();
    assert_eq!(first["more"], true);
    let before = first["next"]["before"].to_string();
    let before_id = first["next"]["before_id"]
        .as_str()
        .ok_or("no cursor")?
        .to_string();
    let second = get(
        &e,
        "journal",
        &[
            ("limit", "2"),
            ("before", &before),
            ("before_id", &before_id),
        ],
        SAM,
    )
    .body_json();
    assert_eq!(second["more"], false);
    let mut ids: Vec<String> = first["facts"]
        .as_array()
        .into_iter()
        .chain(second["facts"].as_array())
        .flatten()
        .filter_map(|f| f["id"].as_str().map(String::from))
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    Ok(())
}

#[test]
fn the_graph_has_spaces_tags_and_keywords_of_the_person_only() -> Res {
    let e = env()?;
    person_with(&e, SAM, "notes")?;
    second_space(&e, SAM, "garden")?;
    person_with(&e, ALEX, "private")?;
    for (space, text, tag) in [
        ("notes", "Boiler service in March.", "home"),
        ("garden", "Tomatoes by the wall.", "home"),
        ("garden", "Water the tomatoes.", "plants"),
    ] {
        post(
            &e,
            "fact/add",
            SAM,
            json!({ "space": space, "text": text, "tags": [tag] }),
        );
    }
    post(
        &e,
        "fact/add",
        ALEX,
        json!({ "space": "private", "text": "Secret.", "tags": ["secret"] }),
    );
    let r = post(
        &e,
        "space/rules",
        SAM,
        json!({ "space": "garden", "keywords": ["tomatoes"] }),
    );
    assert_eq!(r.status, 200, "{}", r.body_json());
    let g = get(&e, "links", &[], SAM).body_json();
    let ids: Vec<&str> = g["nodes"]
        .as_array()
        .ok_or("no nodes")?
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    for want in [
        "space:notes",
        "space:garden",
        "tag:home",
        "tag:plants",
        "keyword:tomatoes",
    ] {
        assert!(ids.contains(&want), "{want} in {ids:?}");
    }
    assert!(
        !ids.iter()
            .any(|i| i.contains("private") || i.contains("secret"))
    );
    assert!(
        !ids.contains(&"tag:rules"),
        "the page's own notes are left out"
    );
    let kw = g["nodes"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n["id"] == "keyword:tomatoes"))
        .ok_or("no keyword")?;
    assert_eq!(kw["count"], 2, "two notes cite it");
    // Every count is the number of notes the journal lists: the rules note is not one.
    let garden = g["nodes"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n["id"] == "space:garden"))
        .ok_or("no space")?;
    assert_eq!(garden["count"], 2);
    let ov = get(&e, "space", &[("space", "garden")], SAM).body_json();
    assert_eq!(ov["counts"]["active"], 2);
    assert!(
        g["edges"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["type"] == "shared_tag"))
    );
    // One space only.
    let one = get(&e, "links", &[("space", "garden")], SAM).body_json();
    assert!(!one.to_string().contains("space:notes"));
    assert_eq!(get(&e, "links", &[("space", "private")], SAM).status, 404);
    Ok(())
}
