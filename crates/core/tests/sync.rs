//! Peer synchronisation: the event log, catch-up, idempotence and conflicts.

mod common;

use common::{Res, Temp, sp};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::layout::Change;
use memcp_core::sync::{self, Cursor, Event};
use memcp_core::text::Language;
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space};

type Boxed = Box<dyn std::error::Error>;

/// A node with the space `notes` and nothing else.
fn node(t: &Temp, name: &str) -> Result<Brain, Boxed> {
    Ok(Brain::init(
        &t.path().join(name),
        &[sp("notes")],
        Language::English,
    )?)
}

fn write(b: &Brain, text: &str) -> Result<String, Boxed> {
    let conn = b.open_space(&sp("notes"))?;
    match facts::remember(
        &conn,
        &sp("notes"),
        "me",
        NewFact::agent(text, "agent:t"),
        &[],
    )? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn replace(b: &Brain, id: &str, text: &str) -> Result<String, Boxed> {
    let conn = b.open_space(&sp("notes"))?;
    Ok(facts::supersede(
        &conn,
        &sp("notes"),
        "me",
        id,
        NewFact::agent(text, "agent:t"),
        None,
    )?
    .id)
}

/// One catch-up: what `from` holds that `to` lacks, read after the cursor `to` keeps for `from`.
fn catch_up(
    from: &Brain,
    to: &Brain,
    from_name: &str,
    to_name: &str,
    space: &Space,
) -> Result<sync::Applied, Boxed> {
    let mut total = sync::Applied::default();
    loop {
        let (pulled, _) = sync::cursors(to, from_name, space)?;
        let page = sync::events_after(from, space, pulled, 3, 1 << 20, Some(to_name))?;
        let r = sync::apply(to, space, &page.events, from_name)?;
        if r.stopped.is_some() {
            return Err(format!("stopped: {:?}", r.stopped).into());
        }
        sync::set_cursor(to, from_name, space, Cursor::Pulled, page.next)?;
        total.applied += r.applied;
        total.skipped += r.skipped;
        total.disputed += r.disputed;
        total.rejected.extend(r.rejected);
        if !page.more {
            return Ok(total);
        }
    }
}

type Row = (String, String, Option<String>);

fn state(b: &Brain) -> Result<Vec<Row>, Boxed> {
    let conn = b.open_space(&sp("notes"))?;
    let mut st = conn.prepare("SELECT id, status, replaced_by FROM facts ORDER BY id")?;
    Ok(st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?)
}

#[test]
fn every_change_is_logged_with_a_stable_id_and_a_rising_clock() -> Res {
    let t = Temp::new("sync-log");
    let a = node(&t, "a")?;
    let one = write(&a, "The boiler is serviced in March")?;
    let two = replace(&a, &one, "The boiler is serviced in April")?;
    facts::retire(&a.open_space(&sp("notes"))?, "me", &two, "gone")?;
    let page = sync::events_after(&a, &sp("notes"), 0, 100, 1 << 20, None)?;
    let kinds: Vec<&str> = page.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "fact_created",
            "fact_created",
            "fact_superseded",
            "fact_retired"
        ]
    );
    assert_eq!(page.events[0].eid, format!("c:{one}"));
    assert_eq!(page.events[2].eid, format!("s:{one}:{two}"));
    let hlcs: Vec<&String> = page.events.iter().map(|e| &e.hlc).collect();
    assert!(hlcs.windows(2).all(|w| w[0] < w[1]), "the clock only rises");
    assert_eq!(page.head, page.events.last().map_or(0, |e| e.seq));
    let none = sync::events_after(&a, &sp("notes"), page.head, 100, 1 << 20, None)?;
    assert!(none.events.is_empty() && !none.more);
    Ok(())
}

#[test]
fn a_space_that_existed_before_the_log_is_logged_whole_once() -> Res {
    let t = Temp::new("sync-backfill");
    let a = node(&t, "a")?;
    let one = write(&a, "alpha beta gamma")?;
    {
        let conn = rusqlite::Connection::open(t.path().join("a").join("notes.sqlite"))?;
        conn.execute_batch(
            "DROP TRIGGER sync_log_no_delete; DELETE FROM sync_log; DELETE FROM meta WHERE k IN ('sync_backfilled','hlc_last');",
        )?;
    }
    let page = sync::events_after(&a, &sp("notes"), 0, 100, 1 << 20, None)?;
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].eid, format!("c:{one}"));
    let again = sync::events_after(&a, &sp("notes"), 0, 100, 1 << 20, None)?;
    assert_eq!(again.events.len(), 1, "logged once");
    Ok(())
}

#[test]
fn first_join_is_a_full_copy_and_a_second_catch_up_changes_nothing() -> Res {
    let t = Temp::new("sync-copy");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let one = write(&a, "The garden gate is green")?;
    let two = replace(&a, &one, "The garden gate is blue")?;
    let three = write(&a, "Water the plants on Sunday")?;
    facts::retire(&a.open_space(&sp("notes"))?, "me", &three, "done")?;
    let first = catch_up(&a, &b, "a", "b", &sp("notes"))?;
    assert_eq!(first.applied, 5);
    assert_eq!(state(&a)?, state(&b)?);
    let conn = b.open_space(&sp("notes"))?;
    let chain = facts::history(&conn, &two)?;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].text, "The garden gate is green");
    let page = sync::events_after(&a, &sp("notes"), 0, 100, 1 << 20, None)?;
    let again = sync::apply(&b, &sp("notes"), &page.events, "a")?;
    assert_eq!((again.applied, again.skipped), (0, 5));
    let n: i64 = conn.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    assert_eq!(n, 3);
    // A third node fed from the copy gets the same memory, and a peer is not sent back what it sent.
    let c = node(&t, "c")?;
    catch_up(&b, &c, "b", "c", &sp("notes"))?;
    assert_eq!(state(&a)?, state(&c)?);
    let back = sync::events_after(&b, &sp("notes"), 0, 100, 1 << 20, Some("a"))?;
    assert!(back.events.is_empty());
    Ok(())
}

#[test]
fn the_same_fact_replaced_on_two_nodes_is_kept_both_ways_and_in_dispute() -> Res {
    let t = Temp::new("sync-dispute");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let base = write(&a, "The meeting is on Monday")?;
    catch_up(&a, &b, "a", "b", &sp("notes"))?;
    let on_a = replace(&a, &base, "The meeting is on Tuesday")?;
    let on_b = replace(&b, &base, "The meeting is on Friday")?;
    catch_up(&a, &b, "a", "b", &sp("notes"))?;
    catch_up(&b, &a, "b", "a", &sp("notes"))?;
    assert_eq!(state(&a)?, state(&b)?, "same result on both");
    assert_eq!(state(&a)?.len(), 3, "both replacements and the original");
    let mut both = vec![on_a, on_b];
    both.sort();
    for n in [&a, &b] {
        let conn = n.open_space(&sp("notes"))?;
        let d = sync::disputes(&conn)?;
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0, base);
        assert_eq!(d[0].1, both);
        let shown: Option<String> = conn.query_row(
            "SELECT replaced_by FROM facts WHERE id = ?1",
            [&base],
            |r| r.get(0),
        )?;
        assert_eq!(shown.as_ref(), both.first());
    }
    Ok(())
}

#[test]
fn retired_on_one_node_and_replaced_on_another_ends_alike_in_any_order() -> Res {
    let t = Temp::new("sync-retire");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let base = write(&a, "Call the plumber")?;
    catch_up(&a, &b, "a", "b", &sp("notes"))?;
    let new = replace(&a, &base, "Call the electrician")?;
    facts::retire(&b.open_space(&sp("notes"))?, "me", &base, "not needed")?;
    catch_up(&a, &b, "a", "b", &sp("notes"))?;
    catch_up(&b, &a, "b", "a", &sp("notes"))?;
    assert_eq!(state(&a)?, state(&b)?);
    let conn = a.open_space(&sp("notes"))?;
    let d = sync::disputes(&conn)?;
    assert_eq!(d.len(), 1);
    assert!(d[0].1.contains(&"retired".to_string()) && d[0].1.contains(&new));
    Ok(())
}

#[test]
fn the_same_text_written_on_two_nodes_is_kept_twice() -> Res {
    let t = Temp::new("sync-twice");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    write(&a, "Buy bread")?;
    write(&b, "Buy bread")?;
    catch_up(&a, &b, "a", "b", &sp("notes"))?;
    catch_up(&b, &a, "b", "a", &sp("notes"))?;
    assert_eq!(state(&a)?.len(), 2);
    assert_eq!(state(&a)?, state(&b)?);
    Ok(())
}

#[test]
fn files_travel_with_their_bytes_and_a_name_clash_keeps_both() -> Res {
    let t = Temp::new("sync-files");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let s = sp("notes");
    let f1 = memcp_core::files::put(&a, &s, "me", "plan.txt", "first", b"first plan")?;
    let f2 = memcp_core::files::put(&b, &s, "me", "plan.txt", "second", b"second plan")?;
    catch_up(&a, &b, "a", "b", &s)?;
    catch_up(&b, &a, "b", "a", &s)?;
    let names = |n: &Brain| -> Result<Vec<String>, Boxed> {
        let mut v: Vec<String> = memcp_core::files::list(n, &s, None, 10)?
            .into_iter()
            .map(|f| f.name)
            .collect();
        v.sort();
        Ok(v)
    };
    for n in [&a, &b] {
        assert_eq!(names(n)?.len(), 2);
        let (_, bytes) = memcp_core::files::get(n, &s, &f1.id)?.ok_or("missing 1")?;
        assert_eq!(bytes, b"first plan");
        let (_, bytes) = memcp_core::files::get(n, &s, &f2.id)?.ok_or("missing 2")?;
        assert_eq!(bytes, b"second plan");
    }
    assert_eq!(names(&a)?, names(&b)?, "the same names on both nodes");
    assert!(names(&a)?.iter().any(|n| n == "plan.txt"));
    memcp_core::files::retire(&a, &s, "me", &f1.id, "old")?;
    catch_up(&a, &b, "a", "b", &s)?;
    assert_eq!(memcp_core::files::list(&b, &s, None, 10)?.len(), 1);
    Ok(())
}

#[test]
fn layout_is_last_writer_wins_per_field() -> Res {
    let t = Temp::new("sync-layout");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let s = sp("notes");
    let label = |l: &str| Change {
        label: Some(l.into()),
        ..Change::default()
    };
    a.set_layout(OWNER, &s, &label("Notes A"))?;
    catch_up(&a, &b, "a", "b", &s)?;
    std::thread::sleep(std::time::Duration::from_millis(5));
    b.set_layout(OWNER, &s, &label("Notes B"))?;
    a.set_layout(
        OWNER,
        &s,
        &Change {
            position: Some(7),
            ..Change::default()
        },
    )?;
    catch_up(&a, &b, "a", "b", &s)?;
    catch_up(&b, &a, "b", "a", &s)?;
    for n in [&a, &b] {
        let p = n
            .layout(OWNER)?
            .into_iter()
            .find(|p| p.space == s)
            .ok_or("no place")?;
        assert_eq!((p.label.as_str(), p.position), ("Notes B", 7));
    }
    Ok(())
}

#[test]
fn bad_events_are_refused_and_change_nothing() -> Res {
    let t = Temp::new("sync-hostile");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let id = write(&a, "Something true")?;
    let page = sync::events_after(&a, &sp("notes"), 0, 10, 1 << 20, None)?;
    let good = page.events[0].clone();
    let mut forged: Event = good.clone();
    forged.eid = "c:f_other".into();
    let mut future = good.clone();
    future.hlc = format!(
        "{:015}.00000.zzzz",
        memcp_core::util::now_ms() + 10 * 3_600_000
    );
    let mut odd = good.clone();
    odd.kind = "wipe".into();
    let r = sync::apply(&b, &sp("notes"), &[forged, future, odd], "a")?;
    assert_eq!((r.applied, r.rejected.len()), (0, 3));
    assert!(state(&b)?.is_empty());
    // An event that needs one not here yet stops the batch: the rest waits.
    replace(&a, &id, "Something else true")?;
    let all = sync::events_after(&a, &sp("notes"), 0, 10, 1 << 20, None)?;
    let late: Vec<Event> = all
        .events
        .iter()
        .filter(|e| e.kind == "fact_superseded")
        .cloned()
        .collect();
    let r = sync::apply(&b, &sp("notes"), &late, "a")?;
    assert!(r.stopped.is_some() && r.consumed == 0);
    Ok(())
}

#[test]
fn status_reports_direction_progress_and_goes_idle() -> Res {
    let t = Temp::new("sync-status");
    let a = node(&t, "a")?;
    let s = sp("notes");
    let conn = a.open_control()?;
    memcp_core::access::connect_for(
        &conn,
        OWNER,
        "peer-laptop-ab12",
        &memcp_core::access::Rights::parse("notes:rw")?,
        None,
        false,
        false,
    )?;
    let idle = sync::status(&a, 1000)?;
    assert_eq!(idle.len(), 1);
    assert_eq!(
        (idle[0].name.as_str(), idle[0].direction.as_str()),
        ("laptop", "idle")
    );
    sync::note_transfer(&a, "peer-laptop-ab12", &s, "sending", 30, 120, 1000)?;
    let now = sync::status(&a, 1002)?;
    assert_eq!(now[0].direction, "sending");
    assert_eq!(now[0].spaces[0].percent, 25);
    sync::note_transfer(&a, "peer-laptop-ab12", &s, "receiving", 120, 120, 1003)?;
    assert_eq!(sync::status(&a, 1004)?[0].spaces[0].percent, 100);
    assert_eq!(sync::status(&a, 1100)?[0].direction, "idle");
    Ok(())
}
