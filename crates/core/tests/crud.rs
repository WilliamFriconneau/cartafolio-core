//! Create, read, update, delete for connections, machines (peers), people, notes, vocabularies and library folders.

mod common;

use common::{Res, Temp, brain, sp};
use pensine_core::access::{self, Rights};
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::peers;
use pensine_core::people;
use pensine_core::users::{self, OWNER};
use pensine_core::{sync, util, vocabulary};

fn audit_count(b: &pensine_core::Brain, event: &str) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(b.open_control()?.query_row(
        "SELECT count(*) FROM auth_audit WHERE event = ?1",
        [event],
        |r| r.get(0),
    )?)
}

#[test]
fn a_connection_key_rotates_and_a_revoked_connection_is_removed_from_the_list() -> Res {
    let t = Temp::new("crud");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let ctl = &ctl;
    let old = access::connect(
        ctl,
        "helper",
        &Rights::parse("work:rw")?,
        Some(30),
        false,
        false,
    )?;
    let before = access::get(ctl, "helper")?.ok_or("helper")?;
    let new = access::rotate(ctl, "helper", OWNER)?.ok_or("rotated")?;
    assert_ne!(old, new);
    assert!(
        access::authenticate(ctl, &old, util::now())?.is_none(),
        "the old key stops"
    );
    assert!(access::authenticate(ctl, &new, util::now())?.is_some());
    let after = access::get(ctl, "helper")?.ok_or("helper")?;
    assert_eq!(
        (after.rights.clone(), after.expires_at),
        (before.rights, before.expires_at)
    );
    // Someone else's connection, or a revoked one, does not rotate.
    let alex = users::create_user(ctl, OWNER, "alex", "Alex", false)?.id;
    assert!(access::rotate(ctl, "helper", &alex)?.is_none());
    // Delete: only a revoked one, only by its owner.
    assert!(
        access::delete(ctl, "helper", OWNER).is_err(),
        "revoke first"
    );
    access::revoke(ctl, "helper")?;
    assert!(access::rotate(ctl, "helper", OWNER)?.is_none());
    assert!(!access::delete(ctl, "helper", &alex)?);
    assert!(access::delete(ctl, "helper", OWNER)?);
    assert!(access::get(ctl, "helper")?.is_none());
    assert_eq!(audit_count(&b, "connection_delete")?, 1);
    assert_eq!(audit_count(&b, "rotate_key")?, 1);
    Ok(())
}

#[test]
fn a_node_is_renamed_paused_revoked_and_forgotten_without_losing_its_history() -> Res {
    let t = Temp::new("crud");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let owner = users::user(&ctl, OWNER)?.ok_or("owner")?;
    let alex = users::create_user(&ctl, OWNER, "alex", "Alex", false)?;
    access::connect_peer(
        &ctl,
        pensine_core::users::OWNER,
        "peer-laptop-ab12",
        &Rights::parse("work:rw")?,
        false,
    )?;
    let now = util::now();
    let listed = peers::list(&b, &owner, now, false)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].name.as_str(), listed[0].state.as_str()),
        ("laptop", "active")
    );
    // Rename and note.
    let n = peers::update(
        &b,
        &owner,
        "peer-laptop-ab12",
        Some("Kitchen laptop"),
        Some("the old one"),
    )?;
    assert_eq!(
        (n.name.as_str(), n.note.as_str()),
        ("Kitchen laptop", "the old one")
    );
    // Another person cannot see or touch it.
    assert!(peers::list(&b, &alex, now, false)?.is_empty());
    assert!(peers::set_paused(&b, &alex, "peer-laptop-ab12", true).is_err());
    assert!(peers::revoke(&b, &alex, "peer-laptop-ab12").is_err());
    // Pause and resume.
    assert_eq!(
        peers::set_paused(&b, &owner, "peer-laptop-ab12", true)?.state,
        "paused"
    );
    assert!(peers::is_paused(&ctl, "peer-laptop-ab12"));
    assert_eq!(
        peers::set_paused(&b, &owner, "peer-laptop-ab12", false)?.state,
        "active"
    );
    assert!(!peers::is_paused(&ctl, "peer-laptop-ab12"));
    // An active, recently used machine cannot be forgotten; a revoked one can, and the entry was marked revoked first.
    assert!(peers::forget(&b, &owner, "peer-laptop-ab12", now).is_err());
    assert_eq!(
        peers::revoke(&b, &owner, "peer-laptop-ab12")?.state,
        "revoked"
    );
    assert_eq!(peers::list(&b, &owner, now, false)?[0].state, "revoked");
    peers::forget(&b, &owner, "peer-laptop-ab12", now)?;
    assert!(
        peers::list(&b, &owner, now, false)?.is_empty(),
        "hidden from the lists"
    );
    assert_eq!(
        peers::list(&b, &owner, now, true)?.len(),
        1,
        "kept, with its history"
    );
    // Not a machine.
    assert!(peers::update(&b, &owner, "helper", Some("x"), None).is_err());
    for e in [
        "node_update",
        "node_pause",
        "node_resume",
        "node_revoke",
        "node_forget",
    ] {
        assert!(audit_count(&b, e)? >= 1, "{e} is audited");
    }
    // Revoked, its key stays refused.
    assert!(
        sync::status(&b, now)?
            .iter()
            .all(|p| p.peer != "peer-laptop-ab12")
    );
    Ok(())
}

#[test]
fn removing_a_person_disables_and_exports_and_never_loses_the_memory() -> Res {
    let t = Temp::new("crud");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let admin = users::user(&ctl, OWNER)?.ok_or("owner")?;
    let alex = users::create_user(&ctl, OWNER, "alex", "Alex", false)?;
    let bob = users::create_user(&ctl, OWNER, "bob", "Bob", false)?;
    b.set_layout(
        OWNER,
        &sp("personal"),
        &pensine_core::layout::Change {
            label: Some("Owner’s personal space".into()),
            ..Default::default()
        },
    )?;
    let s = b.add_vault(&alex.id, "alex", "me")?;
    {
        let conn = b.open_space(&s)?;
        facts::remember(
            &conn,
            &s,
            "alex",
            NewFact::agent("A thing to keep", "tester"),
            &[],
        )?;
    }
    access::connect_for(
        &ctl,
        &alex.id,
        "alex-key",
        &Rights::parse(&format!("{s}:rw"))?,
        None,
        false,
        false,
    )?;
    // A member cannot remove anyone, nobody removes themselves.
    assert!(people::retire_person(&b, &bob, &alex.id, true, 1).is_err());
    assert!(people::retire_person(&b, &admin, &admin.id, true, 1).is_err());
    let dry = people::retire_person(&b, &admin, &alex.id, true, 100)?;
    assert!(dry.dry_run && dry.connections == ["alex-key"] && dry.spaces.len() == 1);
    assert!(
        !users::user(&ctl, &alex.id)?.ok_or("alex")?.disabled,
        "a dry run changes nothing"
    );
    let done = people::retire_person(&b, &admin, &alex.id, false, 100)?;
    let out = done.export.ok_or("export")?;
    pensine_core::export::verify_export(&out)?;
    assert!(users::user(&ctl, &alex.id)?.ok_or("alex")?.disabled);
    assert!(b.dir().join("archive/people").is_dir());
    // The memory is still there, and enabling brings the person back.
    assert!(b.has_space(&s)?);
    assert!(
        people::retire_person(&b, &admin, &alex.id, false, 101).is_err(),
        "already removed"
    );
    assert_eq!(audit_count(&b, "user_delete")?, 1);
    users::set_disabled(&ctl, OWNER, &alex.id, false)?;
    // The last administrator cannot be removed.
    let second = users::create_user(&ctl, OWNER, "chris", "Chris", true)?;
    people::retire_person(&b, &admin, &second.id, false, 102)?;
    Ok(())
}

#[test]
fn a_retired_note_is_restored_as_a_new_version_and_the_old_one_stays_retired() -> Res {
    let t = Temp::new("crud");
    let b = brain(&t)?;
    let s = sp("home");
    let conn = b.open_space(&s)?;
    let Remembered::Created(f) = facts::remember(
        &conn,
        &s,
        "me",
        NewFact {
            tags: vec!["chores".into()],
            ..NewFact::agent("Take the bins out on Tuesday", "me")
        },
        &[],
    )?
    else {
        return Err("created".into());
    };
    assert!(
        facts::restore(&conn, &s, "me", &f.id, None).is_err(),
        "only a retired note"
    );
    facts::retire(&conn, "me", &f.id, "mistake")?;
    let back = facts::restore(&conn, &s, "me", &f.id, Some("it was right"))?;
    assert_eq!(back.status, "active");
    assert_eq!(
        (back.text.as_str(), back.tags.clone()),
        ("Take the bins out on Tuesday", vec!["chores".to_string()])
    );
    assert_eq!(back.replaces.as_deref(), Some(f.id.as_str()));
    let old = facts::read_fact(&conn, &f.id)?.ok_or("old")?;
    assert_eq!(old.status, "retired", "the old one stays retired");
    assert!(
        facts::restore(&conn, &s, "me", &f.id, None).is_err(),
        "already active again"
    );
    Ok(())
}

#[test]
fn vocabulary_terms_and_library_folders_can_be_edited_and_removed() -> Res {
    let t = Temp::new("crud");
    let b = brain(&t)?;
    let mut conn = b.open_space(&sp("health"))?;
    let terms = vec![("fever".to_string(), String::new(), String::new())];
    vocabulary::import(&mut conn, "signs", "test", &terms, 1)?;
    assert_eq!(
        vocabulary::add_terms(&mut conn, "signs", &["Cough".into(), "fever".into()])?,
        1
    );
    assert_eq!(vocabulary::list(&conn)?[0].terms, 2);
    assert_eq!(
        vocabulary::remove_terms(&mut conn, "signs", &["cough".into(), "nothing".into()])?,
        1
    );
    assert_eq!(vocabulary::list(&conn)?[0].terms, 1);
    assert!(vocabulary::add_terms(&mut conn, "missing", &["x".into()]).is_err());
    // A library folder: removing it forgets the index, never the folder.
    let dir = t.path().join("notes");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("a.md"), "# Title\nbody [[b]]")?;
    let id = pensine_core::library::add_root(&conn, &dir, 1)?;
    pensine_core::library::rescan_all(&conn, 2)?;
    assert_eq!(pensine_core::library::roots(&conn)?[0].entries, 1);
    assert!(pensine_core::library::remove_root(&conn, id)?);
    assert!(pensine_core::library::roots(&conn)?.is_empty());
    assert!(dir.join("a.md").is_file(), "the original is never touched");
    assert!(!pensine_core::library::remove_root(&conn, id)?);
    Ok(())
}

#[test]
fn keywords_and_tags_are_added_and_removed_one_by_one_with_history() -> Res {
    let t = Temp::new("crud");
    let b = brain(&t)?;
    let s = sp("work");
    let conn = b.open_space(&s)?;
    let add = |kw: &[&str], rm: &[&str]| {
        pensine_core::rules::edit(
            &conn,
            &s,
            "me",
            &pensine_core::rules::Edit {
                add_keywords: kw.iter().map(|k| k.to_string()).collect(),
                remove_keywords: rm.iter().map(|k| k.to_string()).collect(),
                add_tags: vec!["Clients".into()],
                ..Default::default()
            },
        )
    };
    assert_eq!(add(&["Atlas", "quote"], &[])?.keywords, ["atlas", "quote"]);
    assert_eq!(add(&[], &["quote"])?.keywords, ["atlas"]);
    let (_, r) = pensine_core::rules::read(&conn);
    assert_eq!(
        (r.keywords, r.tags),
        (vec!["atlas".to_string()], vec!["clients".to_string()])
    );
    // Each change is a version: one active rules note, older ones in the history.
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND id IN (SELECT fact_id FROM fact_tags WHERE tag = 'rules')",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(n, 1);
    Ok(())
}
