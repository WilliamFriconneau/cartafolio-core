//! Many threads on one space, each with its own connection, as the server's workers are: nothing is lost, nothing is
//! written twice, a reader never sees a half-written state, and a race on one fact has one winner.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use common::{Res, Temp, brain, sp};
use memcp_core::Brain;
use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};

const NOW: i64 = 1_900_000_000;

/// Made-up words, so that no two texts look alike to the near-duplicate guard.
fn words(seed: u64) -> String {
    const SYL: [&str; 16] = [
        "ka", "lo", "mi", "ru", "te", "vo", "an", "is", "ul", "be", "do", "fi", "go", "ha", "je",
        "ny",
    ];
    let mut x = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (0..8)
        .map(|_| {
            (0..3)
                .map(|_| {
                    x = x
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    SYL[(x >> 33) as usize % SYL.len()]
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn active(b: &Brain, space: &str) -> i64 {
    b.open_space(&sp(space))
        .unwrap_or_else(|e| panic!("{e}"))
        .query_row(
            "SELECT count(*) FROM facts WHERE status = 'active'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1)
}

#[test]
fn many_writers_on_one_space_lose_nothing_and_a_reader_sees_it_only_grow() -> Res {
    let t = Temp::new("conc-many");
    let b = Arc::new(brain(&t)?);
    let (writers, each) = (8usize, 30usize);
    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(writers + 1));
    let failures = Arc::new(AtomicUsize::new(0));
    let reader = {
        let (b, done, start) = (Arc::clone(&b), Arc::clone(&done), Arc::clone(&start));
        std::thread::spawn(move || -> Result<Vec<i64>, String> {
            start.wait();
            let mut seen = vec![];
            let conn = b
                .open_space(&sp("work"))
                .map_err(|e| format!("reader open: {e}"))?;
            while !done.load(Ordering::SeqCst) {
                let n: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM facts WHERE status = 'active'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| format!("count: {e}"))?;
                // Searching while others write answers, and finds only whole facts.
                let hits = facts::recall(&conn, "ka", &RecallOptions::default(), NOW)
                    .map_err(|e| format!("recall: {e}"))?;
                if hits.iter().any(|h| h.fact.text.is_empty()) {
                    return Err("an empty fact".into());
                }
                seen.push(n);
            }
            Ok(seen)
        })
    };
    let handles: Vec<_> = (0..writers)
        .map(|w| {
            let (b, start, failures) = (Arc::clone(&b), Arc::clone(&start), Arc::clone(&failures));
            std::thread::spawn(move || {
                let space = sp("work");
                start.wait();
                for i in 0..each {
                    let conn = match b.open_space(&space) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("OPEN FAILED: {e}");
                            failures.fetch_add(1, Ordering::SeqCst);
                            continue;
                        }
                    };
                    let text = format!("{} ka", words((w * 1000 + i) as u64));
                    match facts::remember(
                        &conn,
                        &space,
                        "me",
                        NewFact::agent(&text, "agent:t"),
                        &[],
                    ) {
                        Ok(Remembered::Created(_)) => {}
                        other => {
                            eprintln!("REMEMBER FAILED: {other:?}");
                            failures.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().map_err(|_| "a writer panicked")?;
    }
    done.store(true, Ordering::SeqCst);
    let seen = reader.join().map_err(|_| "the reader panicked")??;
    assert_eq!(
        failures.load(Ordering::SeqCst),
        0,
        "no write failed or was refused"
    );
    assert_eq!(active(&b, "work"), (writers * each) as i64);
    assert!(!seen.is_empty());
    assert!(
        seen.windows(2).all(|w| w[0] <= w[1]),
        "the count only grows: {seen:?}"
    );
    // The history and the search index hold every one of them.
    let conn = b.open_space(&sp("work"))?;
    let events: i64 = conn.query_row(
        "SELECT count(*) FROM fact_events WHERE action = 'create'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(events, (writers * each) as i64);
    let found = facts::recall(
        &conn,
        "ka",
        &RecallOptions {
            limit: 500,
            ..RecallOptions::default()
        },
        NOW,
    )?;
    // A search returns at most 200 hits; the index itself holds one row per fact.
    assert_eq!(found.len(), 200);
    let indexed: i64 = conn.query_row("SELECT count(*) FROM facts_fts", [], |r| r.get(0))?;
    assert_eq!(indexed, (writers * each) as i64);
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    assert_eq!(check, "ok");
    Ok(())
}

#[test]
fn the_same_text_written_at_once_is_kept_once() -> Res {
    let t = Temp::new("conc-same");
    let b = Arc::new(brain(&t)?);
    let n = 8;
    let start = Arc::new(Barrier::new(n));
    let created = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..n)
        .map(|_| {
            let (b, start, created) = (Arc::clone(&b), Arc::clone(&start), Arc::clone(&created));
            std::thread::spawn(move || {
                let space = sp("work");
                let conn = b.open_space(&space).unwrap_or_else(|e| panic!("{e}"));
                start.wait();
                if let Ok(Remembered::Created(_)) = facts::remember(
                    &conn,
                    &space,
                    "me",
                    NewFact::agent("The boiler is serviced every autumn.", "agent:t"),
                    &[],
                ) {
                    created.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().map_err(|_| "panicked")?;
    }
    assert_eq!(created.load(Ordering::SeqCst), 1);
    assert_eq!(active(&b, "work"), 1);
    Ok(())
}

#[test]
fn many_threads_superseding_one_fact_leave_one_winner_and_a_whole_history() -> Res {
    let t = Temp::new("conc-race");
    let b = Arc::new(brain(&t)?);
    let space = sp("work");
    let id = {
        let conn = b.open_space(&space)?;
        match facts::remember(
            &conn,
            &space,
            "me",
            NewFact::agent("The meeting is at nine.", "agent:t"),
            &[],
        )? {
            Remembered::Created(f) => f.id,
            _ => return Err("not created".into()),
        }
    };
    let n = 8;
    let start = Arc::new(Barrier::new(n));
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let (b, start, id) = (Arc::clone(&b), Arc::clone(&start), id.clone());
            std::thread::spawn(move || {
                let space = sp("work");
                let conn = b.open_space(&space).unwrap_or_else(|e| panic!("{e}"));
                start.wait();
                let text = format!("{} meeting moved", words(900 + i as u64));
                facts::supersede(
                    &conn,
                    &space,
                    "me",
                    &id,
                    NewFact::agent(&text, "agent:t"),
                    None,
                )
                .is_ok()
            })
        })
        .collect();
    let wins = handles
        .into_iter()
        .map(|h| h.join().unwrap_or(false))
        .filter(|ok| *ok)
        .count();
    assert_eq!(wins, 1, "exactly one supersede wins the race");
    let conn = b.open_space(&space)?;
    let (replaced, by): (String, Option<String>) = conn.query_row(
        "SELECT status, replaced_by FROM facts WHERE id = ?1",
        [&id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(replaced, "replaced");
    let successor = by.ok_or("no successor")?;
    assert_eq!(active(&b, "work"), 1, "one active fact is left: the winner");
    let chain: Vec<String> = conn
        .prepare("SELECT id FROM facts WHERE status = 'active'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(chain, [successor]);
    let total: i64 = conn.query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?;
    assert_eq!(total, 2, "no loser left a fact behind");
    Ok(())
}

#[test]
fn a_retire_and_a_supersede_racing_on_one_fact_have_one_winner() -> Res {
    let t = Temp::new("conc-retire");
    let b = Arc::new(brain(&t)?);
    let space = sp("work");
    let id = {
        let conn = b.open_space(&space)?;
        match facts::remember(
            &conn,
            &space,
            "me",
            NewFact::agent("The garage code is old.", "agent:t"),
            &[],
        )? {
            Remembered::Created(f) => f.id,
            _ => return Err("not created".into()),
        }
    };
    let start = Arc::new(Barrier::new(2));
    let (b1, s1, id1) = (Arc::clone(&b), Arc::clone(&start), id.clone());
    let retire = std::thread::spawn(move || {
        let space = sp("work");
        let conn = b1.open_space(&space).unwrap_or_else(|e| panic!("{e}"));
        s1.wait();
        facts::retire(&conn, "me", &id1, "obsolete").is_ok()
    });
    let (b2, s2, id2) = (Arc::clone(&b), Arc::clone(&start), id.clone());
    let supersede = std::thread::spawn(move || {
        let space = sp("work");
        let conn = b2.open_space(&space).unwrap_or_else(|e| panic!("{e}"));
        s2.wait();
        facts::supersede(
            &conn,
            &space,
            "me",
            &id2,
            NewFact::agent("The garage code changed in spring.", "agent:t"),
            None,
        )
        .is_ok()
    });
    let (r, s) = (
        retire.join().unwrap_or(false),
        supersede.join().unwrap_or(false),
    );
    assert!(
        r ^ s,
        "one of the two wins, not both and not neither (retire {r}, supersede {s})"
    );
    let conn = b.open_space(&space)?;
    let status: String = conn.query_row("SELECT status FROM facts WHERE id = ?1", [&id], |r| {
        r.get(0)
    })?;
    assert_eq!(status, if r { "retired" } else { "replaced" });
    Ok(())
}
