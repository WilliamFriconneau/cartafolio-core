//! Create, read, update, delete over `/admin/api`: spaces, nodes, connections, people, notes, vocabularies, libraries.
//! Every destructive route previews with `dry_run`, is audited, and a member cannot touch another person's objects.

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use pensine_core::access::{self, Rights};
use pensine_core::text::Language;
use pensine_core::users;
use pensine_core::{Brain, Space};
use pensine_server::app::App;
use pensine_server::http::{Request, Response};
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;
static N: AtomicU32 = AtomicU32::new(0);

struct Env {
    dir: PathBuf,
    app: App,
    admin: String,
    member: String,
    member_id: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Result<Env, Box<dyn Error>> {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("memcp-crud-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[], Language::English)?;
    let ctl = brain.open_control()?;
    let admin = access::console_token(&ctl, users::OWNER)?;
    let bob = users::create_user(&ctl, users::OWNER, "bob", "Bob", false)?;
    let member = access::console_token(&ctl, &bob.id)?;
    drop(ctl);
    let mut app = App::new(brain);
    app.console_admin = true;
    app.public_url = "https://memory.example.net".into();
    Ok(Env {
        dir,
        app,
        admin,
        member,
        member_id: bob.id,
    })
}

fn call(e: &Env, token: &str, method: &str, path: &str, body: Value) -> Response {
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let query: HashMap<String, String> = q
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut headers: HashMap<String, String> = HashMap::new();
    headers.insert("authorization".into(), format!("Bearer {token}"));
    headers.insert("x-requested-with".into(), "memcp".into());
    e.app.handle(&Request {
        method: method.into(),
        path: format!("/admin/api/{p}"),
        query,
        headers,
        body: if method == "POST" {
            body.to_string().into_bytes()
        } else {
            vec![]
        },
        ip: Some("127.0.0.1".into()),
        via_ingress: false,
        tailnet_login: None,
    })
}

fn post(e: &Env, token: &str, path: &str, body: Value) -> Result<Value, Box<dyn Error>> {
    let r = call(e, token, "POST", path, body);
    if r.status != 200 {
        return Err(format!("{path}: {} {}", r.status, r.body_json()).into());
    }
    Ok(r.body_json())
}

fn get(e: &Env, token: &str, path: &str) -> Result<Value, Box<dyn Error>> {
    let r = call(e, token, "GET", path, json!({}));
    if r.status != 200 {
        return Err(format!("{path}: {} {}", r.status, r.body_json()).into());
    }
    Ok(r.body_json())
}

fn audits(e: &Env, event: &str) -> i64 {
    e.app
        .brain
        .open_control()
        .and_then(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM auth_audit WHERE event = ?1",
                [event],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(-1)
}

#[test]
fn a_space_is_created_read_updated_and_deleted_with_a_preview_and_a_choice() -> Res {
    let e = env()?;
    let t = e.admin.clone();
    let made = post(
        &e,
        &t,
        "spaces/create",
        json!({ "label": "Cars", "keywords": ["garage", "MOT"] }),
    )?;
    let name = made["space"]["space"].as_str().ok_or("name")?.to_string();
    assert_eq!(made["space"]["keywords"], json!(["garage", "mot"]));
    let other = post(
        &e,
        &t,
        "spaces/create",
        json!({ "label": "House", "kind": "home" }),
    )?;
    let other = other["space"]["space"].as_str().ok_or("name")?.to_string();
    // Read.
    let one = get(&e, &t, &format!("spaces/one?space={name}"))?;
    assert_eq!(
        (one["label"].as_str(), one["kind"].as_str()),
        (Some("Cars"), Some("custom"))
    );
    // Update: label, parent, keywords.
    let up = post(
        &e,
        &t,
        "spaces/update",
        json!({ "space": name, "label": "Vehicles", "parent": other, "add_keywords": ["tyres"], "remove_keywords": ["mot"] }),
    )?;
    assert_eq!(up["space"]["label"], "Vehicles");
    assert_eq!(up["space"]["parent"], json!(other));
    assert_eq!(up["space"]["keywords"], json!(["garage", "tyres"]));
    // A write to remember what will be moved.
    post(
        &e,
        &t,
        "fact/add",
        json!({ "space": name, "text": "Winter tyres in the shed" }),
    )?;
    // Delete needs a choice.
    let r = call(&e, &t, "POST", "spaces/delete", json!({ "space": name }));
    assert_eq!(r.status, 400, "no silent delete");
    // Preview, then into another space; the parent receives the notes.
    let pre = post(
        &e,
        &t,
        "spaces/delete",
        json!({ "space": other, "archive": true, "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    assert_eq!(pre["children"][0]["goes_under"], Value::Null);
    assert!(
        pre["would"]
            .as_str()
            .is_some_and(|w| w.contains("archived"))
    );
    let dry = post(
        &e,
        &t,
        "spaces/delete",
        json!({ "space": name, "into": other, "dry_run": true }),
    )?;
    assert_eq!(dry["into"], json!(other));
    assert_eq!(
        get(&e, &t, "layout")?["spaces"].as_array().map(Vec::len),
        Some(2),
        "a preview changes nothing"
    );
    let done = post(
        &e,
        &t,
        "spaces/delete",
        json!({ "space": name, "into": other }),
    )?;
    assert_eq!(done["dry_run"], false);
    let moved = get(&e, &t, &format!("facts?space={other}"))?;
    assert!(moved["facts"].as_array().is_some_and(|f| f.iter().any(|x| {
        x["text"]
            .as_str()
            .is_some_and(|t| t.contains("Winter tyres"))
    })));
    // Archive the other one, list the archive, restore it.
    post(
        &e,
        &t,
        "spaces/delete",
        json!({ "space": other, "archive": true }),
    )?;
    let list = get(&e, &t, "spaces/archived")?;
    let archived = list["archived"].as_array().ok_or("archived")?;
    assert_eq!(archived.len(), 2);
    let mine = archived
        .iter()
        .find(|a| a["space"] == json!(other))
        .ok_or("archive of other")?["name"]
        .as_str()
        .ok_or("n")?
        .to_string();
    let pre = post(
        &e,
        &t,
        "spaces/restore",
        json!({ "archive": mine, "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    post(&e, &t, "spaces/restore", json!({ "archive": mine }))?;
    assert_eq!(
        get(&e, &t, "layout")?["spaces"].as_array().map(Vec::len),
        Some(1)
    );
    for ev in [
        "space_create",
        "space_update",
        "space_delete",
        "space_restore",
    ] {
        assert!(audits(&e, ev) >= 1, "{ev} is audited");
    }
    Ok(())
}

#[test]
fn a_member_cannot_touch_another_persons_objects() -> Res {
    let e = env()?;
    let (a, m) = (e.admin.clone(), e.member.clone());
    let space = post(&e, &a, "spaces/create", json!({ "label": "Private" }))?["space"]["space"]
        .as_str()
        .ok_or("n")?
        .to_string();
    let ctl = e.app.brain.open_control()?;
    access::connect_for(
        &ctl,
        users::OWNER,
        "owners-helper",
        &Rights::parse(&format!("{space}:rw"))?,
        None,
        false,
        false,
    )?;
    access::connect_peer(
        &ctl,
        users::OWNER,
        "peer-pc-cd34",
        &Rights::parse(&format!("{space}:rw"))?,
        false,
    )?;
    drop(ctl);
    for (path, body) in [
        ("spaces/update", json!({ "space": space, "label": "Mine" })),
        ("spaces/delete", json!({ "space": space, "archive": true })),
        (
            "space/rules",
            json!({ "space": space, "add_keywords": ["x"] }),
        ),
        (
            "space/vocabulary",
            json!({ "space": space, "name": "v", "op": "delete" }),
        ),
        ("connections/rotate", json!({ "agent": "owners-helper" })),
        ("connections/revoke", json!({ "agent": "owners-helper" })),
        ("connections/delete", json!({ "agent": "owners-helper" })),
        ("nodes/pause", json!({ "peer": "peer-pc-cd34" })),
        ("nodes/revoke", json!({ "peer": "peer-pc-cd34" })),
        ("users/delete", json!({ "id": users::OWNER })),
    ] {
        let r = call(&e, &m, "POST", path, body);
        assert!(
            matches!(r.status, 403 | 404),
            "{path}: {} {}",
            r.status,
            r.body_json()
        );
    }
    assert_eq!(
        get(&e, &m, &format!("spaces/one?space={space}"))
            .err()
            .map(|x| x.to_string().contains("403")),
        Some(true)
    );
    assert_eq!(
        get(&e, &m, "nodes")?["nodes"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        get(&e, &m, "users")?["users"].as_array().map(Vec::len),
        Some(1),
        "a member reads only themselves"
    );
    assert_eq!(
        get(&e, &a, "users")?["users"].as_array().map(Vec::len),
        Some(2)
    );
    // Their own display name, yes; their own role, no.
    post(
        &e,
        &m,
        "users/update",
        json!({ "id": e.member_id, "display": "Robert" }),
    )?;
    assert_eq!(
        call(
            &e,
            &m,
            "POST",
            "users/update",
            json!({ "id": e.member_id, "admin": true })
        )
        .status,
        403
    );
    Ok(())
}

#[test]
fn a_node_is_listed_renamed_paused_revoked_and_forgotten() -> Res {
    let e = env()?;
    let t = e.admin.clone();
    let ctl = e.app.brain.open_control()?;
    let key = access::connect_peer(
        &ctl,
        users::OWNER,
        "peer-laptop-ab12",
        &Rights::owner(),
        false,
    )?;
    drop(ctl);
    let list = get(&e, &t, "nodes")?;
    assert_eq!(list["nodes"][0]["name"], "laptop");
    let up = post(
        &e,
        &t,
        "nodes/update",
        json!({ "peer": "peer-laptop-ab12", "label": "Study", "note": "second floor" }),
    )?;
    assert_eq!(
        (up["node"]["name"].as_str(), up["node"]["note"].as_str()),
        (Some("Study"), Some("second floor"))
    );
    // Paused: its sync request is refused; resumed: served.
    let sync = |token: &str| {
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert("authorization".into(), format!("Bearer {token}"));
        e.app
            .handle(&Request {
                method: "GET".into(),
                path: "/mcp/peer-laptop-ab12/sync/spaces".into(),
                query: HashMap::new(),
                headers,
                body: vec![],
                ip: Some("10.0.0.5".into()),
                via_ingress: false,
                tailnet_login: None,
            })
            .status
    };
    assert_eq!(sync(&key), 200);
    let pre = post(
        &e,
        &t,
        "nodes/pause",
        json!({ "peer": "peer-laptop-ab12", "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    assert_eq!(
        get(&e, &t, "nodes")?["nodes"][0]["state"],
        "active",
        "a preview changes nothing"
    );
    post(&e, &t, "nodes/pause", json!({ "peer": "peer-laptop-ab12" }))?;
    assert_eq!(sync(&key), 423);
    post(
        &e,
        &t,
        "nodes/resume",
        json!({ "peer": "peer-laptop-ab12" }),
    )?;
    assert_eq!(sync(&key), 200);
    // Forget needs a revoked (or silent) machine.
    assert_eq!(
        call(
            &e,
            &t,
            "POST",
            "nodes/forget",
            json!({ "peer": "peer-laptop-ab12" })
        )
        .status,
        400
    );
    post(
        &e,
        &t,
        "nodes/revoke",
        json!({ "peer": "peer-laptop-ab12" }),
    )?;
    assert_eq!(sync(&key), 401, "the key stops");
    assert_eq!(get(&e, &t, "nodes")?["nodes"][0]["state"], "revoked");
    post(
        &e,
        &t,
        "nodes/forget",
        json!({ "peer": "peer-laptop-ab12" }),
    )?;
    assert_eq!(
        get(&e, &t, "nodes")?["nodes"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        get(&e, &t, "nodes?all=1")?["nodes"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    for ev in [
        "node_update",
        "node_pause",
        "node_resume",
        "node_revoke",
        "node_forget",
    ] {
        assert!(audits(&e, ev) >= 1, "{ev}");
    }
    Ok(())
}

#[test]
fn connections_rotate_and_are_deleted_once_revoked_and_people_are_removed_with_an_export() -> Res {
    let e = env()?;
    let t = e.admin.clone();
    let made = post(
        &e,
        &t,
        "connections",
        json!({ "agent": "helper", "rights": "all" }),
    )?;
    let old = made["token"].as_str().ok_or("token")?.to_string();
    let pre = post(
        &e,
        &t,
        "connections/rotate",
        json!({ "agent": "helper", "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    let new = post(&e, &t, "connections/rotate", json!({ "agent": "helper" }))?["token"]
        .as_str()
        .ok_or("t")?
        .to_string();
    assert_ne!(old, new);
    let ctl = e.app.brain.open_control()?;
    assert!(
        access::authenticate(&ctl, &old, 0)?.is_none()
            && access::authenticate(&ctl, &new, pensine_core::util::now())?.is_some()
    );
    drop(ctl);
    // Delete: refused while active, previewed, then done.
    assert_eq!(
        call(
            &e,
            &t,
            "POST",
            "connections/delete",
            json!({ "agent": "helper" })
        )
        .status,
        400
    );
    let pre = post(
        &e,
        &t,
        "connections/revoke",
        json!({ "agent": "helper", "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    post(&e, &t, "connections/revoke", json!({ "agent": "helper" }))?;
    assert_eq!(
        get(&e, &t, "connections/one?agent=helper")?["revoked"],
        true
    );
    post(
        &e,
        &t,
        "connections/delete",
        json!({ "agent": "helper", "dry_run": true }),
    )?;
    assert!(
        get(&e, &t, "connections/one?agent=helper").is_ok(),
        "a preview deletes nothing"
    );
    post(&e, &t, "connections/delete", json!({ "agent": "helper" }))?;
    assert!(get(&e, &t, "connections/one?agent=helper").is_err());
    // A person: preview, remove, the export exists, and nothing else of the memory moved.
    let bob_space = e.app.brain.add_vault(&e.member_id, "bob", "me")?;
    let pre = post(
        &e,
        &t,
        "users/delete",
        json!({ "id": e.member_id, "dry_run": true }),
    )?;
    assert_eq!(pre["spaces"], json!([bob_space.as_str()]));
    assert_eq!(get(&e, &t, "user?id=bob")?["disabled"], false);
    let done = post(&e, &t, "users/delete", json!({ "id": e.member_id }))?;
    assert!(
        done["export"]
            .as_str()
            .is_some_and(|p| PathBuf::from(p).join("manifest.json").is_file())
    );
    assert_eq!(get(&e, &t, "user?id=bob")?["disabled"], true);
    assert!(e.app.brain.has_space(&bob_space)?);
    assert_eq!(
        call(
            &e,
            &t,
            "POST",
            "users/delete",
            json!({ "id": users::OWNER })
        )
        .status,
        400,
        "not oneself"
    );
    for ev in ["rotate_key", "connection_delete", "user_delete"] {
        assert!(audits(&e, ev) >= 1, "{ev}");
    }
    Ok(())
}

#[test]
fn a_retired_note_comes_back_and_keywords_vocabularies_and_libraries_are_edited() -> Res {
    let e = env()?;
    let t = e.admin.clone();
    let s = post(&e, &t, "spaces/create", json!({ "label": "Notes" }))?["space"]["space"]
        .as_str()
        .ok_or("n")?
        .to_string();
    let f = post(
        &e,
        &t,
        "fact/add",
        json!({ "space": s, "text": "Water the plants on Sunday" }),
    )?["fact"]["id"]
        .as_str()
        .ok_or("id")?
        .to_string();
    post(
        &e,
        &t,
        "fact/retire",
        json!({ "space": s, "id": f, "reason": "oops" }),
    )?;
    let back = post(&e, &t, "fact/restore", json!({ "space": s, "id": f }))?;
    assert_eq!(back["result"], "restored");
    assert_eq!(
        call(
            &e,
            &t,
            "POST",
            "fact/restore",
            json!({ "space": s, "id": f })
        )
        .status,
        400,
        "already active"
    );
    // Keywords one by one.
    post(
        &e,
        &t,
        "space/rules",
        json!({ "space": s, "add_keywords": ["plants", "garden"], "add_tags": ["home/garden"] }),
    )?;
    post(
        &e,
        &t,
        "space/rules",
        json!({ "space": s, "remove_keywords": ["garden"] }),
    )?;
    let one = get(&e, &t, &format!("spaces/one?space={s}"))?;
    assert_eq!(
        (&one["keywords"], &one["tags"]),
        (&json!(["plants"]), &json!(["home/garden"]))
    );
    // A vocabulary: imported by the core (a file on the machine), edited and removed here.
    let space = Space::new(&s)?;
    {
        let mut conn = e.app.brain.open_space(&space)?;
        pensine_core::vocabulary::import(
            &mut conn,
            "signs",
            "test",
            &[("fever".into(), String::new(), String::new())],
            1,
        )?;
    }
    post(
        &e,
        &t,
        "space/vocabulary",
        json!({ "space": s, "name": "signs", "op": "add", "terms": ["cough"] }),
    )?;
    assert_eq!(
        get(&e, &t, &format!("space/vocabularies?space={s}"))?["vocabularies"][0]["terms"],
        2
    );
    let pre = post(
        &e,
        &t,
        "space/vocabulary",
        json!({ "space": s, "name": "signs", "op": "delete", "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    post(
        &e,
        &t,
        "space/vocabulary",
        json!({ "space": s, "name": "signs", "op": "delete" }),
    )?;
    assert_eq!(
        get(&e, &t, &format!("space/vocabularies?space={s}"))?["vocabularies"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    // A library folder: administrators mount, anyone unmounts their own, the folder is never touched.
    let dir = e.dir.join("lib");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("n.md"), "# Hello")?;
    let path = dir.to_string_lossy().to_string();
    let m = post(
        &e,
        &t,
        "space/library",
        json!({ "space": s, "op": "add", "path": path }),
    )?;
    let id = m["id"].as_i64().ok_or("id")?;
    assert_eq!(
        get(&e, &t, &format!("space/libraries?space={s}"))?["libraries"][0]["files"],
        1
    );
    let pre = post(
        &e,
        &t,
        "space/library",
        json!({ "space": s, "op": "remove", "id": id, "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    post(
        &e,
        &t,
        "space/library",
        json!({ "space": s, "op": "remove", "id": id }),
    )?;
    assert!(dir.join("n.md").is_file());
    assert_eq!(
        get(&e, &t, &format!("space/libraries?space={s}"))?["libraries"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    // A member cannot mount a path of the machine.
    let bob_space = e.app.brain.add_vault(&e.member_id, "bob", "me")?;
    let r = call(
        &e,
        &e.member,
        "POST",
        "space/library",
        json!({ "space": bob_space.as_str(), "op": "add", "path": path }),
    );
    assert_eq!(r.status, 403);
    Ok(())
}

#[test]
fn the_folder_of_a_space_on_this_node_is_set_read_synced_and_put_back() -> Res {
    let e = env()?;
    let t = e.admin.clone();
    let s = post(&e, &t, "spaces/create", json!({ "label": "Notes" }))?["space"]["space"]
        .as_str()
        .ok_or("n")?
        .to_string();
    post(
        &e,
        &t,
        "fact/add",
        json!({ "space": s, "text": "Water the plants on Sunday" }),
    )?;
    let base = std::env::temp_dir().join(format!("memcp-crud-folder-{}", std::process::id()));
    let dir = base
        .join("CloudStorage")
        .join("GoogleDrive-me@example.org")
        .join("notes");
    let _ = fs::remove_dir_all(&base);
    let path = dir.to_string_lossy().to_string();
    // Nothing chosen: the node's own folder, no mirror.
    let none = get(&e, &t, &format!("space/folder?space={s}"))?;
    assert_eq!(
        (none["custom"].as_bool(), none["mode"].is_null()),
        (Some(false), true)
    );
    // A preview first, then the setting; a cloud folder is flagged and allowed.
    let pre = post(
        &e,
        &t,
        "space/folder",
        json!({ "space": s, "path": path, "dry_run": true }),
    )?;
    assert_eq!(pre["dry_run"], true);
    assert!(
        pre["would"]
            .as_str()
            .is_some_and(|w| w.contains("not shared with the other nodes"))
    );
    assert!(!dir.exists(), "a preview changes nothing");
    let set = post(&e, &t, "space/folder", json!({ "space": s, "path": path }))?;
    assert_eq!(set["folder"]["cloud"], "google-drive");
    assert!(set["folder"]["warning"].is_string());
    assert_eq!(set["folder"]["mode"], "readwrite");
    let synced = post(&e, &t, "space/folder", json!({ "space": s, "op": "sync" }))?;
    assert_eq!(synced["scan"]["written"], 1, "{synced}");
    assert_eq!(synced["folder"]["files"], 1);
    assert!(synced["folder"]["last_scan"].is_string());
    let mds: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
    assert_eq!(mds.len(), 1);
    assert!(fs::read_to_string(mds[0].path())?.contains("Water the plants on Sunday"));
    assert_eq!(audits(&e, "folder_edit"), 2);
    // A member names no path of the machine, and sees no one else's space.
    let r = call(
        &e,
        &e.member,
        "POST",
        "space/folder",
        json!({ "space": s, "path": path }),
    );
    assert_eq!(r.status, 403);
    let r = call(
        &e,
        &e.member,
        "GET",
        &format!("space/folder?space={s}"),
        json!({}),
    );
    assert_eq!(r.status, 403);
    // The library list does not need to show the folder twice (the page filters it); `library add` is the
    // read-only alias of the same setting and replaces the folder.
    let ro = base.join("vault");
    fs::create_dir_all(&ro)?;
    fs::write(ro.join("a.md"), "# Hello")?;
    post(
        &e,
        &t,
        "space/library",
        json!({ "space": s, "op": "add", "path": ro.to_string_lossy() }),
    )?;
    let now = get(&e, &t, &format!("space/folder?space={s}"))?;
    assert_eq!(now["mode"], "read");
    assert_eq!(
        now["path"],
        fs::canonicalize(&ro)?.to_string_lossy().to_string()
    );
    assert_eq!(
        get(&e, &t, &format!("space/libraries?space={s}"))?["libraries"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "one folder, one index"
    );
    // Back to the default: the folder it had stays as it is.
    let back = post(&e, &t, "space/folder", json!({ "space": s, "path": null }))?;
    assert_eq!(back["folder"]["custom"], false);
    assert_eq!(fs::read_dir(&dir)?.count(), 1);
    assert!(
        post(&e, &t, "space/folder", json!({ "space": s, "op": "sync" })).is_err(),
        "nothing to sync"
    );
    assert!(
        post(
            &e,
            &t,
            "space/folder",
            json!({ "space": s, "mode": "nope", "path": path })
        )
        .is_err()
    );
    let _ = fs::remove_dir_all(&base);
    Ok(())
}

#[test]
fn the_sync_policy_of_a_machine_is_read_and_set_by_its_person_only() -> Res {
    let e = env()?;
    let t = e.admin.clone();
    let ctl = e.app.brain.open_control()?;
    let key = access::connect_peer(
        &ctl,
        users::OWNER,
        "peer-laptop-ab12",
        &Rights::owner(),
        false,
    )?;
    drop(ctl);
    for s in ["health", "work"] {
        e.app.brain.add_space_for(&Space::new(s)?, users::OWNER)?;
    }
    // Nothing set: both ways, for every space and for the spaces made later.
    let v = get(&e, &t, "sync/policy?peer=peer-laptop-ab12")?;
    assert_eq!(v["later"], "both");
    let row = |v: &Value, s: &str| {
        v["spaces"]
            .as_array()
            .and_then(|a| a.iter().find(|r| r["space"] == s).cloned())
            .unwrap_or_default()
    };
    assert_eq!(row(&v, "health")["policy"], "both");
    assert_eq!(row(&v, "health")["sensitive"], true);
    assert_eq!(row(&v, "work")["sensitive"], false);
    post(
        &e,
        &t,
        "sync/policy",
        json!({ "peer": "peer-laptop-ab12", "space": "health", "policy": "off" }),
    )?;
    post(
        &e,
        &t,
        "sync/policy",
        json!({ "peer": "peer-laptop-ab12", "later": "receive" }),
    )?;
    let v = get(&e, &t, "sync/policy?peer=peer-laptop-ab12")?;
    assert_eq!(row(&v, "health")["policy"], "off");
    assert_eq!(row(&v, "work")["policy"], "both", "already here");
    assert_eq!(v["later"], "receive");
    assert_eq!(audits(&e, "sync_policy"), 2);
    // A word that is not a policy, a space that is not one.
    for body in [
        json!({ "peer": "peer-laptop-ab12", "space": "health", "policy": "sometimes" }),
        json!({ "peer": "peer-laptop-ab12", "space": "nowhere", "policy": "off" }),
        json!({ "peer": "peer-laptop-ab12" }),
    ] {
        let r = call(&e, &t, "POST", "sync/policy", body);
        assert!(matches!(r.status, 400 | 403), "{}", r.body_json());
    }
    // Another person: neither reads nor sets it.
    let m = e.member.clone();
    let r = call(
        &e,
        &m,
        "GET",
        "sync/policy?peer=peer-laptop-ab12",
        json!({}),
    );
    assert_eq!(r.status, 403, "{}", r.body_json());
    let r = call(
        &e,
        &m,
        "POST",
        "sync/policy",
        json!({ "peer": "peer-laptop-ab12", "space": "health", "policy": "both" }),
    );
    assert_eq!(r.status, 403, "{}", r.body_json());
    // The machine's own key is not a key to the page.
    let r = call(
        &e,
        &key,
        "GET",
        "sync/policy?peer=peer-laptop-ab12",
        json!({}),
    );
    assert_eq!(r.status, 401);
    let r = call(
        &e,
        &key,
        "POST",
        "sync/policy",
        json!({ "peer": "peer-laptop-ab12", "space": "health", "policy": "both" }),
    );
    assert_eq!(r.status, 401);
    let v = get(&e, &t, "sync/policy?peer=peer-laptop-ab12")?;
    assert_eq!(row(&v, "health")["policy"], "off");
    Ok(())
}
