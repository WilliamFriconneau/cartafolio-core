//! Crash safety of the life of a space: making one, archiving or deleting it into another, restoring it, changing its
//! layout, merging, adopting an export. An operation stopped after any of its steps (as a crash, a `kill -9` or a power
//! loss would stop it) ends done once or not done once the node starts again (`Brain::recover`): the space is usable or
//! absent, never both or neither; an archive can be restored; a peer never brings an archived space back; a merge
//! finished after a crash doubles nothing. See docs/CRASH.md.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Res, Temp, sp};
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::failpoint::{self, at};
use pensine_core::files;
use pensine_core::layout::Change;
use pensine_core::lifecycle::{Children, Disposition, SpaceNew};
use pensine_core::moves::{self, Step};
use pensine_core::swarm::{self, Answer};
use pensine_core::text::Language;
use pensine_core::users::{self, OWNER};
use pensine_core::{Brain, Space, catalogue, export, merge, sync, triage};

type Out<T> = Result<T, Box<dyn std::error::Error>>;

fn node(t: &Temp, name: &str) -> Out<Brain> {
    Ok(Brain::init(
        &t.path().join(name),
        &[sp("notes"), sp("home")],
        Language::English,
    )?)
}

/// Stops the operation at `point`, then starts the node again: what a crash there leaves, recovered.
fn crash<T>(
    b: &Brain,
    point: &'static str,
    op: impl FnOnce() -> pensine_core::Result<T>,
) -> Out<(Brain, pensine_core::recover::Recovered)> {
    failpoint::arm(Some(point));
    let r = op();
    failpoint::arm(None);
    let e = r.err().ok_or("the operation went through its failpoint")?;
    assert!(e.to_string().contains("failpoint"), "{point}: {e}");
    let b = Brain::open(b.dir())?;
    let r = b.recover()?;
    assert!(r.unsettled.is_empty(), "{point}: {r:?}");
    assert!(r.stray.is_empty(), "{point}: {r:?}");
    Ok((b, r))
}

fn say(b: &Brain, space: &Space, text: &str) -> Out<String> {
    let conn = b.open_space(space)?;
    match facts::remember(&conn, space, "me", NewFact::agent(text, "agent:t"), &[])? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn active(b: &Brain, space: &Space, like: &str) -> Out<i64> {
    Ok(b.open_space(space)?.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE ?1",
        [like],
        |r| r.get(0),
    )?)
}

/// The space is registered and usable, its file in place, and no archive of it is under way; or it is absent, its file
/// gone from the data folder, and exactly one archive (not under way) holds it.
fn usable_or_archived(b: &Brain, space: &Space) -> Out<bool> {
    let file = b.dir().join(format!("{space}.sqlite"));
    let wal = b.dir().join(format!("{space}.sqlite-wal"));
    let archives: Vec<_> = std::fs::read_dir(b.archive_root())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(e.path().join("archive.json")).ok()?)
                    .ok()?;
            (v["space"] == space.as_str()).then_some((e.path(), v))
        })
        .collect();
    for (_, v) in &archives {
        assert!(
            v["state"].as_str().is_none_or(|s| s == "archived"),
            "an archive left under way: {v}"
        );
    }
    if b.has_space(space)? {
        assert!(file.is_file(), "registered without its file");
        b.open_space(space)?;
        assert!(archives.is_empty(), "registered and archived");
        Ok(true)
    } else {
        assert!(
            !file.exists() && !wal.exists(),
            "unregistered, its file left"
        );
        assert_eq!(archives.len(), 1, "absent and archived nowhere");
        let live = archives[0].0.join("live").join(format!("{space}.sqlite"));
        assert!(live.is_file(), "the archive holds no database");
        Ok(false)
    }
}

// --- making a space --------------------------------------------------------------------------------------------------

#[test]
fn a_creation_cut_off_leaves_no_space_and_the_name_free() -> Res {
    let t = Temp::new("crash-create");
    let b = node(&t, "a")?;
    // A vault: its name is the person's and the kind's, always the same.
    let (b, r) = crash(&b, at::SPACE_CREATE_FILE, || {
        b.add_vault(OWNER, "owner", "health")
    })?;
    assert_eq!(r.orphans, 1, "{r:?}");
    let vault = catalogue::vault_name("owner", "health")?;
    assert!(!b.has_space(&vault)? && !b.dir().join(format!("{vault}.sqlite")).exists());
    assert_eq!(b.add_vault(OWNER, "owner", "health")?, vault);
    assert_eq!(b.space_kind(&vault)?, "health");
    // Without a start between: the next creation of the name clears what the cut-off one left.
    failpoint::arm(Some(at::SPACE_CREATE_FILE));
    let cut = triage::ensure(&b, OWNER, "owner");
    failpoint::arm(None);
    assert!(cut.is_err());
    let inbox = triage::ensure(&b, OWNER, "owner")?;
    assert!(triage::is_triage(&b, &inbox), "made a box at once");
    // A space of the person's own, with keywords: all of it, or none.
    let new = SpaceNew {
        kind: Some("work".into()),
        label: "Clients".into(),
        keywords: vec!["atlas".into()],
        ..Default::default()
    };
    let (b, _) = crash(&b, at::SPACE_CREATE_FILE, || {
        b.create_space_with(OWNER, "owner", &new)
    })?;
    assert!(b.layout(OWNER)?.iter().all(|p| p.label != "Clients"));
    let s = b.create_space_with(OWNER, "owner", &new)?;
    let info = b.space_info(OWNER, &s)?;
    assert_eq!(
        (info.kind.as_str(), info.rules.keywords.clone()),
        ("work", vec!["atlas".to_string()])
    );
    let labels: i64 = b.open_space(&s)?.query_row(
        "SELECT count(*) FROM sync_log WHERE kind = 'layout_changed' AND eid LIKE 'l:label:%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(labels, 1, "its label is in its log from the start");
    Ok(())
}

#[test]
fn a_space_file_registered_nowhere_that_holds_notes_is_kept_and_reported() -> Res {
    let t = Temp::new("crash-stray");
    let b = node(&t, "a")?;
    say(&b, &sp("home"), "2026-11-19, the gate code changed.")?;
    // As if the control rows were lost: the file stays, with its note.
    b.open_control()?.execute_batch(
        "DELETE FROM space_members WHERE space = 'home'; DELETE FROM spaces WHERE name = 'home';",
    )?;
    let r = b.recover()?;
    assert_eq!(r.stray, vec!["home.sqlite".to_string()], "{r:?}");
    assert!(b.dir().join("home.sqlite").is_file(), "never removed");
    Ok(())
}

#[test]
fn a_registered_space_whose_file_went_gets_an_empty_one() -> Res {
    let t = Temp::new("crash-heal");
    let b = node(&t, "a")?;
    std::fs::remove_file(b.dir().join("home.sqlite"))?;
    let conn = b.open_space(&sp("home"))?;
    let n: i64 = conn.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    assert_eq!(n, 0);
    say(&b, &sp("home"), "2026-11-19, written after the heal.")?;
    Ok(())
}

// --- archiving and restoring -----------------------------------------------------------------------------------------

/// Five notes that say different things, each ending with a mark the tests count by.
const HOUSE: [&str; 5] = [
    "2026-11-19, the roses need pruning in March [kept].",
    "2026-11-19, boiler pressure gets checked every autumn [kept].",
    "2026-11-19, gutters are cleaned before winter storms [kept].",
    "2026-11-19, fence paint colour: dark green, brand Tollens [kept].",
    "2026-11-19, outdoor tap freezes below zero, close valve [kept].",
];

fn with_notes_and_a_file(b: &Brain, space: &Space) -> Out<String> {
    for text in HOUSE {
        say(b, space, text)?;
    }
    Ok(files::put(
        b,
        space,
        "device",
        "plan.txt",
        "",
        b"the plan of the garden",
    )?
    .id)
}

#[test]
fn an_archive_cut_off_at_any_step_ends_archived_and_restores_whole() -> Res {
    for point in [
        at::ARCHIVE_JOURNAL,
        at::ARCHIVE_UNREGISTERED,
        at::ARCHIVE_DB,
    ] {
        let t = Temp::new("crash-archive");
        let b = node(&t, "a")?;
        let home = sp("home");
        let file = with_notes_and_a_file(&b, &home)?;
        let (b, r) = crash(&b, point, || {
            b.delete_space(
                OWNER,
                &home,
                &Disposition::Archive,
                &Children::Grandparent,
                false,
                1_900_000_000,
            )
        })?;
        assert!(r.spaces >= 1, "{point}: {r:?}");
        assert!(!usable_or_archived(&b, &home)?, "{point}: archived");
        assert!(
            swarm::archived_at(&b, &home)?.is_some(),
            "{point}: the mark"
        );
        let list = b.archived_spaces(OWNER)?;
        assert_eq!(list.len(), 1, "{point}");
        b.restore_space(OWNER, &list[0].name, false)?;
        assert!(usable_or_archived(&b, &home)?);
        assert_eq!(active(&b, &home, "%[kept].")?, 5, "{point}");
        assert!(files::get(&b, &home, &file)?.is_some(), "{point}: the file");
    }
    Ok(())
}

#[test]
fn a_restore_cut_off_at_any_step_ends_restored() -> Res {
    for point in [
        at::RESTORE_JOURNAL,
        at::RESTORE_DB,
        at::RESTORE_FILES,
        at::RESTORE_REGISTERED,
    ] {
        let t = Temp::new("crash-restore");
        let b = node(&t, "a")?;
        let home = sp("home");
        let file = with_notes_and_a_file(&b, &home)?;
        b.delete_space(
            OWNER,
            &home,
            &Disposition::Archive,
            &Children::Grandparent,
            false,
            1_900_000_000,
        )?;
        let name = b.archived_spaces(OWNER)?[0].name.clone();
        let (b, _) = crash(&b, point, || b.restore_space(OWNER, &name, false))?;
        assert!(usable_or_archived(&b, &home)?, "{point}: restored");
        assert!(b.archived_spaces(OWNER)?.is_empty(), "{point}");
        assert_eq!(active(&b, &home, "%[kept].")?, 5, "{point}");
        assert!(files::get(&b, &home, &file)?.is_some(), "{point}: the file");
        assert!(swarm::archived_at(&b, &home)?.is_none(), "{point}: back");
    }
    Ok(())
}

#[test]
fn a_deletion_into_another_space_cut_off_copies_once_and_archives() -> Res {
    for point in [
        at::ARCHIVE_JOURNAL,
        at::ARCHIVE_MERGED,
        at::ARCHIVE_UNREGISTERED,
        at::ARCHIVE_DB,
    ] {
        let t = Temp::new("crash-into");
        let b = node(&t, "a")?;
        let (home, notes) = (sp("home"), sp("notes"));
        with_notes_and_a_file(&b, &home)?;
        let (b, _) = crash(&b, point, || {
            b.delete_space(
                OWNER,
                &home,
                &Disposition::Into(notes.clone()),
                &Children::Grandparent,
                false,
                1_900_000_000,
            )
        })?;
        assert!(!usable_or_archived(&b, &home)?, "{point}");
        assert_eq!(active(&b, &notes, "%[kept].")?, 5, "{point}: once each");
        let files: i64 = b.open_space(&notes)?.query_row(
            "SELECT count(*) FROM files WHERE status = 'active'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(files, 1, "{point}");
    }
    Ok(())
}

/// The cut-off archive of an earlier version: the files moved aside, the control transaction never committed. The
/// space stays, its files come back.
#[test]
fn an_archive_an_earlier_version_left_half_done_is_undone() -> Res {
    let t = Temp::new("crash-legacy-archive");
    let b = node(&t, "a")?;
    let home = sp("home");
    with_notes_and_a_file(&b, &home)?;
    let dir = b.archive_root().join("home-20330518T033320");
    std::fs::create_dir_all(dir.join("live"))?;
    std::fs::write(
        dir.join("archive.json"),
        serde_json::json!({ "format": "memcp-archived-space", "version": 1, "space": "home", "owner": OWNER,
                            "archived_at": 2_000_000_000i64, "label": "home", "kind": "custom" })
        .to_string(),
    )?;
    {
        let c = b.open_space(&home)?;
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    }
    std::fs::rename(
        b.dir().join("home.sqlite"),
        dir.join("live").join("home.sqlite"),
    )?;
    let r = b.recover()?;
    assert_eq!(r.spaces, 1, "{r:?}");
    assert!(usable_or_archived(&b, &home)?);
    assert_eq!(active(&b, &home, "%[kept].")?, 5);
    assert!(dir.join("aborted.json").is_file());
    Ok(())
}

// --- the swarm -------------------------------------------------------------------------------------------------------

fn swarm_node(t: &Temp, name: &str) -> Out<Brain> {
    let b = Brain::init(&t.path().join(name), &[], Language::English)?;
    triage::ensure(&b, OWNER, name)?;
    Ok(b)
}

/// A space archived on one node, with a crash right after its control transaction: a peer that still offers it is
/// answered that it is archived, and never gives it back.
#[test]
fn an_archived_space_is_not_brought_back_by_a_peer_after_a_crash() -> Res {
    for point in [at::ARCHIVE_UNREGISTERED, at::ARCHIVE_DB] {
        let t = Temp::new("crash-swarm-archive");
        let a = swarm_node(&t, "alma")?;
        let b = swarm_node(&t, "bea")?;
        let garden = a.add_custom_space(OWNER, "alma", "Garden", None)?;
        say(&a, &garden, "2026-11-19, the roses are pruned.")?;
        for o in swarm::offers(&a, OWNER)? {
            swarm::accept(&b, OWNER, &o)?;
        }
        assert!(b.has_space(&garden)?);
        let (a, _) = crash(&a, point, || {
            a.delete_space(
                OWNER,
                &garden,
                &Disposition::Archive,
                &Children::Grandparent,
                false,
                pensine_core::util::now(),
            )
        })?;
        let offer = swarm::offers(&b, OWNER)?
            .into_iter()
            .find(|o| o.space == garden)
            .ok_or("bea offers it")?;
        let answer = swarm::accept(&a, OWNER, &offer)?;
        assert!(matches!(answer, Answer::Archived(_)), "{point}: {answer:?}");
        assert!(!a.has_space(&garden)?, "{point}: not brought back");
    }
    Ok(())
}

/// A space made from a peer's offer keeps the time the peer's copy began, from the moment it exists: a crash cannot
/// leave it stamped now (which would beat earlier archives elsewhere and bring archived spaces back).
#[test]
fn a_space_made_from_a_peer_is_made_with_its_time_and_place_or_not_at_all() -> Res {
    let t = Temp::new("crash-swarm-adopt");
    let a = swarm_node(&t, "alma")?;
    let b = swarm_node(&t, "bea")?;
    let hobbies = a.add_custom_space(OWNER, "alma", "Hobbies", None)?;
    let pompiers = a.add_custom_space(OWNER, "alma", "Pompiers", Some(&hobbies))?;
    let offers = swarm::offers(&a, OWNER)?;
    let parent = offers
        .iter()
        .find(|o| o.space == hobbies)
        .ok_or("offered")?;
    swarm::accept(&b, OWNER, parent)?;
    let child = offers
        .iter()
        .find(|o| o.space == pompiers)
        .ok_or("offered")?;
    let (b, _) = crash(&b, at::SPACE_CREATE_FILE, || {
        swarm::accept(&b, OWNER, child)
    })?;
    assert!(!b.has_space(&pompiers)?, "not made");
    assert_eq!(
        swarm::accept(&b, OWNER, child)?,
        Answer::Created(pompiers.clone())
    );
    assert_eq!(swarm::since(&b, &pompiers)?, swarm::since(&a, &pompiers)?);
    let place = b
        .layout(OWNER)?
        .into_iter()
        .find(|p| p.space == pompiers)
        .ok_or("placed")?;
    assert_eq!(
        (place.label.as_str(), place.parent),
        ("Pompiers", Some(hobbies))
    );
    Ok(())
}

// --- layout ----------------------------------------------------------------------------------------------------------

fn label_events(b: &Brain, space: &Space, label: &str) -> Out<i64> {
    Ok(b.open_space(space)?.query_row(
        "SELECT count(*) FROM sync_log WHERE kind = 'layout_changed' AND eid LIKE 'l:label:%'
         AND json_extract(payload, '$.value') = ?1",
        [label],
        |r| r.get(0),
    )?)
}

#[test]
fn a_layout_change_cut_off_is_logged_once_at_start() -> Res {
    for point in [at::LAYOUT_CONTROL, at::LAYOUT_LOGGED] {
        let t = Temp::new("crash-layout");
        let b = node(&t, "a")?;
        let s = b.add_custom_space(OWNER, "owner", "Garden", None)?;
        let (b, r) = crash(&b, point, || {
            b.set_layout(
                OWNER,
                &s,
                &Change {
                    label: Some("Orchard".into()),
                    ..Default::default()
                },
            )
        })?;
        assert_eq!(r.spaces, 1, "{point}: {r:?}");
        assert_eq!(label_events(&b, &s, "Orchard")?, 1, "{point}: once");
        assert_eq!(b.recover()?.spaces, 0, "{point}: nothing left");
    }
    Ok(())
}

// --- merging and adopting --------------------------------------------------------------------------------------------

/// Another memory of `sam` with two vaults, exported; this one has `kim` with a work vault.
fn two_memories(t: &Temp) -> Out<(Brain, users::User, std::path::PathBuf)> {
    let other = Brain::init(&t.path().join("other"), &[], Language::English)?;
    let sam = users::create_user(&other.open_control()?, OWNER, "sam", "Sam", false)?;
    let work = other.add_vault(&sam.id, "sam", "work")?;
    let health = other.add_vault(&sam.id, "sam", "health")?;
    for text in [
        "2026-11-19, the audit is due on Friday [work item].",
        "2026-11-19, invoices go out monthly via Odoo [work item].",
        "2026-11-19, client meeting moved to Tuesday afternoon [work item].",
    ] {
        say(&other, &work, text)?;
    }
    for text in [
        "2026-11-19, resting heart rate was 58 [health item].",
        "2026-11-19, flu vaccine booked for December [health item].",
        "2026-11-19, physiotherapy twice weekly for the knee [health item].",
    ] {
        say(&other, &health, text)?;
    }
    files::put(
        &other,
        &work,
        "device",
        "contract.txt",
        "",
        b"the contract text",
    )?;
    let exported = export::export_person(&other, &sam, &t.path().join("out"), 1_900_000_000)?;
    let mine = Brain::init(&t.path().join("mine"), &[], Language::English)?;
    let kim = users::create_user(&mine.open_control()?, OWNER, "kim", "Kim", false)?;
    mine.add_vault(&kim.id, "kim", "work")?;
    Ok((mine, kim, exported))
}

#[test]
fn a_merge_cut_off_is_finished_at_start_and_doubles_nothing() -> Res {
    for point in [at::MERGE_SPACES, at::MERGE_TARGET] {
        let t = Temp::new("crash-merge");
        let (mine, kim, exported) = two_memories(&t)?;
        let mut plan = merge::plan(&mine, &exported, &kim, merge::Policy::KeepBoth)?;
        for l in &mut plan.lines {
            if l.source == "sam-work" {
                l.action = merge::Action::Merge;
                l.target = "kim-work".into();
            } else {
                l.action = merge::Action::Copy;
                l.target = "kim-health".into();
            }
        }
        let opts = merge::Options {
            backup_to: t.path().join("backups"),
            at: 1_900_000_100,
            stop_after: None,
        };
        let (mine, r) = crash(&mine, point, || merge::apply(&mine, &plan, &opts))?;
        assert_eq!(r.spaces, 1, "{point}: {r:?}");
        assert_eq!(
            active(&mine, &sp("kim-work"), "%[work item]%")?,
            3,
            "{point}"
        );
        assert_eq!(
            active(&mine, &sp("kim-health"), "%[health item]%")?,
            3,
            "{point}"
        );
        assert_eq!(mine.space_kind(&sp("kim-health"))?, "health", "{point}");
        // Applied again by hand: nothing more.
        merge::apply(&mine, &plan, &opts)?;
        assert_eq!(
            active(&mine, &sp("kim-work"), "%[work item]%")?,
            3,
            "{point}"
        );
    }
    Ok(())
}

#[test]
fn an_adoption_cut_off_leaves_nothing_and_its_retry_adopts_whole() -> Res {
    let t = Temp::new("crash-adopt");
    let (mine, kim, exported) = two_memories(&t)?;
    mine.set_layout(
        &kim.id,
        &sp("kim-work"),
        &Change {
            label: Some("Kim’s work".into()),
            ..Default::default()
        },
    )?;
    let (mine, r) = crash(&mine, at::ADOPT_FILES, || {
        export::adopt(&mine, &exported, &kim, Some("old"))
    })?;
    assert_eq!(r.spaces, 1, "{r:?}");
    assert!(!mine.dir().join("old-sam-work.sqlite").exists());
    assert!(!mine.dir().join("files").join("old-sam-work").exists());
    let done = export::adopt(&mine, &exported, &kim, Some("old"))?;
    assert_eq!(done.len(), 2);
    assert_eq!(active(&mine, &sp("old-sam-work"), "%[work item]%")?, 3);
    let f = files::list(&mine, &sp("old-sam-work"), None, 10)?;
    assert!(files::get(&mine, &sp("old-sam-work"), &f[0].id)?.is_some());
    Ok(())
}

/// Bytes torn under their final name (an earlier version copied them straight there) are never taken for the file:
/// the next writer of those bytes writes them again.
#[test]
fn a_torn_file_under_its_name_is_written_again() -> Res {
    let t = Temp::new("crash-torn");
    let b = node(&t, "a")?;
    let bytes = b"the whole content of the file, longer than its torn part".to_vec();
    let sha = pensine_core::chunks::digest(&bytes);
    let dir = b.dir().join("files").join("notes");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(&sha), &bytes[..10])?;
    let f = files::put(&b, &sp("notes"), "device", "whole.txt", "", &bytes)?;
    let (_, got) = files::get(&b, &sp("notes"), &f.id)?.ok_or("there")?;
    assert_eq!(got, bytes);
    Ok(())
}

// --- moves and a peer's correction -----------------------------------------------------------------------------------

/// A note under a move cut off is corrected on another node meanwhile; its correction arrives here before the move
/// ends. The move ends with the newest version: it is active once, in the target, and the copy of the old one there is
/// replaced by it.
#[test]
fn a_note_corrected_by_a_peer_while_its_move_was_cut_off_ends_active_once() -> Res {
    let t = Temp::new("crash-move-peer");
    let a = node(&t, "a")?;
    let b = node(&t, "b")?;
    let notes = sp("notes");
    let conn = a.open_space(&notes)?;
    let mut n = NewFact::agent("2026-11-19, the boiler is serviced in March.", "agent:t");
    n.tags = vec!["house".into()];
    let Remembered::Created(f) = facts::remember(&conn, &notes, "me", n, &[])? else {
        return Err("not created".into());
    };
    let copy = |from: &Brain, to: &Brain, peer: &str| -> Res {
        let page = sync::events_after(from, &notes, 0, 10_000, 64 << 20, None)?;
        sync::apply(to, &notes, &page.events, peer)?;
        Ok(())
    };
    copy(&a, &b, "a")?;
    moves::fail_at(Some(Step::AfterCopies));
    let cut = a.move_by_tag(&notes, &sp("home"), "house", "me", false);
    moves::fail_at(None);
    assert!(cut.is_err());
    // On b, the note is corrected; the correction reaches a.
    let fix = NewFact::agent("2026-11-19, the boiler is serviced in April.", "agent:t");
    facts::supersede(&b.open_space(&notes)?, &notes, "me", &f.id, fix, None)?;
    copy(&b, &a, "b")?;
    let a = Brain::open(a.dir())?;
    a.recover()?;
    let home = sp("home");
    assert_eq!(
        active(&a, &notes, "%boiler%")?,
        0,
        "nothing left in the source"
    );
    assert_eq!(active(&a, &home, "%boiler%")?, 1, "once, in the target");
    assert_eq!(active(&a, &home, "%April%")?, 1, "the newest version");
    Ok(())
}

// --- kill -9 ---------------------------------------------------------------------------------------------------------

/// The child of `kill_9_while_a_space_is_archived_and_restored`: archives and restores `home` until it is killed. Does
/// nothing in a plain test run.
#[test]
fn child_archiving_and_restoring() -> Res {
    let Ok(dir) = std::env::var("MEMCP_CRASH_SPACES_CHILD_DIR") else {
        return Ok(());
    };
    let b = Brain::open(Path::new(&dir))?;
    let home = sp("home");
    for i in 0..10_000 {
        if b.has_space(&home)? {
            b.delete_space(
                OWNER,
                &home,
                &Disposition::Archive,
                &Children::Grandparent,
                false,
                1_900_000_000 + i,
            )?;
        } else if let Some(a) = b.archived_spaces(OWNER)?.first() {
            b.restore_space(OWNER, &a.name, false)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn kill_9_while_a_space_is_archived_and_restored() -> Res {
    let t = Temp::new("crash-spaces-kill");
    let b = node(&t, "a")?;
    let home = sp("home");
    let file = with_notes_and_a_file(&b, &home)?;
    let exe = std::env::current_exe()?;
    for wait_ms in [40u64, 90, 160, 260, 420, 700, 1000] {
        let mut child = std::process::Command::new(&exe)
            .args([
                "child_archiving_and_restoring",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MEMCP_CRASH_SPACES_CHILD_DIR", b.dir())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        std::thread::sleep(Duration::from_millis(wait_ms));
        child.kill()?; // SIGKILL
        child.wait()?;
        let b = Brain::open(b.dir())?;
        let r = b.recover()?;
        assert!(r.unsettled.is_empty() && r.stray.is_empty(), "{r:?}");
        if !usable_or_archived(&b, &home)? {
            let name = b.archived_spaces(OWNER)?[0].name.clone();
            b.restore_space(OWNER, &name, false)?;
        }
        assert_eq!(active(&b, &home, "%[kept].")?, 5, "after {wait_ms} ms");
        assert!(
            files::get(&b, &home, &file)?.is_some(),
            "after {wait_ms} ms"
        );
    }
    Ok(())
}
