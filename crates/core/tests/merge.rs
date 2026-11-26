//! Merging two memories: a plan that writes nothing, then an apply that backs up first, never
//! deletes, changes nothing twice, and is all or nothing.

mod common;

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use common::{Res, Temp, sp};
use pensine_core::Brain;
use pensine_core::export;
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::files;
use pensine_core::guide;
use pensine_core::merge::{self, Action, Options, Plan, Policy};
use pensine_core::text::Language;
use pensine_core::users::{self, OWNER, User};

struct World {
    t: Temp,
    /// The memory that is merged in.
    other: Brain,
    sam: User,
    /// This installation.
    mine: Brain,
    kim: User,
    export: PathBuf,
}

fn say(brain: &Brain, space: &str, text: &str, tags: &[&str]) -> Result<String, Box<dyn Error>> {
    let conn = brain.open_space(&sp(space))?;
    let mut n = NewFact::agent(text, "agent:test");
    n.tags = tags.iter().map(|t| t.to_string()).collect();
    match facts::remember(&conn, &sp(space), "tester", n, &[])? {
        Remembered::Created(f) => Ok(f.id),
        _ => Err("not created".into()),
    }
}

fn texts(brain: &Brain, space: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let conn = brain.open_space(&sp(space))?;
    let mut st = conn.prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")?;
    Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
}

/// Every fact row and the event count, to see that nothing moved.
fn digest(brain: &Brain, space: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let c = brain.open_space(&sp(space))?;
    let mut st = c.prepare(
        "SELECT f.id || '|' || f.text || '|' || f.status || '|' || coalesce(f.replaced_by, '') || '|' ||
                coalesce((SELECT group_concat(tag) FROM (SELECT tag FROM fact_tags t WHERE t.fact_id = f.id ORDER BY tag)), '')
         FROM facts f ORDER BY f.id",
    )?;
    let mut rows: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let events: i64 = c.query_row("SELECT count(*) FROM fact_events", [], |r| r.get(0))?;
    rows.push(format!("events {events}"));
    Ok(rows)
}

/// The other memory (person `sam`, vaults `sam-work` and `sam-health`) is exported later by each
/// test; this installation has `kim` with `kim-work`.
fn world() -> Result<World, Box<dyn Error>> {
    let t = Temp::new("merge");
    let other = Brain::init(&t.path().join("other"), &[], Language::English)?;
    let sam = users::create_user(&other.open_control()?, OWNER, "sam", "Sam", false)?;
    other.add_vault(&sam.id, "sam", "work")?;
    other.add_vault(&sam.id, "sam", "health")?;
    let mine = Brain::init(&t.path().join("mine"), &[], Language::English)?;
    let kim = users::create_user(&mine.open_control()?, OWNER, "kim", "Kim", false)?;
    mine.add_vault(&kim.id, "kim", "work")?;
    Ok(World {
        t,
        other,
        sam,
        mine,
        kim,
        export: PathBuf::new(),
    })
}

fn export_now(w: &mut World, tag: &str, at: i64) -> Res {
    w.export = export::export_person(&w.other, &w.sam, &w.t.path().join(tag), at)?;
    Ok(())
}

/// `mine` has adopted a first export of the other memory, so both sides share the ids of the
/// notes: two copies of one memory that then drift.
fn drifted(seed: impl Fn(&Brain) -> Res) -> Result<World, Box<dyn Error>> {
    let t = Temp::new("merge-drift");
    let other = Brain::init(&t.path().join("other"), &[], Language::English)?;
    let sam = users::create_user(&other.open_control()?, OWNER, "sam", "Sam", false)?;
    other.add_vault(&sam.id, "sam", "work")?;
    seed(&other)?;
    let first = export::export_person(&other, &sam, &t.path().join("first"), 1_900_000_000)?;
    let mine = Brain::init(&t.path().join("mine"), &[], Language::English)?;
    let kim = users::create_user(&mine.open_control()?, OWNER, "kim", "Kim", false)?;
    export::adopt(&mine, &first, &kim, None)?;
    Ok(World {
        t,
        other,
        sam,
        mine,
        kim,
        export: first,
    })
}

fn opts(w: &World, at: i64) -> Options {
    Options {
        backup_to: w.t.path().join("backups"),
        at,
        stop_after: None,
    }
}

fn plan_of(w: &World, policy: Policy) -> Result<Plan, Box<dyn Error>> {
    Ok(merge::plan(&w.mine, &w.export, &w.kim, policy)?)
}

fn line<'a>(p: &'a mut Plan, source: &str) -> Result<&'a mut merge::Line, Box<dyn Error>> {
    p.lines
        .iter_mut()
        .find(|l| l.source == source)
        .ok_or_else(|| "no such line".into())
}

#[test]
fn the_plan_proposes_a_table_and_writes_nothing() -> Res {
    let mut w = world()?;
    say(
        &w.other,
        "sam-work",
        "The audit is due on Friday.",
        &["work/audit"],
    )?;
    say(&w.other, "sam-health", "Resting heart rate was 58.", &[])?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let before = digest(&w.mine, "kim-work")?;
    let spaces_before = w.mine.spaces()?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    assert_eq!(p.lines.len(), 2);
    // Same kind, another name: offered, and the note says to check.
    let work = p
        .lines
        .iter()
        .find(|l| l.source == "sam-work")
        .ok_or("work")?;
    assert_eq!(work.action, Action::Merge);
    assert_eq!(work.target, "kim-work");
    assert!(work.note.contains("check"));
    assert_eq!(work.preview.new, 1);
    // No match: a new space.
    let health = p
        .lines
        .iter()
        .find(|l| l.source == "sam-health")
        .ok_or("health")?;
    assert_eq!(health.action, Action::Copy);
    assert_eq!(health.target, "sam-health");
    assert_eq!(health.preview.new, 1);
    // The text form reads back the same, one line of text per space.
    assert_eq!(Plan::from_text(&p.to_text())?, p);
    assert_eq!(
        p.to_text()
            .lines()
            .filter(|l| l.contains("\"source\""))
            .count(),
        2
    );
    assert_eq!(digest(&w.mine, "kim-work")?, before);
    assert_eq!(w.mine.spaces()?, spaces_before);
    assert!(!w.t.path().join("backups").exists());
    Ok(())
}

#[test]
fn a_space_merges_into_a_space_of_another_name() -> Res {
    let mut w = world()?;
    say(
        &w.other,
        "sam-work",
        "The audit is due on Friday.",
        &["work/audit"],
    )?;
    say(
        &w.mine,
        "kim-work",
        "The team meets on Tuesdays.",
        &["work/meetings"],
    )?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    let l = p
        .lines
        .iter()
        .find(|l| l.source == "sam-work")
        .ok_or("work")?;
    assert_eq!(l.target, "kim-work");
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert!(r.changed());
    assert_eq!(
        texts(&w.mine, "kim-work")?,
        vec!["The audit is due on Friday.", "The team meets on Tuesdays."]
    );
    let conn = w.mine.open_space(&sp("kim-work"))?;
    let tag: String = conn.query_row(
        "SELECT t.tag FROM fact_tags t JOIN facts f ON f.id = t.fact_id WHERE f.text LIKE 'The audit%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(tag, "work/audit");
    Ok(())
}

#[test]
fn a_copy_creates_the_space_with_its_kind_owned_by_the_person() -> Res {
    let mut w = world()?;
    say(
        &w.other,
        "sam-health",
        "Resting heart rate was 58.",
        &["health/vitals"],
    )?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let mut p = plan_of(&w, Policy::KeepBoth)?;
    line(&mut p, "sam-health")?.rename = Some("kim-health".into());
    merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    let health = sp("kim-health");
    assert!(w.mine.has_space(&health)?);
    assert_eq!(w.mine.space_kind(&health)?, "health");
    assert_eq!(
        texts(&w.mine, "kim-health")?,
        vec!["Resting heart rate was 58."]
    );
    let ctl = w.mine.open_control()?;
    let owner: i64 = ctl.query_row(
        "SELECT owner FROM space_members WHERE space = 'kim-health' AND user_id = ?1",
        [&w.kim.id],
        |r| r.get(0),
    )?;
    assert_eq!(owner, 1);
    Ok(())
}

#[test]
fn the_same_note_is_not_copied_twice_and_tags_merge() -> Res {
    let mut w = drifted(|b| {
        say(
            b,
            "sam-work",
            "The audit is due on Friday.",
            &["work/audit"],
        )?;
        say(b, "sam-work", "Meetings are on Tuesdays.", &[])?;
        Ok(())
    })?;
    // Here, a note has an extra tag; there, another has one, and a third note is new.
    w.mine.open_space(&sp("sam-work"))?.execute(
        "INSERT INTO fact_tags (fact_id, tag) SELECT id, 'mine/only' FROM facts WHERE text LIKE 'The audit%'",
        [],
    )?;
    say(
        &w.other,
        "sam-work",
        "A new note for the merge.",
        &["work/new"],
    )?;
    w.other.open_space(&sp("sam-work"))?.execute(
        "INSERT INTO fact_tags (fact_id, tag) SELECT id, 'theirs/only' FROM facts WHERE text LIKE 'Meetings%'",
        [],
    )?;
    export_now(&mut w, "second", 1_900_000_100)?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    let l = &p.lines[0];
    assert_eq!(l.action, Action::Merge);
    assert_eq!(
        (l.preview.new, l.preview.already, l.preview.differing),
        (1, 2, 0)
    );
    assert_eq!(l.preview.retag, 1);
    merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert_eq!(texts(&w.mine, "sam-work")?.len(), 3);
    let conn = w.mine.open_space(&sp("sam-work"))?;
    let tags = |like: &str| -> Result<Vec<String>, Box<dyn Error>> {
        let mut st = conn.prepare(
            "SELECT t.tag FROM fact_tags t JOIN facts f ON f.id = t.fact_id WHERE f.text LIKE ?1 ORDER BY t.tag",
        )?;
        Ok(st
            .query_map([like], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    };
    assert_eq!(tags("The audit%")?, vec!["mine/only", "work/audit"]);
    assert_eq!(tags("Meetings%")?, vec!["theirs/only"]);
    Ok(())
}

/// Two copies of one note that drifted: the same id, two texts.
fn conflict_world() -> Result<World, Box<dyn Error>> {
    let w = drifted(|b| {
        say(
            b,
            "sam-work",
            "The audit is due on Friday.",
            &["work/audit"],
        )?;
        say(b, "sam-work", "The printer is in room four.", &[])?;
        Ok(())
    })?;
    w.mine.open_space(&sp("sam-work"))?.execute(
        "UPDATE facts SET text = 'The audit moved to Monday.', text_hash = 'h-mine' WHERE text LIKE 'The audit%'",
        [],
    )?;
    Ok(w)
}

#[test]
fn a_conflict_keeps_both_by_default() -> Res {
    let w = conflict_world()?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    assert_eq!(p.lines[0].preview.differing, 1);
    merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert_eq!(
        texts(&w.mine, "sam-work")?,
        vec![
            "The audit is due on Friday.",
            "The audit moved to Monday.",
            "The printer is in room four."
        ]
    );
    Ok(())
}

#[test]
fn a_conflict_can_keep_mine() -> Res {
    let w = conflict_world()?;
    let before = digest(&w.mine, "sam-work")?;
    let p = plan_of(&w, Policy::KeepMine)?;
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert!(!r.changed(), "nothing to write, so no backup either");
    assert_eq!(digest(&w.mine, "sam-work")?, before);
    Ok(())
}

#[test]
fn a_conflict_can_take_theirs_and_mine_goes_to_history() -> Res {
    let w = conflict_world()?;
    let p = plan_of(&w, Policy::TakeTheirs)?;
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert_eq!(r.lines[0].replaced, 1);
    assert_eq!(
        texts(&w.mine, "sam-work")?,
        vec![
            "The audit is due on Friday.",
            "The printer is in room four."
        ]
    );
    // Mine is still there, replaced, with its text, and the history says so.
    let conn = w.mine.open_space(&sp("sam-work"))?;
    let (status, by): (String, String) = conn.query_row(
        "SELECT status, replaced_by FROM facts WHERE text = 'The audit moved to Monday.'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(status, "replaced");
    assert_eq!(facts::history(&conn, &by)?.len(), 2);
    let events: i64 = conn.query_row(
        "SELECT count(*) FROM fact_events WHERE action = 'supersede' AND actor = 'merge'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(events, 1);
    Ok(())
}

#[test]
fn the_target_keeps_its_dashboard_and_guide_unless_replace() -> Res {
    let mut w = drifted(|b| {
        say(
            b,
            "sam-work",
            "Dashboard: three audits open.",
            &["dashboard"],
        )?;
        Ok(())
    })?;
    w.mine.open_space(&sp("sam-work"))?.execute(
        "UPDATE facts SET text = 'Dashboard: all audits closed.', text_hash = 'h2'",
        [],
    )?;
    let scope = guide::space_scope(&sp("sam-work"));
    let ctl = w.mine.open_control()?;
    guide::set(&ctl, &w.kim.id, &scope, "My guide.")?;
    guide::set(&w.other.open_control()?, &w.sam.id, &scope, "Their guide.")?;
    export_now(&mut w, "second", 1_900_000_100)?;
    let mut p = plan_of(&w, Policy::TakeTheirs)?;
    assert_eq!(p.lines[0].guide, "differs");
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert!(!r.changed());
    assert_eq!(
        texts(&w.mine, "sam-work")?,
        vec!["Dashboard: all audits closed."]
    );
    assert_eq!(
        guide::current(&ctl, &scope)?.map(|g| g.body),
        Some("My guide.".into())
    );
    assert_eq!(
        r.guide_proposals,
        vec![("sam-work".to_string(), "Their guide.".to_string())]
    );
    // With replace: both are replaced, the old ones stay in history.
    p.lines[0].policy = Policy::Replace;
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_400))?;
    assert!(r.changed());
    assert_eq!(
        texts(&w.mine, "sam-work")?,
        vec!["Dashboard: three audits open."]
    );
    assert_eq!(
        guide::current(&ctl, &scope)?.map(|g| g.body),
        Some("Their guide.".into())
    );
    assert_eq!(guide::history(&ctl, &scope)?.len(), 2);
    Ok(())
}

#[test]
fn merging_twice_changes_nothing() -> Res {
    for policy in [Policy::KeepBoth, Policy::TakeTheirs, Policy::Replace] {
        let mut w = conflict_world()?;
        say(
            &w.other,
            "sam-work",
            "A new note for the merge.",
            &["work/new"],
        )?;
        export_now(&mut w, "second", 1_900_000_100)?;
        let p = plan_of(&w, policy)?;
        let first = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
        assert!(first.changed());
        let state = digest(&w.mine, "sam-work")?;
        let second = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_301))?;
        assert!(!second.changed(), "{policy:?}: nothing left to do");
        assert_eq!(digest(&w.mine, "sam-work")?, state);
        // A new plan from the same export has nothing left to say either.
        let again = plan_of(&w, policy)?;
        assert_eq!(again.lines[0].preview.new, 0);
        assert_eq!(again.lines[0].preview.differing, 0);
    }
    Ok(())
}

#[test]
fn a_copy_twice_joins_the_space_it_made() -> Res {
    let mut w = world()?;
    say(&w.other, "sam-health", "Resting heart rate was 58.", &[])?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    let health = p
        .lines
        .iter()
        .find(|l| l.source == "sam-health")
        .ok_or("health")?;
    assert_eq!(health.action, Action::Copy);
    merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    let state = digest(&w.mine, "sam-health")?;
    let second = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_301))?;
    assert!(!second.changed());
    assert_eq!(digest(&w.mine, "sam-health")?, state);
    Ok(())
}

#[test]
fn an_interruption_changes_nothing_and_the_merge_can_be_run_again() -> Res {
    let mut w = world()?;
    say(&w.other, "sam-work", "The audit is due on Friday.", &[])?;
    say(&w.other, "sam-health", "Resting heart rate was 58.", &[])?;
    files::put(
        &w.other,
        &sp("sam-health"),
        "sam",
        "note.txt",
        "A note",
        b"hello",
    )?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    assert_eq!(p.lines.len(), 2);
    let before = digest(&w.mine, "kim-work")?;
    let spaces = w.mine.spaces()?;
    let mut o = opts(&w, 1_900_000_300);
    o.stop_after = Some(2);
    let e = merge::apply(&w.mine, &p, &o).err().ok_or("should fail")?;
    assert!(e.to_string().contains("nothing was changed"), "{e}");
    assert_eq!(digest(&w.mine, "kim-work")?, before);
    assert_eq!(w.mine.spaces()?, spaces);
    assert!(!w.mine.dir().join("sam-health.sqlite").exists());
    assert!(!w.mine.dir().join("files").join("sam-health").exists());
    // The backup was taken before, and the merge now goes through.
    assert!(w.t.path().join("backups").is_dir());
    o.stop_after = None;
    o.at = 1_900_000_400;
    let r = merge::apply(&w.mine, &p, &o)?;
    assert_eq!(r.lines.len(), 2);
    assert_eq!(
        texts(&w.mine, "kim-work")?,
        vec!["The audit is due on Friday."]
    );
    assert!(w.mine.has_space(&sp("sam-health"))?);
    Ok(())
}

#[test]
fn a_plan_that_cannot_be_applied_is_refused_before_anything_is_written() -> Res {
    let mut w = world()?;
    say(&w.other, "sam-work", "The audit is due on Friday.", &[])?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let mut p = plan_of(&w, Policy::KeepBoth)?;
    let before = digest(&w.mine, "kim-work")?;
    let edits: [fn(&mut merge::Line); 3] = [
        |l| l.target = "nowhere".into(),
        |l| l.rename = Some("kim-work".into()),
        |l| l.source = "ghost".into(),
    ];
    for edit in edits {
        let mut bad = p.clone();
        edit(line(&mut bad, "sam-work")?);
        assert!(merge::apply(&w.mine, &bad, &opts(&w, 1_900_000_300)).is_err());
    }
    assert_eq!(digest(&w.mine, "kim-work")?, before);
    assert!(!w.t.path().join("backups").exists());
    // A skipped line writes nothing.
    for l in &mut p.lines {
        l.action = Action::Skip;
    }
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert!(!r.changed());
    // An altered export is refused.
    let manifest = w.export.join("manifest.json");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)?.replace("memcp-export", "memcp-exportx"),
    )?;
    assert!(merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300)).is_err());
    Ok(())
}

#[test]
fn a_backup_is_taken_first_and_holds_the_state_before() -> Res {
    let mut w = world()?;
    say(&w.mine, "kim-work", "Existing note.", &[])?;
    say(&w.other, "sam-work", "The audit is due on Friday.", &[])?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    let backup = r.backup.ok_or("a backup")?;
    let copy = rusqlite::Connection::open(backup.join("kim-work.sqlite"))?;
    let n: i64 = copy.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    assert_eq!(n, 1);
    Ok(())
}

#[test]
fn files_come_once_with_their_bytes_and_their_description() -> Res {
    let mut w = world()?;
    files::put(
        &w.other,
        &sp("sam-work"),
        "sam",
        "lease.txt",
        "The flat lease",
        b"Lease text, twelve months.",
    )?;
    say(&w.other, "sam-work", "The audit is due on Friday.", &[])?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let p = plan_of(&w, Policy::KeepBoth)?;
    let work = p
        .lines
        .iter()
        .find(|l| l.source == "sam-work")
        .ok_or("work")?;
    assert_eq!((work.preview.files_new, work.preview.new), (1, 1));
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert_eq!(r.lines.iter().map(|l| l.files).sum::<usize>(), 1);
    let kim_work = sp("kim-work");
    let listed = files::list(&w.mine, &kim_work, None, 10)?;
    assert_eq!(listed.len(), 1);
    let (info, bytes) = files::get(&w.mine, &kim_work, &listed[0].id)?.ok_or("file")?;
    assert_eq!(bytes, b"Lease text, twelve months.");
    assert_eq!(info.name, "lease.txt");
    assert!(texts(&w.mine, "kim-work")?.contains(&"File: lease.txt — The flat lease".to_string()));
    // Again: nothing, not even a second row.
    let again = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_301))?;
    assert!(!again.changed());
    assert_eq!(files::list(&w.mine, &kim_work, None, 10)?.len(), 1);
    let re = plan_of(&w, Policy::KeepBoth)?;
    let work = re
        .lines
        .iter()
        .find(|l| l.source == "sam-work")
        .ok_or("work")?;
    assert_eq!((work.preview.files_new, work.preview.files_already), (0, 1));
    Ok(())
}

#[test]
fn nothing_the_target_retired_comes_back() -> Res {
    let w = drifted(|b| {
        say(b, "sam-work", "The old printer is in room four.", &[])?;
        Ok(())
    })?;
    {
        let conn = w.mine.open_space(&sp("sam-work"))?;
        let id: String = conn.query_row("SELECT id FROM facts", [], |r| r.get(0))?;
        facts::retire(&conn, "kim", &id, "the printer was removed")?;
    }
    let p = plan_of(&w, Policy::Replace)?;
    assert_eq!(p.lines[0].preview.already, 1);
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert!(!r.changed());
    assert!(texts(&w.mine, "sam-work")?.is_empty());
    Ok(())
}

#[test]
fn another_persons_space_cannot_be_merged_into() -> Res {
    let mut w = world()?;
    let ctl = w.mine.open_control()?;
    let lou = users::create_user(&ctl, OWNER, "lou", "Lou", false)?;
    w.mine.add_vault(&lou.id, "lou", "health")?;
    say(&w.other, "sam-work", "The audit is due on Friday.", &[])?;
    export_now(&mut w, "out", 1_900_000_200)?;
    let mut p = plan_of(&w, Policy::KeepBoth)?;
    line(&mut p, "sam-work")?.target = "lou-health".into();
    let e = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))
        .err()
        .ok_or("should fail")?;
    assert!(e.to_string().contains("cannot write"), "{e}");
    Ok(())
}

/// A replica of `space` kept by sync from `from`: everything after its cursor, applied.
fn catch_up(from: &Brain, to: &Brain, space: &str) -> Res {
    let s = sp(space);
    loop {
        let (pulled, _) = pensine_core::sync::cursors(to, "peer-from", &s)?;
        let page = pensine_core::sync::events_after(from, &s, pulled, 50, 1 << 20, None)?;
        let r = pensine_core::sync::apply(to, &s, &page.events, "peer-from")?;
        assert!(r.stopped.is_none(), "{r:?}");
        assert!(r.rejected.is_empty(), "{r:?}");
        pensine_core::sync::set_cursor(
            to,
            "peer-from",
            &s,
            pensine_core::sync::Cursor::Pulled,
            page.next,
        )?;
        if !page.more {
            return Ok(());
        }
    }
}

/// What two replicas must agree on: every note (id, text, kind, status, link, date, author, type, fields, tags) and
/// every file (id, name, digest, status, note, date).
fn replica_state(brain: &Brain, space: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let c = brain.open_space(&sp(space))?;
    let mut st = c.prepare(
        "SELECT f.id || '|' || f.text || '|' || f.kind || '|' || f.status || '|' || coalesce(f.replaced_by, '') || '|' ||
                f.created_at || '|' || f.author || '|' || coalesce(f.type, '') || '|' || coalesce(f.fields, '') || '|' ||
                coalesce((SELECT group_concat(tag) FROM (SELECT tag FROM fact_tags t WHERE t.fact_id = f.id ORDER BY tag)), '')
         FROM facts f ORDER BY f.id",
    )?;
    let mut rows: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut st = c.prepare(
        "SELECT 'file|' || id || '|' || name || '|' || sha256 || '|' || status || '|' || coalesce(fact_id, '') || '|' || created_at
         FROM files ORDER BY id",
    )?;
    rows.extend(
        st.query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(rows)
}

#[test]
fn a_merge_reaches_the_replicas_of_the_target_space() -> Res {
    let mut w = drifted(|b| {
        say(b, "sam-work", "The audit is due on Friday.", &[])?;
        say(b, "sam-work", "Meetings are on Tuesdays.", &[])?;
        Ok(())
    })?;
    // Here, the audit note drifted (same id, another text); the log is started again from what this copy holds.
    {
        let conn = w.mine.open_space(&sp("sam-work"))?;
        conn.execute(
            "UPDATE facts SET text = 'The audit moved to Monday.', text_hash = 'h-mine' WHERE text LIKE 'The audit%'",
            [],
        )?;
        conn.execute_batch(
            "DROP TRIGGER sync_log_no_delete; DELETE FROM sync_log;
             DELETE FROM meta WHERE k IN ('sync_backfilled', 'sync_layout_logged');",
        )?;
    }
    // A replica of this copy's space, in step before the merge.
    let replica = Brain::init(
        &w.t.path().join("replica"),
        &[sp("sam-work")],
        Language::English,
    )?;
    catch_up(&w.mine, &replica, "sam-work")?;
    assert_eq!(
        replica_state(&replica, "sam-work")?,
        replica_state(&w.mine, "sam-work")?
    );

    // There: an old note, a tag on a note both hold, a file. The merge takes theirs for the drifted note.
    let old = say(
        &w.other,
        "sam-work",
        "The boiler was serviced in 2020.",
        &[],
    )?;
    w.other.open_space(&sp("sam-work"))?.execute(
        "UPDATE facts SET created_at = 1600000000 WHERE id = ?1",
        [&old],
    )?;
    w.other.open_space(&sp("sam-work"))?.execute(
        "INSERT INTO fact_tags (fact_id, tag) SELECT id, 'work/meetings' FROM facts WHERE text LIKE 'Meetings%'",
        [],
    )?;
    files::put(
        &w.other,
        &sp("sam-work"),
        "sam",
        "lease.txt",
        "The flat lease",
        b"Lease text, twelve months.",
    )?;
    export_now(&mut w, "second", 1_900_000_100)?;
    let p = plan_of(&w, Policy::TakeTheirs)?;
    let r = merge::apply(&w.mine, &p, &opts(&w, 1_900_000_300))?;
    assert_eq!(
        (r.lines[0].replaced, r.lines[0].tagged, r.lines[0].files),
        (1, 1, 1),
        "{r:?}"
    );

    // The replica, in step again, holds what the merged copy holds.
    catch_up(&w.mine, &replica, "sam-work")?;
    assert_eq!(
        replica_state(&replica, "sam-work")?,
        replica_state(&w.mine, "sam-work")?
    );
    Ok(())
}
