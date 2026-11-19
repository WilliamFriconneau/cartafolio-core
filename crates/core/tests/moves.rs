//! Moving notes from a space to its sub-space by tag: previewed, then done, and never an erasure.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::layout::Change;
use memcp_core::users::OWNER;

fn put(b: &memcp_core::Brain, space: &str, text: &str, tag: &str) -> Res {
    let conn = b.open_space(&sp(space))?;
    let mut n = NewFact::agent(text, "agent:test");
    n.tags = vec![tag.into()];
    match facts::remember(&conn, &sp(space), "tester", n, &[])? {
        Remembered::Created(_) => Ok(()),
        other => Err(format!("not created: {other:?}").into()),
    }
}

#[test]
fn notes_of_a_tag_go_to_the_sub_space_and_the_source_says_where() -> Res {
    let t = Temp::new("moves");
    let b = brain(&t)?;
    // `home` is the parent; `personal` plays the sub-space of cars here.
    b.set_layout(
        OWNER,
        &sp("personal"),
        &Change {
            parent: Some(Some(sp("home"))),
            ..Default::default()
        },
    )?;
    put(
        &b,
        "home",
        "The ID4 charges at 11 kW on the wallbox.",
        "home/voitures/id4",
    )?;
    put(
        &b,
        "home",
        "Winter tyres go on in November.",
        "home/voitures",
    )?;
    put(
        &b,
        "home",
        "The boiler is serviced in March.",
        "home/chauffage",
    )?;
    // A preview changes nothing.
    let seen = b.move_by_tag(
        &sp("home"),
        &sp("personal"),
        "home/voitures",
        "tester",
        true,
    )?;
    assert_eq!((seen.matched, seen.moved), (2, 0));
    assert_eq!(seen.sample.len(), 2);
    let dst = b.open_space(&sp("personal"))?;
    assert_eq!(facts::counts(&dst)?["active"], serde_json::Value::Null);
    // The move writes them again in the target and retires them in the source, with the reason.
    let done = b.move_by_tag(
        &sp("home"),
        &sp("personal"),
        "home/voitures",
        "tester",
        false,
    )?;
    assert_eq!((done.matched, done.moved, done.failed), (2, 2, 0));
    assert_eq!(facts::counts(&dst)?["active"], 2);
    let src = b.open_space(&sp("home"))?;
    assert_eq!(facts::counts(&src)?["active"], 1, "the boiler stays");
    let reason: String = src.query_row(
        "SELECT status_reason FROM facts WHERE status = 'retired' LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(reason, "moved to personal");
    // The tags travel with the notes, and doing it again moves nothing.
    let tags: Vec<String> = dst
        .prepare("SELECT DISTINCT tag FROM fact_tags ORDER BY tag")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert!(tags.contains(&"home/voitures/id4".to_string()), "{tags:?}");
    let again = b.move_by_tag(
        &sp("home"),
        &sp("personal"),
        "home/voitures",
        "tester",
        false,
    )?;
    assert_eq!((again.matched, again.moved), (0, 0));
    Ok(())
}

#[test]
fn a_space_s_own_settings_never_move_with_its_notes() -> Res {
    let t = Temp::new("moves");
    let b = brain(&t)?;
    put(&b, "home", "{\"version\":1,\"sections\":[]}", "dashboard")?;
    let done = b.move_by_tag(&sp("home"), &sp("personal"), "dashboard", "tester", false)?;
    assert_eq!(done.matched, 0);
    Ok(())
}

// ---- Big moves: bounded calls, resumable, crash-safe ----------------------------------------------------------

use std::time::{Duration, Instant};

use memcp_core::Brain;
use memcp_core::moves::{Limits, Step, fail_at};

type Out<T> = Result<T, Box<dyn std::error::Error>>;

/// Five made-up words for note `i`: no two notes look alike, so what is measured is the move, not a closeness check.
fn words(i: usize) -> String {
    let syl = [
        "ka", "lo", "mi", "ne", "ru", "sa", "to", "vi", "be", "do", "fu", "gi", "ha", "ji", "pe",
        "zo",
    ];
    let mut x = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut out = vec![];
    for _ in 0..5 {
        let mut w = String::new();
        for _ in 0..4 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            w.push_str(syl[(x % 16) as usize]);
        }
        out.push(w);
    }
    format!("{} n{i}", out.join(" "))
}

/// `n` notes under `tag` in `space`. Returns their texts.
fn fill(b: &Brain, space: &str, tag: &str, n: usize) -> Out<Vec<String>> {
    let conn = b.open_space(&sp(space))?;
    let mut texts = vec![];
    for i in 0..n {
        let text = words(i);
        let mut new = NewFact::agent(&text, "agent:test");
        new.tags = vec![tag.into()];
        match facts::remember(&conn, &sp(space), "tester", new, &[])? {
            Remembered::Created(_) => texts.push(text),
            other => return Err(format!("fixture {i} not created: {other:?}").into()),
        }
    }
    Ok(texts)
}

/// How many active notes hold `text` in `space`.
fn active_with(b: &Brain, space: &str, text: &str) -> Out<usize> {
    let c = b.open_space(&sp(space))?;
    let k: i64 = c.query_row(
        "SELECT COUNT(*) FROM facts WHERE status = 'active' AND text = ?1",
        [text],
        |r| r.get(0),
    )?;
    Ok(usize::try_from(k)?)
}

/// The invariant of a move, whatever happened: every note is active exactly once, in the source or in the target.
/// Returns how many are in each.
fn exactly_once(b: &Brain, from: &str, to: &str, texts: &[String]) -> Out<(usize, usize)> {
    let (mut here, mut there) = (0, 0);
    for t in texts {
        let (a, z) = (active_with(b, from, t)?, active_with(b, to, t)?);
        assert_eq!(a + z, 1, "{t:?}: {a} in {from}, {z} in {to}");
        here += a;
        there += z;
    }
    Ok((here, there))
}

/// The peers' log tells each change once: one creation per fact of the target, one retirement per retired source.
fn logged_once(b: &Brain, from: &str, to: &str) -> Res {
    let count = |c: &rusqlite::Connection, sql: &str| -> Out<i64> {
        Ok(c.query_row(sql, [], |r| r.get(0))?)
    };
    let src = b.open_space(&sp(from))?;
    let dst = b.open_space(&sp(to))?;
    assert_eq!(
        count(&src, "SELECT COUNT(*) FROM facts WHERE status = 'retired'")?,
        count(
            &src,
            "SELECT COUNT(*) FROM sync_log WHERE kind = 'fact_retired'"
        )?,
        "every retirement is logged once"
    );
    assert_eq!(
        count(&dst, "SELECT COUNT(*) FROM facts")?,
        count(
            &dst,
            "SELECT COUNT(*) FROM sync_log WHERE kind = 'fact_created'"
        )?,
        "every copy is logged once"
    );
    Ok(())
}

fn limits(limit: usize) -> Limits {
    Limits {
        limit,
        budget: Duration::from_secs(120),
    }
}

fn move_all(b: &Brain, from: &str, to: &str, tag: &str) -> Out<memcp_core::Moved> {
    Ok(b.move_by_tag_within(&sp(from), &sp(to), tag, "tester", false, &limits(5000))?)
}

#[test]
fn a_big_tag_moves_in_bounded_calls_until_nothing_remains() -> Res {
    let t = Temp::new("moves-bounded");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", 25)?;
    let pre = b.move_by_tag_within(
        &sp("home"),
        &sp("personal"),
        "bulk",
        "tester",
        true,
        &limits(10),
    )?;
    assert_eq!(
        (pre.matched, pre.moved, pre.remaining),
        (25, 0, 25),
        "{pre:?}"
    );
    let (mut calls, mut moved) = (0, 0);
    loop {
        let r = b.move_by_tag_within(
            &sp("home"),
            &sp("personal"),
            "bulk",
            "tester",
            false,
            &limits(10),
        )?;
        calls += 1;
        moved += r.moved;
        assert!(r.moved <= 10, "{r:?}");
        assert_eq!(r.remaining, 25 - moved, "{r:?}");
        if r.remaining == 0 {
            break;
        }
        assert!(calls < 10, "it never ends: {r:?}");
    }
    assert_eq!((calls, moved), (3, 25));
    assert_eq!(exactly_once(&b, "home", "personal", &texts)?, (0, 25));
    logged_once(&b, "home", "personal")?;
    // A spent time budget stops the call early, and the next one carries on.
    let more = fill(&b, "home", "late", 3)?;
    let none = b.move_by_tag_within(
        &sp("home"),
        &sp("personal"),
        "late",
        "tester",
        false,
        &Limits {
            limit: 100,
            budget: Duration::ZERO,
        },
    )?;
    assert_eq!((none.moved, none.remaining), (0, 3), "{none:?}");
    assert_eq!(move_all(&b, "home", "personal", "late")?.moved, 3);
    exactly_once(&b, "home", "personal", &more)?;
    Ok(())
}

#[test]
fn a_crash_at_any_step_ends_in_moved_once_never_in_both_or_neither() -> Res {
    for step in [Step::AfterJournal, Step::AfterCopies] {
        let t = Temp::new("moves-crash");
        let b = brain(&t)?;
        let texts = fill(&b, "home", "bulk", 30)?;
        fail_at(Some(step));
        let cut = b.move_by_tag(&sp("home"), &sp("personal"), "bulk", "tester", false);
        fail_at(None);
        assert!(cut.is_err(), "{step:?}: the failpoint stops the call");
        // The node starts again: the moves it had begun are finished before anything else.
        let b = Brain::open(b.dir())?;
        let finished = b.finish_moves()?;
        assert_eq!(finished, 30, "{step:?}");
        assert_eq!(
            exactly_once(&b, "home", "personal", &texts)?,
            (0, 30),
            "{step:?}"
        );
        logged_once(&b, "home", "personal")?;
        assert_eq!(b.finish_moves()?, 0, "{step:?}: nothing is left pending");
        let again = move_all(&b, "home", "personal", "bulk")?;
        assert_eq!((again.matched, again.moved), (0, 0), "{step:?}");
    }
    Ok(())
}

#[test]
fn the_next_call_finishes_a_move_cut_off_before_it_starts_its_own() -> Res {
    let t = Temp::new("moves-next-call");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", 12)?;
    fail_at(Some(Step::AfterCopies));
    assert!(
        b.move_by_tag(&sp("home"), &sp("personal"), "bulk", "tester", false)
            .is_err()
    );
    fail_at(None);
    let r = move_all(&b, "home", "personal", "bulk")?;
    assert_eq!(
        (r.resumed, r.moved, r.failed, r.remaining),
        (12, 0, 0, 0),
        "{r:?}"
    );
    assert_eq!(exactly_once(&b, "home", "personal", &texts)?, (0, 12));
    logged_once(&b, "home", "personal")?;
    Ok(())
}

#[test]
fn a_copy_left_by_an_older_version_is_not_written_twice() -> Res {
    let t = Temp::new("moves-older-crash");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", 6)?;
    // A crash between the copy and the retirement, from a version that kept no journal: the copies are there, the
    // sources still active, and nothing says a move was under way.
    fail_at(Some(Step::AfterCopies));
    assert!(
        b.move_by_tag(&sp("home"), &sp("personal"), "bulk", "tester", false)
            .is_err()
    );
    fail_at(None);
    b.open_space(&sp("home"))?
        .execute("DELETE FROM move_pending", [])?;
    assert_eq!(active_with(&b, "personal", &texts[0])?, 1);
    let r = move_all(&b, "home", "personal", "bulk")?;
    assert_eq!(
        (r.matched, r.moved, r.already, r.failed),
        (6, 0, 6, 0),
        "{r:?}"
    );
    assert_eq!(exactly_once(&b, "home", "personal", &texts)?, (0, 6));
    logged_once(&b, "home", "personal")?;
    Ok(())
}

#[test]
fn two_calls_at_once_on_the_same_tag_both_end_and_move_each_note_once() -> Res {
    let t = Temp::new("moves-concurrent");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", 600)?;
    let dir = b.dir().to_path_buf();
    let run = |dir: std::path::PathBuf| {
        std::thread::spawn(move || -> Result<(usize, usize), String> {
            let b = Brain::open(&dir).map_err(|e| e.to_string())?;
            let (mut moved, mut already) = (0, 0);
            loop {
                let r = b
                    .move_by_tag_within(
                        &sp("home"),
                        &sp("personal"),
                        "bulk",
                        "tester",
                        false,
                        &limits(100),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(r.failed, 0, "{r:?}");
                // A note finished by the other call's resumption is counted there, not here.
                moved += r.moved;
                already += r.already + r.resumed;
                if r.remaining == 0 {
                    return Ok((moved, already));
                }
            }
        })
    };
    let one = run(dir.clone());
    let two = run(dir);
    let one = one.join().map_err(|_| "thread one panicked")??;
    let two = two.join().map_err(|_| "thread two panicked")??;
    assert!(one.0 + two.0 <= 600, "{one:?} {two:?}");
    assert_eq!(exactly_once(&b, "home", "personal", &texts)?, (0, 600));
    logged_once(&b, "home", "personal")?;
    Ok(())
}

#[test]
fn a_note_close_to_one_of_the_target_moves_anyway() -> Res {
    let t = Temp::new("moves-close");
    let b = brain(&t)?;
    put(
        &b,
        "personal",
        "The ID4 charges at 11 kW on the wallbox.",
        "cars",
    )?;
    put(
        &b,
        "home",
        "The ID4 charges at 11 kW on the wallbox in the garage.",
        "cars",
    )?;
    let r = move_all(&b, "home", "personal", "cars")?;
    assert_eq!((r.moved, r.failed), (1, 0), "{r:?}");
    assert_eq!(
        active_with(
            &b,
            "personal",
            "The ID4 charges at 11 kW on the wallbox in the garage."
        )?,
        1
    );
    Ok(())
}

#[test]
fn a_note_the_target_refuses_is_listed_with_its_reason_and_stays() -> Res {
    let t = Temp::new("moves-refused");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", 3)?;
    // A note stored by an older version with a control character: the write rules of today refuse it.
    let src = b.open_space(&sp("home"))?;
    let bad = format!("{}\u{7}", texts[1]);
    src.execute(
        "UPDATE facts SET text = ?1 WHERE text = ?2",
        [&bad, &texts[1]],
    )?;
    let r = move_all(&b, "home", "personal", "bulk")?;
    assert_eq!((r.moved, r.failed, r.remaining), (2, 1, 0), "{r:?}");
    assert_eq!(r.failures.len(), 1);
    assert!(
        r.failures[0].1.contains("control characters"),
        "{:?}",
        r.failures
    );
    assert_eq!(
        active_with(&b, "home", &bad)?,
        1,
        "the refused note stays where it was"
    );
    assert_eq!(
        b.finish_moves()?,
        0,
        "a refused note leaves nothing pending"
    );
    // Calling again tries it again, fails the same way, and still ends.
    let again = move_all(&b, "home", "personal", "bulk")?;
    assert_eq!(
        (again.moved, again.failed, again.remaining),
        (0, 1, 0),
        "{again:?}"
    );
    Ok(())
}

#[test]
fn a_few_thousand_notes_move_fast_enough() -> Res {
    let n = 3000;
    let t = Temp::new("moves-speed");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", n)?;
    let start = Instant::now();
    let mut calls = 0;
    loop {
        let r = b.move_by_tag(&sp("home"), &sp("personal"), "bulk", "tester", false)?;
        calls += 1;
        assert_eq!(r.failed, 0, "{r:?}");
        if r.remaining == 0 {
            break;
        }
    }
    let took = start.elapsed();
    println!(
        "moved {n} notes in {calls} calls, {:.2} s, {:.0} notes/s",
        took.as_secs_f64(),
        n as f64 / took.as_secs_f64()
    );
    assert_eq!(exactly_once(&b, "home", "personal", &texts)?, (0, n));
    logged_once(&b, "home", "personal")?;
    Ok(())
}

/// The child of `kill_9_in_the_middle_of_a_big_move`: moves the tag of the data directory it is given until the end
/// (it is killed before). Does nothing in a plain test run.
#[test]
fn child_moving_a_big_tag() -> Res {
    let Ok(dir) = std::env::var("MEMCP_MOVE_CHILD_DIR") else {
        return Ok(());
    };
    let b = Brain::open(std::path::Path::new(&dir))?;
    loop {
        let r = b.move_by_tag_within(
            &sp("home"),
            &sp("personal"),
            "bulk",
            "tester",
            false,
            &limits(400),
        )?;
        if r.remaining == 0 {
            return Ok(());
        }
    }
}

#[cfg(unix)]
#[test]
fn kill_9_in_the_middle_of_a_big_move() -> Res {
    let t = Temp::new("moves-kill");
    let b = brain(&t)?;
    let texts = fill(&b, "home", "bulk", 2000)?;
    let exe = std::env::current_exe()?;
    let mut rounds = 0;
    for wait_ms in [40u64, 90, 150, 220, 300, 400, 600, 900] {
        rounds += 1;
        let mut child = std::process::Command::new(&exe)
            .args([
                "child_moving_a_big_tag",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MEMCP_MOVE_CHILD_DIR", b.dir())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        std::thread::sleep(Duration::from_millis(wait_ms));
        child.kill()?; // SIGKILL
        child.wait()?;
        // The node starts again: what was begun is finished, and the invariant holds after every kill.
        let b = Brain::open(b.dir())?;
        let finished = b.finish_moves()?;
        let (here, there) = exactly_once(&b, "home", "personal", &texts)?;
        logged_once(&b, "home", "personal")?;
        println!(
            "kill after {wait_ms} ms: {finished} finished at restart, {here} in the source, {there} in the target"
        );
        if here == 0 {
            break;
        }
    }
    let r = move_all(&b, "home", "personal", "bulk")?;
    assert_eq!(r.failed, 0, "{r:?}");
    assert_eq!(exactly_once(&b, "home", "personal", &texts)?, (0, 2000));
    logged_once(&b, "home", "personal")?;
    println!("{rounds} kills");
    Ok(())
}
