//! A replica that holds a window of days: catch-up leaves out what is older, pruning removes it locally and only
//! once acknowledged, pinned notes stay, and later events about a pruned note apply without error.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Res, Temp, sp};
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::sync::{self, Cursor, Event};
use memcp_core::text::Language;
use memcp_core::tools;
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space};
use serde_json::json;

type Boxed = Box<dyn std::error::Error>;

const DAY: i64 = 86_400;
const NOW: i64 = 1_900_000_000;

fn node(t: &Temp, name: &str) -> Result<Brain, Boxed> {
    Ok(Brain::init(
        &t.path().join(name),
        &[sp("notes")],
        Language::English,
    )?)
}

/// Writes a note with tags, `age` days old.
fn write(b: &Brain, text: &str, tags: &[&str], age: i64) -> Result<String, Boxed> {
    let conn = b.open_space(&sp("notes"))?;
    let mut new = NewFact::agent(text, "agent:t");
    new.tags = tags.iter().map(|t| (*t).to_string()).collect();
    let Remembered::Created(f) = facts::remember(&conn, &sp("notes"), "me", new, &[])? else {
        return Err("not created".into());
    };
    conn.execute(
        "UPDATE facts SET created_at = ?2, valid_from = ?2 WHERE id = ?1",
        rusqlite::params![f.id, NOW - age * DAY],
    )?;
    Ok(f.id)
}

fn ids(b: &Brain) -> Result<BTreeSet<String>, Boxed> {
    let conn = b.open_space(&sp("notes"))?;
    let mut st = conn.prepare("SELECT id FROM facts")?;
    Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
}

fn count(b: &Brain, sql: &str) -> Result<i64, Boxed> {
    Ok(b.open_space(&sp("notes"))?
        .query_row(sql, [], |r| r.get(0))?)
}

/// What `peer` has acknowledged of the log of `b`: everything it holds now.
fn acknowledge(b: &Brain, peer: &str) -> Result<(), Boxed> {
    let head = sync::head(b, &sp("notes"))?;
    sync::set_cursor(b, peer, &sp("notes"), Cursor::Pushed, head)?;
    Ok(())
}

/// One catch-up of `to` from `from`, window-aware: `ages` rewrites the creation date of a note (in days before
/// `NOW`), because the log carries the date it was written with.
fn catch_up(
    from: &Brain,
    to: &Brain,
    from_name: &str,
    to_name: &str,
    ages: &BTreeMap<String, i64>,
) -> Result<sync::Applied, Boxed> {
    let space: Space = sp("notes");
    let cutoff = sync::window_cutoff(to, NOW)?;
    let mut total = sync::Applied::default();
    loop {
        let (pulled, _) = sync::cursors(to, from_name, &space)?;
        let mut page = sync::events_after(from, &space, pulled, 50, 1 << 20, Some(to_name))?;
        for e in &mut page.events {
            if let Some(id) = e.payload["id"].as_str()
                && let Some(age) = ages.get(id)
                && e.kind == "fact_created"
            {
                e.payload["created_at"] = json!(NOW - age * DAY);
            }
        }
        let r = sync::apply_window(to, &space, &page.events, from_name, cutoff)?;
        assert!(r.stopped.is_none(), "stopped: {:?}", r.stopped);
        assert!(r.rejected.is_empty(), "rejected: {:?}", r.rejected);
        sync::set_cursor(to, from_name, &space, Cursor::Pulled, page.next)?;
        total.applied += r.applied;
        total.skipped += r.skipped;
        if !page.more {
            return Ok(total);
        }
    }
}

fn eids(b: &Brain) -> Result<BTreeSet<String>, Boxed> {
    Ok(
        sync::events_after(b, &sp("notes"), 0, 10_000, 1 << 30, None)?
            .events
            .into_iter()
            .map(|e: Event| e.eid)
            .collect(),
    )
}

#[test]
fn a_note_nobody_acknowledged_is_never_pruned() -> Res {
    let t = Temp::new("win-ack");
    let phone = node(&t, "phone")?;
    let old = write(&phone, "The boiler was serviced in March", &[], 90)?;
    let space = sp("notes");
    // No peer at all.
    assert_eq!(sync::prune_window(&phone, &space, 30, NOW)?.facts, 0);
    // A peer that has not reached it.
    sync::set_cursor(&phone, "server", &space, Cursor::Pushed, 0)?;
    let r = sync::prune_window(&phone, &space, 30, NOW)?;
    assert_eq!((r.facts, r.kept_unacknowledged), (0, 1));
    assert!(ids(&phone)?.contains(&old));
    // Two peers: one is not enough.
    acknowledge(&phone, "server")?;
    sync::set_cursor(&phone, "drive", &space, Cursor::Pushed, 0)?;
    assert_eq!(sync::prune_window(&phone, &space, 30, NOW)?.facts, 0);
    acknowledge(&phone, "drive")?;
    let r = sync::prune_window(&phone, &space, 30, NOW)?;
    assert_eq!(r.facts, 1);
    assert!(ids(&phone)?.is_empty());
    // The note, its tags, its history, its search entry and its log rows are gone.
    assert_eq!(count(&phone, "SELECT count(*) FROM fact_events")?, 0);
    assert_eq!(count(&phone, "SELECT count(*) FROM facts_fts")?, 0);
    assert_eq!(count(&phone, "SELECT count(*) FROM sync_log")?, 0);
    Ok(())
}

#[test]
fn a_replaced_note_is_acknowledged_with_its_replacement_event() -> Res {
    let t = Temp::new("win-ack-supersede");
    let phone = node(&t, "phone")?;
    let space = sp("notes");
    let old = write(&phone, "The gate is green", &[], 90)?;
    acknowledge(&phone, "server")?;
    // Replaced afterwards: the replacement event has not been sent, so the old note stays.
    let conn = phone.open_space(&space)?;
    facts::retire(&conn, "me", &old, "gone")?;
    let r = sync::prune_window(&phone, &space, 30, NOW)?;
    assert_eq!((r.facts, r.kept_unacknowledged), (0, 1));
    acknowledge(&phone, "server")?;
    assert_eq!(sync::prune_window(&phone, &space, 30, NOW)?.facts, 1);
    Ok(())
}

#[test]
fn pinned_notes_and_the_last_days_stay() -> Res {
    let t = Temp::new("win-pin");
    let phone = node(&t, "phone")?;
    let space = sp("notes");
    let plain = write(
        &phone,
        "Dentist appointment moved to the autumn",
        &["home"],
        90,
    )?;
    let pinned = write(&phone, "Wifi password of the holiday cottage", &["pin"], 90)?;
    let under = write(
        &phone,
        "Passport renewal needs two photographs",
        &["pin/trip"],
        90,
    )?;
    let recent = write(
        &phone,
        "Buy tickets for the concert on Friday",
        &["home"],
        5,
    )?;
    acknowledge(&phone, "server")?;
    let r = sync::prune_window(&phone, &space, 30, NOW)?;
    assert_eq!((r.facts, r.tags, r.kept_pinned), (1, 1, 2));
    let left = ids(&phone)?;
    assert!(!left.contains(&plain));
    assert!([&pinned, &under, &recent].iter().all(|i| left.contains(*i)));
    // A retired pinned note stays too.
    let conn = phone.open_space(&space)?;
    facts::retire(&conn, "me", &pinned, "done")?;
    acknowledge(&phone, "server")?;
    assert_eq!(sync::prune_window(&phone, &space, 30, NOW)?.facts, 0);
    Ok(())
}

#[test]
fn pruning_emits_no_event_and_a_later_supersede_of_a_pruned_note_applies() -> Res {
    let t = Temp::new("win-later");
    let (server, phone) = (node(&t, "server")?, node(&t, "phone")?);
    let space = sp("notes");
    let old = write(&server, "The meeting is on Monday", &[], 90)?;
    let other = write(&server, "The code of the shed is 1234", &[], 90)?;
    let none = BTreeMap::new();
    sync::set_window_days(&phone, None)?;
    catch_up(&server, &phone, "server", "phone", &none)?;
    assert_eq!(ids(&phone)?.len(), 2);
    // The server learned nothing from the phone yet: acknowledge, then prune.
    acknowledge(&phone, "server")?;
    let before = eids(&phone)?;
    let seq_before = sync::head(&phone, &space)?;
    let hlc_before: String = phone.open_space(&space)?.query_row(
        "SELECT v FROM meta WHERE k = 'hlc_last'",
        [],
        |r| r.get(0),
    )?;
    // Rows the server sent are acknowledged by the server itself, so the phone's pushed cursor is not needed.
    let r = sync::prune_window(&phone, &space, 30, NOW)?;
    assert_eq!(r.facts, 2);
    // Local only: nothing new in the log, the clock did not move, the head did not pass where it was.
    assert!(eids(&phone)?.is_subset(&before));
    assert!(sync::head(&phone, &space)? <= seq_before);
    let hlc_after: String = phone.open_space(&space)?.query_row(
        "SELECT v FROM meta WHERE k = 'hlc_last'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(hlc_before, hlc_after);
    // The server replaces one note and retires the other, and adds a recent note replacing the first.
    let conn = server.open_space(&space)?;
    let new = facts::supersede(
        &conn,
        &space,
        "me",
        &old,
        NewFact::agent("The meeting is on Tuesday", "agent:t"),
        None,
    )?;
    facts::retire(&conn, "me", &other, "changed")?;
    let applied = catch_up(&server, &phone, "server", "phone", &none)?;
    assert!(applied.applied >= 1);
    // The replacement is here; the pruned notes did not come back, and nothing waits.
    let left = ids(&phone)?;
    assert!(left.contains(&new.id) && !left.contains(&old) && !left.contains(&other));
    let (pulled, _) = sync::cursors(&phone, "server", &space)?;
    assert_eq!(pulled, sync::head(&server, &space)?);
    // The same events again change nothing.
    assert_eq!(
        catch_up(&server, &phone, "server", "phone", &none)?.applied,
        0
    );
    Ok(())
}

#[test]
fn the_first_catch_up_of_a_window_replica_takes_the_window_and_the_pinned_notes() -> Res {
    let t = Temp::new("win-first");
    let (server, phone) = (node(&t, "server")?, node(&t, "phone")?);
    let space = sp("notes");
    let recent = write(&server, "Tomatoes ripen late in the north garden", &[], 3)?;
    let old = write(
        &server,
        "Quarterly invoice sent to the accountant",
        &[],
        200,
    )?;
    let pinned = write(
        &server,
        "Insurance policy number is stored in the drawer",
        &["pin"],
        200,
    )?;
    let replaced = write(
        &server,
        "Car tyres were changed before winter",
        &["pin"],
        200,
    )?;
    let gone = write(&server, "Piano tuning happens every spring", &[], 200)?;
    {
        let conn = server.open_space(&space)?;
        facts::supersede(
            &conn,
            &space,
            "me",
            &gone,
            NewFact::agent("Piano tuner phone changed recently", "agent:t"),
            None,
        )?;
        facts::retire(&conn, "me", &old, "closed")?;
    }
    let ages: BTreeMap<String, i64> = [
        (recent.clone(), 3),
        (old.clone(), 200),
        (pinned.clone(), 200),
        (replaced.clone(), 200),
        (gone.clone(), 200),
    ]
    .into();
    sync::set_window_days(&phone, Some(30))?;
    // The replacement of `gone` was written now, so its date is recent: it is held, `gone` is not.
    let applied = catch_up(&server, &phone, "server", "phone", &ages)?;
    assert!(applied.applied >= 3);
    let left = ids(&phone)?;
    assert!(left.contains(&recent) && left.contains(&pinned) && left.contains(&replaced));
    assert!(!left.contains(&old) && !left.contains(&gone));
    // Nothing of what was left out is in this replica's log, text included.
    assert_eq!(
        count(
            &phone,
            "SELECT count(*) FROM sync_log WHERE payload LIKE '%Quarterly invoice%'"
        )?,
        0
    );
    // A pinned note replaced by a note outside the window is closed as retired, not left open.
    let conn = server.open_space(&space)?;
    let mut next = NewFact::agent("Winter tyres stored at the garage", "agent:t");
    next.tags = vec!["archive".into()];
    let again = facts::supersede(&conn, &space, "me", &replaced, next, None)?;
    let mut ages2 = ages.clone();
    ages2.insert(again.id.clone(), 200);
    catch_up(&server, &phone, "server", "phone", &ages2)?;
    let status: String = phone.open_space(&space)?.query_row(
        "SELECT status FROM facts WHERE id = ?1",
        [&replaced],
        |r| r.get(0),
    )?;
    assert_eq!(status, "retired");
    assert!(!ids(&phone)?.contains(&again.id));
    Ok(())
}

#[test]
fn two_replicas_converge_on_what_they_both_hold() -> Res {
    let t = Temp::new("win-converge");
    let (server, phone) = (node(&t, "server")?, node(&t, "phone")?);
    let space = sp("notes");
    let a = write(&server, "Kitchen light bulbs need replacing soon", &[], 2)?;
    let b = write(&server, "Neighbour keeps the spare key", &["pin"], 100)?;
    write(&server, "Bicycle repair shop closes on Sundays", &[], 100)?;
    let ages: BTreeMap<String, i64> = [(a.clone(), 2), (b.clone(), 100)].into();
    sync::set_window_days(&phone, Some(30))?;
    catch_up(&server, &phone, "server", "phone", &ages)?;
    // The phone writes a note, and the server replaces one the phone holds.
    let c = write(&phone, "Bread from the bakery tastes better warm", &[], 0)?;
    {
        let conn = server.open_space(&space)?;
        facts::retire(&conn, "me", &a, "done")?;
    }
    // Both ways, twice.
    for _ in 0..2 {
        catch_up(&phone, &server, "phone", "server", &BTreeMap::new())?;
        catch_up(&server, &phone, "server", "phone", &ages)?;
    }
    acknowledge(&phone, "server")?;
    sync::prune_all(&phone, NOW)?;
    let held = ids(&phone)?;
    assert!(held.contains(&c) && held.contains(&a) && held.contains(&b));
    let state = |x: &Brain| -> Result<Vec<(String, String)>, Boxed> {
        let conn = x.open_space(&space)?;
        let mut st = conn.prepare("SELECT id, status FROM facts ORDER BY id")?;
        let rows: Vec<(String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows
            .into_iter()
            .filter(|(id, _)| held.contains(id))
            .collect())
    };
    assert_eq!(state(&phone)?, state(&server)?);
    Ok(())
}

#[test]
fn the_journal_and_a_board_still_work_on_the_window() -> Res {
    let t = Temp::new("win-journal");
    let phone = node(&t, "phone")?;
    let space = sp("notes");
    let ctl = phone.open_control()?;
    let rights = Rights::parse("notes:rw")?;
    let tok = access::connect_for(&ctl, OWNER, "assistant", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, NOW)?.ok_or("refused")?;
    let today = write(
        &phone,
        "Journal, today: walked to the market",
        &["journal"],
        1,
    )?;
    write(
        &phone,
        "Journal, long ago: a day at the sea",
        &["journal"],
        200,
    )?;
    let made = tools::call(
        &phone,
        &c,
        "living_set",
        &json!({ "space": "notes", "page": "next",
                 "add": [ { "text": "Call the plumber", "column": "now" } ] }),
    )?;
    let card = made["added"][0].as_str().ok_or("card")?.to_string();
    // The card is old, and so is the board; the sync is acknowledged.
    phone.open_space(&space)?.execute(
        "UPDATE facts SET created_at = ?1, valid_from = ?1 WHERE id = ?2",
        rusqlite::params![NOW - 200 * DAY, card],
    )?;
    acknowledge(&phone, "server")?;
    sync::set_window_days(&phone, Some(30))?;
    let done = sync::prune_all(&phone, NOW)?.ok_or("no window")?;
    assert_eq!(done.iter().map(|p| p.facts).sum::<usize>(), 1);
    // The journal of the last days reads, the old entry is gone, and the board still has its card.
    let read = tools::call(
        &phone,
        &c,
        "recall",
        &json!({ "space": "notes", "query": "journal", "tags": ["journal"] }),
    )?;
    let text = read.to_string();
    assert!(text.contains("walked to the market"), "{text}");
    assert!(!text.contains("a day at the sea"), "{text}");
    assert!(ids(&phone)?.contains(&today));
    let board = tools::call(
        &phone,
        &c,
        "living_get",
        &json!({ "space": "notes", "page": "next" }),
    )?;
    assert_eq!(
        board["board"]["now"].as_array().map(Vec::len),
        Some(1),
        "{board}"
    );
    // The page says what the window is and what was pruned.
    let status = sync::window_status(&phone)?;
    assert_eq!(status.label(), "window: 30 days");
    assert_eq!(status.last_prune.map(|l| l.facts), Some(1));
    Ok(())
}

#[test]
fn the_window_is_off_by_default_and_checked() -> Res {
    let t = Temp::new("win-setting");
    let phone = node(&t, "phone")?;
    assert_eq!(sync::window_days(&phone)?, None);
    assert!(sync::prune_all(&phone, NOW)?.is_none());
    assert_eq!(sync::window_status(&phone)?.label(), "window: all");
    assert!(sync::set_window_days(&phone, Some(0)).is_err());
    sync::set_window_days(&phone, Some(14))?;
    assert_eq!(sync::window_days(&phone)?, Some(14));
    sync::set_window_days(&phone, None)?;
    assert_eq!(sync::window_days(&phone)?, None);
    Ok(())
}
