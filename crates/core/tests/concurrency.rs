//! Many threads on one space, each with its own connection, as the server's workers are: nothing is lost, nothing is
//! written twice, a reader never sees a half-written state, and a race on one fact has one winner.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

use common::{Res, Temp, brain, sp};
use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};
use memcp_core::{Brain, Error};

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
    let b = brain(&t)?;
    let (writers, each) = (8usize, 30usize);
    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(writers + 1));
    let space = sp("work");
    let conn = b.open_space(&space)?;
    let writers_conn = (0..writers)
        .map(|_| b.open_space(&space))
        .collect::<Result<Vec<_>, _>>()?;
    let reader = {
        let (done, start) = (Arc::clone(&done), Arc::clone(&start));
        std::thread::spawn(move || -> Result<Vec<i64>, String> {
            let count = || {
                conn.query_row(
                    "SELECT count(*) FROM facts WHERE status = 'active'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| format!("count: {e}"))
            };
            let first = count();
            start.wait();
            let mut seen = vec![first?];
            while !done.load(Ordering::SeqCst) {
                let n = count()?;
                let hits = facts::recall(&conn, "ka", &RecallOptions::default(), NOW)
                    .map_err(|e| format!("recall: {e}"))?;
                if hits.iter().any(|h| h.fact.text.is_empty()) {
                    return Err("an empty fact".into());
                }
                seen.push(n);
            }
            seen.push(count()?);
            Ok(seen)
        })
    };
    let handles: Vec<_> = writers_conn
        .into_iter()
        .enumerate()
        .map(|(w, conn)| {
            let start = Arc::clone(&start);
            let space = sp("work");
            std::thread::spawn(move || {
                start.wait();
                (0..each)
                    .filter_map(|i| {
                        let text = format!("{} ka", words((w * 1000 + i) as u64));
                        match facts::remember(
                            &conn,
                            &space,
                            "me",
                            NewFact::agent(&text, "agent:t"),
                            &[],
                        ) {
                            Ok(Remembered::Created(_)) => None,
                            other => Some(format!("writer {w}, write {i}: {other:?}")),
                        }
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let failures: Vec<_> = handles
        .into_iter()
        .flat_map(|h| {
            h.join()
                .unwrap_or_else(|_| vec!["a writer panicked".into()])
        })
        .collect();
    done.store(true, Ordering::SeqCst);
    let seen = reader.join().map_err(|_| "the reader panicked")?;
    assert!(
        failures.is_empty() && seen.is_ok(),
        "writers: {failures:?}, reader: {seen:?}"
    );
    let seen = seen?;
    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&((writers * each) as i64)));
    assert_eq!(active(&b, "work"), (writers * each) as i64);
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
    let b = brain(&t)?;
    let n = 8;
    let start = Arc::new(Barrier::new(n));
    let connections = (0..n)
        .map(|_| b.open_space(&sp("work")))
        .collect::<Result<Vec<_>, _>>()?;
    let handles: Vec<_> = connections
        .into_iter()
        .map(|conn| {
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                facts::remember(
                    &conn,
                    &sp("work"),
                    "me",
                    NewFact::agent("The boiler is serviced every autumn.", "agent:t"),
                    &[],
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join()).collect();
    assert!(
        results.iter().all(|r| matches!(r, Ok(Ok(_)))),
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Ok(Ok(Remembered::Created(_)))))
            .count(),
        1
    );
    assert_eq!(active(&b, "work"), 1);
    Ok(())
}

#[test]
fn many_threads_superseding_one_fact_leave_one_winner_and_a_whole_history() -> Res {
    let t = Temp::new("conc-race");
    let b = brain(&t)?;
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
    let connections = (0..n)
        .map(|_| b.open_space(&space))
        .collect::<Result<Vec<_>, _>>()?;
    let handles: Vec<_> = connections
        .into_iter()
        .enumerate()
        .map(|(i, conn)| {
            let (start, id) = (Arc::clone(&start), id.clone());
            std::thread::spawn(move || {
                let space = sp("work");
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
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join()).collect();
    assert_eq!(
        results.iter().filter(|r| matches!(r, Ok(Ok(_)))).count(),
        1,
        "{results:?}"
    );
    assert!(
        results.iter().all(|r| matches!(r, Ok(Ok(_)))
            || matches!(r, Ok(Err(Error::Invalid(msg))) if msg.contains("is replaced:"))),
        "{results:?}"
    );
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
    let b = brain(&t)?;
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
    let (c1, c2) = (b.open_space(&space)?, b.open_space(&space)?);
    let (s1, id1) = (Arc::clone(&start), id.clone());
    let retire = std::thread::spawn(move || {
        s1.wait();
        facts::retire(&c1, "me", &id1, "obsolete")
    });
    let id2 = id.clone();
    let supersede = std::thread::spawn(move || {
        let space = sp("work");
        start.wait();
        facts::supersede(
            &c2,
            &space,
            "me",
            &id2,
            NewFact::agent("The garage code changed in spring.", "agent:t"),
            None,
        )
    });
    let (r, s) = (retire.join(), supersede.join());
    assert!(
        matches!((&r, &s), (Ok(Ok(_)), Ok(Err(Error::Invalid(msg)))) if msg.contains("is retired:"))
            || matches!((&r, &s), (Ok(Err(Error::Invalid(msg))), Ok(Ok(_))) if msg.contains("is replaced:")),
        "retire: {r:?}, supersede: {s:?}"
    );
    let conn = b.open_space(&space)?;
    let status: String = conn.query_row("SELECT status FROM facts WHERE id = ?1", [&id], |r| {
        r.get(0)
    })?;
    assert_eq!(
        status,
        if matches!(r, Ok(Ok(_))) {
            "retired"
        } else {
            "replaced"
        }
    );
    Ok(())
}

/// SQLite counts the memory it takes under one lock of the whole process unless it is built without that count: with
/// it, every connection a worker opens (and the server opens several per request) waits for every other, and eight
/// clients got fewer answers than one. The build of this workspace turns it off (`.cargo/config.toml`).
#[test]
fn sqlite_is_built_without_the_process_wide_memory_count() -> Res {
    let t = Temp::new("memstatus");
    let b = brain(&t)?;
    let conn = b.open_space(&sp("personal"))?;
    let mut st = conn.prepare("PRAGMA compile_options")?;
    let options: Vec<String> = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    assert!(
        options.iter().any(|o| o == "DEFAULT_MEMSTATUS=0"),
        "{options:?}"
    );
    Ok(())
}
