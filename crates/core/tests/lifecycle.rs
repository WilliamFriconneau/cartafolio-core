//! The life of a space: create, update, delete into another space or archive, restore. Nothing is destroyed.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact};
use memcp_core::layout::Change;
use memcp_core::lifecycle::{Children, Disposition, SpaceNew, SpaceUpdate};
use memcp_core::rules;
use memcp_core::users::{self, OWNER};

fn note(b: &memcp_core::Brain, space: &str, text: &str, tags: &[&str]) -> Res {
    let conn = b.open_space(&sp(space))?;
    let mut n = NewFact::agent(text, "tester");
    n.tags = tags.iter().map(|t| t.to_string()).collect();
    facts::remember(&conn, &sp(space), "tester", n, &[])?;
    Ok(())
}

fn active(b: &memcp_core::Brain, space: &str) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(b.open_space(&sp(space))?.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active'",
        [],
        |r| r.get(0),
    )?)
}

#[test]
fn a_space_is_created_with_a_kind_a_parent_and_keywords_and_changed_later() -> Res {
    let t = Temp::new("life");
    let b = brain(&t)?;
    let s = b.create_space_with(
        OWNER,
        "owner",
        &SpaceNew {
            kind: Some("work".into()),
            label: "Clients".into(),
            parent: Some(sp("work")),
            keywords: vec!["#Atlas".into(), "devis".into()],
            tags: vec!["clients".into()],
        },
    )?;
    let info = b.space_info(OWNER, &s)?;
    assert_eq!(info.label, "Clients");
    assert_eq!(info.kind, "work");
    assert_eq!(info.parent, Some(sp("work")));
    assert_eq!(info.rules.keywords, ["atlas", "devis"]);
    assert_eq!(info.rules.tags, ["clients"]);
    assert!(
        b.create_space_with(
            OWNER,
            "owner",
            &SpaceNew {
                kind: Some("nonsense".into()),
                label: "X".into(),
                ..Default::default()
            }
        )
        .is_err()
    );
    let info = b.update_space(
        OWNER,
        "owner",
        &s,
        &SpaceUpdate {
            layout: Change {
                label: Some("Customers".into()),
                parent: Some(None),
                position: Some(7),
            },
            kind: Some("custom".into()),
            rules: rules::Edit {
                add_keywords: vec!["facture".into()],
                remove_keywords: vec!["devis".into()],
                ..Default::default()
            },
        },
    )?;
    assert_eq!(
        (info.label.as_str(), info.parent.clone(), info.position),
        ("Customers", None, 7)
    );
    assert_eq!(info.kind, "custom");
    assert_eq!(info.rules.keywords, ["atlas", "facture"]);
    // Every change is audited.
    let ctl = b.open_control()?;
    let n: i64 = ctl.query_row(
        "SELECT count(*) FROM auth_audit WHERE event IN ('space_create','space_update')",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(n, 2);
    Ok(())
}

#[test]
fn deleting_into_another_space_moves_everything_and_keeps_the_original_aside() -> Res {
    let t = Temp::new("life");
    let b = brain(&t)?;
    note(&b, "work", "Call the client about the quote", &["clients"])?;
    note(&b, "work", "Invoice sent on Monday", &[])?;
    note(&b, "home", "Buy light bulbs", &[])?;
    memcp_core::files::put_tagged(
        &b,
        &sp("work"),
        "owner",
        "plan.txt",
        "a plan",
        b"hello",
        &[],
    )?;
    let conn = b.open_space(&sp("work"))?;
    rules::edit(
        &conn,
        &sp("work"),
        "owner",
        &rules::Edit {
            add_keywords: vec!["atlas".into()],
            ..Default::default()
        },
    )?;
    drop(conn);
    // A child of `work` goes to the top when asked.
    let child = b.create_space_with(
        OWNER,
        "owner",
        &SpaceNew {
            label: "Atlas".into(),
            parent: Some(sp("work")),
            ..Default::default()
        },
    )?;
    let ctl = b.open_control()?;
    access::connect(
        &ctl,
        "helper",
        &Rights::parse("work:rw")?,
        None,
        false,
        false,
    )?;
    drop(ctl);

    let dry = b.delete_space(
        OWNER,
        &sp("work"),
        &Disposition::Into(sp("home")),
        &Children::Grandparent,
        true,
        1_000,
    )?;
    assert!(dry.dry_run && dry.facts == 4 && dry.files == 1, "{dry:?}");
    assert_eq!(dry.connections, ["helper"]);
    assert_eq!(dry.children, [(child.to_string(), None)]);
    assert!(b.has_space(&sp("work"))?, "a dry run changes nothing");
    assert_eq!(active(&b, "home")?, 1);

    let done = b.delete_space(
        OWNER,
        &sp("work"),
        &Disposition::Into(sp("home")),
        &Children::Grandparent,
        false,
        1_000,
    )?;
    assert!(!done.dry_run);
    assert!(!b.has_space(&sp("work"))?);
    assert!(!b.dir().join("work.sqlite").exists());
    // 1 own + 2 notes + the file note: more than before, none lost.
    assert!(active(&b, "home")? >= 3);
    let home = b.open_space(&sp("home"))?;
    let found: i64 = home.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE 'Invoice sent%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(found, 1);
    let tagged: i64 = home.query_row(
        "SELECT count(*) FROM fact_tags WHERE tag = 'clients'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(tagged, 1, "tags travel with the notes");
    assert_eq!(
        memcp_core::files::list(&b, &sp("home"), None, 10)?.len(),
        1,
        "the file moved"
    );
    let r = rules::read(&home).1;
    assert!(
        r.keywords.contains(&"atlas".to_string()) && r.keywords.contains(&"work".to_string()),
        "the keywords join the target's: {r:?}"
    );
    // The child moved up to the top and the connection lost the space.
    let places = b.layout(OWNER)?;
    let c = places.iter().find(|p| p.space == child).ok_or("child")?;
    assert_eq!(c.parent, None);
    let ctl = b.open_control()?;
    let g: i64 = ctl.query_row(
        "SELECT count(*) FROM grants WHERE space = 'work'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(g, 0);
    // The original is kept: it can be listed and restored.
    let archived = b.archived_spaces(OWNER)?;
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].into.as_deref(), Some("home"));
    assert_eq!(
        ctl.query_row(
            "SELECT count(*) FROM auth_audit WHERE event = 'space_delete'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    Ok(())
}

#[test]
fn an_archive_is_a_full_export_and_the_space_comes_back_with_restore() -> Res {
    let t = Temp::new("life");
    let b = brain(&t)?;
    note(&b, "health", "Slept well", &["sleep"])?;
    memcp_core::files::put_tagged(&b, &sp("health"), "owner", "scan.txt", "", b"data", &[])?;
    b.set_layout(
        OWNER,
        &sp("health"),
        &Change {
            label: Some("My health".into()),
            ..Default::default()
        },
    )?;
    let ctl = b.open_control()?;
    access::connect(
        &ctl,
        "reader",
        &Rights::parse("health:r")?,
        None,
        false,
        false,
    )?;
    drop(ctl);
    b.delete_space(
        OWNER,
        &sp("health"),
        &Disposition::Archive,
        &Children::Grandparent,
        false,
        2_000,
    )?;
    assert!(!b.has_space(&sp("health"))?);
    let list = b.archived_spaces(OWNER)?;
    assert_eq!(list.len(), 1);
    let dir = b.archive_root().join(&list[0].name);
    assert!(
        dir.join("live/health.sqlite").is_file(),
        "moved aside, not erased"
    );
    assert!(dir.join("live/files").is_dir());
    let export = std::fs::read_dir(dir.join("export"))?
        .next()
        .ok_or("export")??
        .path();
    memcp_core::export::verify_export(&export)?;
    // Another space cannot take the name while restoring is pending, and a stranger cannot restore.
    assert!(
        b.restore_space("someone-else", &list[0].name, false)
            .is_err()
    );
    let back = b.restore_space(OWNER, &list[0].name, false)?;
    assert_eq!(back, sp("health"));
    assert_eq!(active(&b, "health")?, 2, "the note and the file note");
    let info = b.space_info(OWNER, &back)?;
    assert_eq!(info.label, "My health");
    assert_eq!(memcp_core::files::list(&b, &back, None, 10)?.len(), 1);
    assert!(b.archived_spaces(OWNER)?.is_empty());
    let ctl = b.open_control()?;
    let g: i64 = ctl.query_row(
        "SELECT count(*) FROM grants WHERE space = 'health'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(g, 1, "the connection that reached it reaches it again");
    // Restoring twice is refused.
    assert!(b.restore_space(OWNER, &list[0].name, false).is_err());
    Ok(())
}

#[test]
fn a_connection_limited_to_paths_comes_back_limited_after_a_restore() -> Res {
    let t = Temp::new("life");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let token = access::connect(
        &ctl,
        "sources",
        &Rights::parse("health/sources/scale:rw")?,
        None,
        false,
        false,
    )?;
    let before = access::authenticate(&ctl, &token, memcp_core::util::now())?.ok_or("caller")?;
    assert!(
        before.read_paths(&sp("health")).is_some(),
        "limited to paths"
    );
    drop(ctl);
    b.delete_space(
        OWNER,
        &sp("health"),
        &Disposition::Archive,
        &Children::Grandparent,
        false,
        2_000,
    )?;
    let list = b.archived_spaces(OWNER)?;
    b.restore_space(OWNER, &list[0].name, false)?;
    let ctl = b.open_control()?;
    let after = access::authenticate(&ctl, &token, memcp_core::util::now())?.ok_or("caller")?;
    assert_eq!(
        after.read_paths(&sp("health")),
        before.read_paths(&sp("health")),
        "the same paths, not the whole space"
    );
    Ok(())
}

/// A reader still open on the space keeps the last writes in the write-ahead log, out of the file: archiving and
/// restoring keeps them.
#[cfg(unix)]
#[test]
fn an_archive_keeps_writes_still_in_the_log() -> Res {
    let t = Temp::new("life");
    let b = brain(&t)?;
    note(&b, "health", "Slept well", &["sleep"])?;
    let reader = b.open_space(&sp("health"))?;
    reader.execute_batch("BEGIN")?;
    let _: i64 = reader.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    note(&b, "health", "Walked an hour", &["walk"])?;
    b.delete_space(
        OWNER,
        &sp("health"),
        &Disposition::Archive,
        &Children::Grandparent,
        false,
        2_000,
    )?;
    drop(reader);
    let list = b.archived_spaces(OWNER)?;
    b.restore_space(OWNER, &list[0].name, false)?;
    assert_eq!(active(&b, "health")?, 2, "both notes");
    Ok(())
}

#[test]
fn children_go_to_the_grandparent_by_default_and_one_cannot_delete_what_is_not_theirs() -> Res {
    let t = Temp::new("life");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let alex = users::create_user(&ctl, OWNER, "alex", "Alex", false)?.id;
    drop(ctl);
    let mid = b.create_space_with(
        OWNER,
        "owner",
        &SpaceNew {
            label: "Mid".into(),
            parent: Some(sp("work")),
            ..Default::default()
        },
    )?;
    let leaf = b.create_space_with(
        OWNER,
        "owner",
        &SpaceNew {
            label: "Leaf".into(),
            parent: Some(mid.clone()),
            ..Default::default()
        },
    )?;
    // A member cannot read, change or delete another person's space.
    assert!(b.space_info(&alex, &mid).is_err());
    assert!(
        b.delete_space(&alex, &mid, &Disposition::Archive, &Children::Top, false, 5)
            .is_err()
    );
    assert!(
        b.update_space(&alex, "alex", &mid, &SpaceUpdate::default())
            .is_err()
    );
    // Nor can they send notes into it.
    assert!(
        b.delete_space(
            OWNER,
            &leaf,
            &Disposition::Into(leaf.clone()),
            &Children::Top,
            true,
            5
        )
        .is_err()
    );
    b.delete_space(
        OWNER,
        &mid,
        &Disposition::Archive,
        &Children::default(),
        false,
        5,
    )?;
    let l = b.layout(OWNER)?;
    let moved = l.iter().find(|p| p.space == leaf).ok_or("leaf")?;
    assert_eq!(moved.parent, Some(sp("work")), "up to the grandparent");
    // « À trier » is never deleted.
    let tri = memcp_core::triage::ensure(&b, OWNER, "owner")?;
    assert!(
        b.delete_space(OWNER, &tri, &Disposition::Archive, &Children::Top, true, 5)
            .is_err()
    );
    Ok(())
}
