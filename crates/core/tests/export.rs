//! Export and adopt: a person's memory leaves as a directory and is taken in elsewhere,
//! identical, with nothing of the installation it came from.

mod common;

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{Res, Temp, sp};
use memcp_core::access::{self, Rights};
use memcp_core::export::{self, FORMAT};
use memcp_core::guide;
use memcp_core::text::Language;
use memcp_core::tools;
use memcp_core::users::{self, OWNER, User};
use memcp_core::{Brain, Space, files};
use rusqlite::Connection;
use serde_json::{Value, json};

struct World {
    t: Temp,
    brain: Brain,
    sam: User,
}

/// Sam owns `sam-work` and `sam-health`. Alex owns `alex-work` and shares it with Sam.
fn world() -> Result<World, Box<dyn Error>> {
    let t = Temp::new("export");
    let brain = Brain::init(&t.path().join("data"), &[sp("legacy")], Language::English)?;
    let ctl = brain.open_control()?;
    let sam = users::create_user(&ctl, OWNER, "sam", "Sam", false)?;
    let alex = users::create_user(&ctl, OWNER, "alex", "Alex", false)?;
    let work = brain.add_vault(&sam.id, "sam", "work")?;
    brain.set_layout(
        &sam.id,
        &work,
        &memcp_core::layout::Change {
            label: Some("Sam’s work".into()),
            ..Default::default()
        },
    )?;
    brain.add_vault(&sam.id, "sam", "health")?;
    brain.add_vault(&alex.id, "alex", "work")?;

    let caller = |user: &str, agent: &str, spec: &str| -> Result<access::Caller, Box<dyn Error>> {
        let r = Rights::parse(spec)?;
        let tok = access::connect_for(&ctl, user, agent, &r, None, false, false)?;
        Ok(access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?)
    };
    let s = caller(&sam.id, "s", "sam-work:rw,sam-health:rw")?;
    let a = caller(&alex.id, "a", "alex-work:rw")?;
    let put = |c: &access::Caller,
               space: &str,
               text: &str,
               tags: Value|
     -> Result<Value, Box<dyn Error>> {
        Ok(tools::call(
            &brain,
            c,
            "remember",
            &json!({ "space": space, "text": text, "tags": tags }),
        )?)
    };
    put(
        &s,
        "sam-work",
        "The audit report is due on Friday.",
        json!(["work/audit"]),
    )?;
    let old = put(
        &s,
        "sam-work",
        "The team meets on Mondays at nine.",
        json!(["work/meetings"]),
    )?;
    let id = old["fact"]["id"].as_str().ok_or("id")?.to_string();
    tools::call(
        &brain,
        &s,
        "supersede",
        &json!({ "id": id, "text": "The team meets on Tuesdays at ten.", "reason": "the schedule moved" }),
    )?;
    let gone = put(
        &s,
        "sam-work",
        "The old printer is in room 4.",
        json!(["work/office"]),
    )?;
    tools::call(
        &brain,
        &s,
        "retire",
        &json!({ "id": gone["fact"]["id"], "reason": "the printer was removed" }),
    )?;
    put(
        &s,
        "sam-work",
        "Map: audits under work/audit, meetings under work/meetings.",
        json!(["index"]),
    )?;
    put(
        &s,
        "sam-health",
        "Resting heart rate was 58 on Sunday.",
        json!(["health/vitals"]),
    )?;
    put(
        &a,
        "alex-work",
        "ALEX-ONLY secret plan for the reorganisation.",
        json!([]),
    )?;
    files::put(
        &brain,
        &sp("sam-work"),
        "sam",
        "lease.txt",
        "The flat lease",
        b"Lease text, twelve months.",
    )?;
    {
        let conn = brain.open_space(&sp("sam-health"))?;
        conn.execute(
            "INSERT INTO record (id, kind, label, detail, start, status, verification, author, created_at, updated_at)
             VALUES ('r1', 'condition', 'Seasonal rhinitis', 'Spring only', '2024-03', 'active', 'confirmed', 'sam', 1, 1)",
            [],
        )?;
    }
    guide::set(
        &ctl,
        &sam.id,
        &guide::person_scope(&sam.id),
        "Short sentences, no emojis.",
    )?;
    guide::set(
        &ctl,
        &sam.id,
        &guide::space_scope(&sp("sam-work")),
        "Formal tone at work.",
    )?;
    Ok(World { t, brain, sam })
}

fn out_dir(w: &World) -> PathBuf {
    w.t.path().join("out")
}

fn export_of(w: &World) -> Result<PathBuf, Box<dyn Error>> {
    Ok(export::export_person(
        &w.brain,
        &w.sam,
        &out_dir(w),
        1_900_000_000,
    )?)
}

/// Id, fingerprint, status and tags of every fact of a space, and the history.
fn digest(brain: &Brain, space: &Space) -> Result<(Vec<String>, i64), Box<dyn Error>> {
    let c = brain.open_space(space)?;
    let mut st = c.prepare(
        "SELECT f.id || '|' || f.text_hash || '|' || f.status || '|' || coalesce(f.replaced_by, '') || '|' ||
                coalesce((SELECT group_concat(tag) FROM (SELECT tag FROM fact_tags t WHERE t.fact_id = f.id ORDER BY tag)), '')
         FROM facts f ORDER BY f.id",
    )?;
    let rows: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let events: i64 = c.query_row("SELECT count(*) FROM fact_events", [], |r| r.get(0))?;
    Ok((rows, events))
}

fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(all_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// A second installation with one person, `kim`, who has nothing yet.
fn elsewhere() -> Result<(Temp, Brain, User), Box<dyn Error>> {
    let t = Temp::new("elsewhere");
    let brain = Brain::init(&t.path().join("data"), &[], Language::English)?;
    let ctl = brain.open_control()?;
    let kim = users::create_user(&ctl, OWNER, "kim", "Kim", false)?;
    Ok((t, brain, kim))
}

#[test]
fn an_export_made_before_the_rename_is_still_read() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let path = dir.join("manifest.json");
    let old = fs::read_to_string(&path)?.replace("\"memcp-export\"", "\"humanmemcp-export\"");
    fs::write(&path, old)?;
    assert_eq!(export::verify_export(&dir)?["format"], "humanmemcp-export");
    Ok(())
}

/// An export made before spaces were called spaces: the manifest lists `vaults`, the databases are in `vaults/`, and the
/// fingerprints are keyed by those paths. It verifies, and it is taken in whole.
#[test]
fn an_export_in_the_old_shape_with_vaults_is_verified_and_taken_in() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let current = export::verify_export(&dir)?;
    assert!(current["spaces"].is_array() && current["vaults"].is_null());
    fs::rename(dir.join("spaces"), dir.join("vaults"))?;
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    let spaces = m["spaces"].take();
    m.as_object_mut().ok_or("manifest")?.remove("spaces");
    m["vaults"] = spaces;
    let hashes = m["hashes"].as_object().ok_or("hashes")?.clone();
    let renamed: serde_json::Map<String, Value> = hashes
        .into_iter()
        .map(|(k, v)| (k.replacen("spaces/", "vaults/", 1), v))
        .collect();
    m["hashes"] = Value::Object(renamed);
    fs::write(&path, serde_json::to_string_pretty(&m)?)?;
    let old = export::verify_export(&dir)?;
    assert_eq!(
        export::manifest_spaces(&old).map(Vec::len),
        export::manifest_spaces(&current).map(Vec::len)
    );
    let (_t, brain, kim) = elsewhere()?;
    let adopted = export::adopt(&brain, &dir, &kim, None)?;
    assert_eq!(adopted.len(), 2, "both spaces of the old export arrive");
    assert!(brain.has_space(&sp("sam-work"))?);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_export_holds_only_what_the_person_owns_and_no_key() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let m = export::verify_export(&dir)?;
    assert_eq!(m["format"], FORMAT);
    assert_eq!(m["person"]["name"], "sam");
    let names: Vec<&str> = m["spaces"]
        .as_array()
        .ok_or("spaces")?
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert_eq!(
        names,
        ["sam-health", "sam-work"],
        "not the space Alex shared, not the legacy one"
    );
    assert_eq!(m["spaces"][1]["kind"], "work");
    assert_eq!(
        m["spaces"][1]["facts"],
        json!({ "active": 4, "replaced": 1, "retired": 1 })
    );
    assert_eq!(m["spaces"][1]["files"], 1);
    // What is in the directory: spaces, files, readable, guides, README, manifest. No control data.
    let rels: Vec<String> = all_files(&dir)
        .iter()
        .filter_map(|p| {
            p.strip_prefix(&dir)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        })
        .collect();
    for want in [
        "manifest.json",
        "README.txt",
        "guides.json",
        "spaces/sam-work.sqlite",
        "readable/sam-health.md",
    ] {
        assert!(rels.iter().any(|r| r == want), "{want} in {rels:?}");
    }
    assert!(
        rels.iter().all(|r| !r.contains("control")),
        "no control data: {rels:?}"
    );
    // No key, and nothing from the space of someone else, anywhere in any byte.
    for p in all_files(&dir) {
        let bytes = fs::read(&p)?;
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("hcp_"), "a key in {}", p.display());
        assert!(
            !text.contains("ALEX-ONLY"),
            "Alex's text in {}",
            p.display()
        );
    }
    // Private: nobody else on the machine reads it.
    assert_eq!(fs::metadata(&dir)?.permissions().mode() & 0o077, 0);
    assert_eq!(
        fs::metadata(dir.join("manifest.json"))?
            .permissions()
            .mode()
            & 0o077,
        0
    );
    Ok(())
}

#[test]
fn the_readable_version_tells_the_facts_and_their_history() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let work = fs::read_to_string(dir.join("readable/sam-work.md"))?;
    assert!(work.starts_with("# sam-work (work)"));
    let (live, past) = work
        .split_once("## Replaced or retired")
        .ok_or("no history section")?;
    assert!(live.contains("## In force (4)"));
    assert!(live.contains("The audit report is due on Friday.") && live.contains("work/audit"));
    assert!(live.contains("The team meets on Tuesdays at ten."));
    assert!(!live.contains("Mondays"), "the old text is not in force");
    assert!(past.contains("(2)"));
    assert!(past.contains("The team meets on Mondays at nine.") && past.contains("Replaced on"));
    assert!(
        past.contains("The old printer is in room 4.")
            && past.contains("Retired on")
            && past.contains("the printer was removed")
    );
    let health = fs::read_to_string(dir.join("readable/sam-health.md"))?;
    assert!(health.contains("Resting heart rate was 58") && health.contains("## Record entries"));
    assert!(health.contains("condition: Seasonal rhinitis"));
    assert!(!work.contains("ALEX-ONLY") && !health.contains("ALEX-ONLY"));
    let g: Value = serde_json::from_str(&fs::read_to_string(dir.join("guides.json"))?)?;
    assert!(
        g.as_array()
            .ok_or("guides")?
            .iter()
            .any(|x| x["scope"] == "person"
                && x["body"] == "Short sentences, no emojis."
                && x["current"] == true)
    );
    assert!(
        g.as_array()
            .ok_or("guides")?
            .iter()
            .any(|x| x["scope"] == "space:sam-work" && x["body"] == "Formal tone at work.")
    );
    Ok(())
}

#[test]
fn an_export_is_taken_in_whole_by_another_installation() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let (_t, brain, kim) = elsewhere()?;
    let done = export::adopt(&brain, &dir, &kim, None)?;
    assert_eq!(
        done.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["sam-health", "sam-work"]
    );
    assert_eq!(
        (done[1].kind.as_str(), done[1].facts, done[1].files),
        ("work", 6, 1)
    );
    for v in ["sam-work", "sam-health"] {
        assert_eq!(
            digest(&w.brain, &sp(v))?,
            digest(&brain, &sp(v))?,
            "{v}: facts, tags and history identical"
        );
    }
    // Kim owns them, with their kinds; a file reads back; the instructions came with them.
    let ctl = brain.open_control()?;
    assert!(users::is_space_owner(&ctl, &sp("sam-work"), &kim.id)?);
    assert_eq!(brain.space_kind(&sp("sam-health"))?, "health");
    let listed = files::list(&brain, &sp("sam-work"), None, 10)?;
    assert_eq!(listed.len(), 1);
    let (info, bytes) = files::get(&brain, &sp("sam-work"), &listed[0].id)?.ok_or("file")?;
    assert_eq!(
        (info.name.as_str(), bytes.as_slice()),
        ("lease.txt", b"Lease text, twelve months.".as_slice())
    );
    assert_eq!(
        guide::current(&ctl, &guide::space_scope(&sp("sam-work")))?
            .ok_or("guide")?
            .body,
        "Formal tone at work."
    );
    assert_eq!(
        guide::current(&ctl, &guide::person_scope(&kim.id))?
            .ok_or("guide")?
            .body,
        "Short sentences, no emojis."
    );
    // Nothing of the old installation came: no connection, and Alex's space is nowhere.
    assert!(access::list(&ctl)?.is_empty());
    assert!(!brain.has_space(&sp("alex-work"))?);
    // And the memory works: a search finds the text, with its tag.
    let r = access::connect_for(
        &ctl,
        &kim.id,
        "k",
        &Rights::parse("all")?,
        None,
        false,
        false,
    )?;
    let c = access::authenticate(&ctl, &r, 1_900_000_000)?.ok_or("refused")?;
    let found = tools::call(
        &brain,
        &c,
        "recall",
        &json!({ "query": "audit report", "tags": ["work"] }),
    )?;
    assert_eq!(
        found["results"][0]["text"],
        "The audit report is due on Friday."
    );
    Ok(())
}

#[test]
fn an_erased_recording_leaves_no_bytes_in_the_export_and_the_record_travels() -> Res {
    use memcp_core::facts::{self, NewFact, Remembered};
    let w = world()?;
    let work = sp("sam-work");
    let mut memo = vec![0, 0, 0, 0x20];
    memo.extend_from_slice(b"ftypM4A \0\0\0\0");
    memo.resize(300, 9);
    let rec = files::put_tagged(
        &w.brain,
        &work,
        "device",
        "Mémo.m4a",
        "",
        &memo,
        &["inbox/audio".to_string()],
    )?;
    let conn = w.brain.open_space(&work)?;
    let Remembered::Created(text) = facts::remember(
        &conn,
        &work,
        "dictee",
        NewFact::agent(
            &format!("Mémo, audio {}: acheter du pain.", rec.name),
            "agent:dictee",
        ),
        &[],
    )?
    else {
        return Err("transcript".into());
    };
    conn.execute(
        "UPDATE facts SET created_at = 1700000000 WHERE id = ?1",
        [&text.id],
    )?;
    conn.execute("UPDATE files SET created_at = 1699999000", [])?;
    files::erase_at(
        &w.brain,
        &work,
        "dictee",
        &rec.id,
        &work,
        &text.id,
        1_800_000_000,
    )?;

    let dir = export_of(&w)?;
    assert!(
        all_files(&dir)
            .iter()
            .all(|p| p.file_name().and_then(|n| n.to_str()) != Some(rec.sha256.as_str())),
        "the recording is not in the export"
    );
    // The record and its trace travel; another installation takes the space in whole.
    let (_t, brain, kim) = elsewhere()?;
    export::adopt(&brain, &dir, &kim, None)?;
    assert!(files::erased(&brain, &work, &rec.id)?);
    assert!(files::get(&brain, &work, &rec.id)?.is_none());
    assert_eq!(
        files::list(&brain, &work, None, 10)?.len(),
        1,
        "the lease stays"
    );
    Ok(())
}

#[test]
fn an_altered_export_is_caught_and_nothing_is_adopted() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let (t, brain, kim) = elsewhere()?;
    let data = t.path().join("data");
    let before = all_files(&data).len();

    // One byte of a space changed.
    let db = dir.join("spaces/sam-work.sqlite");
    let mut bytes = fs::read(&db)?;
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&db, &bytes)?;
    let e = export::verify_export(&dir)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        e.contains("spaces/sam-work.sqlite") && e.contains("altered"),
        "{e}"
    );
    assert!(export::adopt(&brain, &dir, &kim, None).is_err());
    assert_eq!(all_files(&data).len(), before, "no file left behind");
    assert!(brain.spaces()?.is_empty(), "nothing registered");

    // A file missing, a file added, a path that climbs out of the export.
    let dir = export_of_again(&w)?;
    let blob = all_files(&dir.join("files")).pop().ok_or("blob")?;
    fs::remove_file(&blob)?;
    assert!(
        export::verify_export(&dir)
            .err()
            .is_some_and(|e| e.to_string().contains("cannot be read"))
    );
    let dir = export_of_again(&w)?;
    fs::write(dir.join("extra.txt"), "not listed")?;
    assert!(
        export::verify_export(&dir)
            .err()
            .is_some_and(|e| e.to_string().contains("not in its manifest"))
    );
    let dir = export_of_again(&w)?;
    let mut m: Value = serde_json::from_str(&fs::read_to_string(dir.join("manifest.json"))?)?;
    m["hashes"]["../outside"] = json!("00");
    fs::write(dir.join("manifest.json"), m.to_string())?;
    assert!(
        export::verify_export(&dir)
            .err()
            .is_some_and(|e| e.to_string().contains("outside the export"))
    );
    // Not an export at all.
    assert!(export::verify_export(t.path()).is_err());
    Ok(())
}

fn export_of_again(w: &World) -> Result<PathBuf, Box<dyn Error>> {
    static N: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    let at = 1_900_000_000 + N.fetch_add(1, std::sync::atomic::Ordering::SeqCst) * 10;
    Ok(export::export_person(&w.brain, &w.sam, &out_dir(w), at)?)
}

#[test]
fn a_name_in_use_is_refused_up_front_and_a_prefix_gives_other_names() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    // Taking his own export back into the same installation: the names are taken.
    let e = export::adopt(&w.brain, &dir, &w.sam, None)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        e.contains("already in use here") && e.contains("sam-work") && e.contains("prefix"),
        "{e}"
    );
    assert_eq!(w.brain.spaces()?.len(), 4, "nothing was added");
    // With a prefix, the spaces come as copies, under their new name, and work.
    let ctl = w.brain.open_control()?;
    let kim = users::create_user(&ctl, OWNER, "kim", "Kim", false)?;
    let done = export::adopt(&w.brain, &dir, &kim, Some("copy"))?;
    assert_eq!(done[0].name, "copy-sam-health");
    assert_eq!(
        digest(&w.brain, &sp("sam-work"))?,
        digest(&w.brain, &sp("copy-sam-work"))?
    );
    assert!(
        w.brain.open_space(&sp("copy-sam-work")).is_ok(),
        "it declares its new name"
    );
    assert_eq!(
        guide::current(&ctl, &guide::space_scope(&sp("copy-sam-work")))?
            .ok_or("guide")?
            .body,
        "Formal tone at work."
    );
    let again = export::adopt(&w.brain, &dir, &kim, Some("copy"))
        .err()
        .map(|e| e.to_string());
    assert!(
        again.is_some_and(|m| m.contains("already in use")),
        "a second time is refused too"
    );
    Ok(())
}

#[test]
fn an_unknown_kind_becomes_custom_and_a_person_with_nothing_has_nothing_to_export() -> Res {
    let w = world()?;
    let dir = export_of(&w)?;
    let mut m: Value = serde_json::from_str(&fs::read_to_string(dir.join("manifest.json"))?)?;
    m["spaces"][0]["kind"] = json!("alien");
    fs::write(dir.join("manifest.json"), m.to_string())?;
    let (_t, brain, kim) = elsewhere()?;
    export::adopt(&brain, &dir, &kim, None)?;
    assert_eq!(brain.space_kind(&sp("sam-health"))?, "custom");
    assert_eq!(brain.space_kind(&sp("sam-work"))?, "work");
    // Someone who owns no space: refused, and so is a destination inside the data directory.
    let (_t2, b2, nobody) = elsewhere()?;
    assert!(
        export::export_person(&b2, &nobody, &out_dir(&w), 1_900_000_001)
            .err()
            .is_some_and(|e| e.to_string().contains("owns no space"))
    );
    let inside = w.brain.dir().join("exports");
    assert!(
        export::export_person(&w.brain, &w.sam, &inside, 1_900_000_002)
            .err()
            .is_some_and(|e| e.to_string().contains("outside the data directory"))
    );
    assert!(!inside.exists());
    Ok(())
}

#[allow(dead_code)]
fn _unused(_: BTreeMap<String, String>, _: Connection) {}
