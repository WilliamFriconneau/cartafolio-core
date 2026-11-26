//! Facts: writing, duplicates, replacement, retirement, history, search (English).
//! A fictitious family (Sam, Dr Martin, the Moon Bakery). No real data.

mod common;

use std::error::Error;
use std::fs;

use common::{Res, Temp, brain, sp};
use pensine_core::Space;
use pensine_core::facts::{self, NewFact, RecallOptions, Remembered};
use rusqlite::Connection;

struct Base {
    _t: Temp,
    space: Space,
    conn: Connection,
}

fn base() -> Result<Base, Box<dyn Error>> {
    let t = Temp::new("facts");
    let space = sp("personal");
    let conn = brain(&t)?.open_space(&space)?;
    Ok(Base { _t: t, space, conn })
}

fn remember(b: &Base, text: &str, distinct: &[String]) -> pensine_core::Result<Remembered> {
    facts::remember(
        &b.conn,
        &b.space,
        "owner",
        NewFact::agent(text, "agent:test"),
        distinct,
    )
}

fn create(b: &Base, text: &str) -> Result<facts::Fact, Box<dyn Error>> {
    match remember(b, text, &[])? {
        Remembered::Created(f) => Ok(f),
        other => Err(format!("expected Created, got {other:?}").into()),
    }
}

fn supersede(
    b: &Base,
    id: &str,
    text: &str,
    reason: Option<&str>,
) -> pensine_core::Result<facts::Fact> {
    facts::supersede(
        &b.conn,
        &b.space,
        "owner",
        id,
        NewFact::agent(text, "agent:test"),
        reason,
    )
}

fn recall(b: &Base, q: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let opts = RecallOptions {
        limit: 10,
        ..Default::default()
    };
    Ok(facts::recall(&b.conn, q, &opts, 1_900_000_000)?
        .into_iter()
        .map(|h| h.fact.text)
        .collect())
}

fn count(b: &Base) -> Result<i64, Box<dyn Error>> {
    Ok(b.conn
        .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))?)
}

#[test]
fn write_then_read_back() -> Res {
    let b = base()?;
    let f = create(&b, "Sam has green tea at four.")?;
    assert!(f.id.starts_with("f_") && f.id.len() == 14);
    assert_eq!(f.status, "active");
    assert_eq!(f.author, "owner");
    assert_eq!(f.source_agent.as_deref(), Some("agent:test"));
    assert_eq!(facts::read_fact(&b.conn, &f.id)?.ok_or("not found")?, f);
    Ok(())
}

#[test]
fn exact_duplicate_dropped_even_with_different_spacing() -> Res {
    let b = base()?;
    create(&b, "Dr Martin sees patients on Tuesdays.")?;
    let r = remember(&b, "  Dr   Martin sees patients\non Tuesdays.  ", &[])?;
    assert!(matches!(r, Remembered::Duplicate(_)), "{r:?}");
    assert_eq!(count(&b)?, 1);
    Ok(())
}

#[test]
fn near_duplicate_writes_nothing_then_distinct_from_confirms() -> Res {
    let b = base()?;
    let a = create(&b, "Sam has green tea at four every afternoon.")?;
    let r = remember(&b, "Sam has green tea at five every afternoon.", &[])?;
    let Remembered::NeedsDecision(c) = r else {
        return Err(format!("expected NeedsDecision, got {r:?}").into());
    };
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].id, a.id);
    assert_eq!(count(&b)?, 1, "nothing was written");

    // The caller confirms it is a distinct fact.
    let r = remember(
        &b,
        "Sam has green tea at five every afternoon.",
        std::slice::from_ref(&a.id),
    )?;
    assert!(matches!(r, Remembered::Created(_)), "{r:?}");
    Ok(())
}

#[test]
fn unrelated_facts_are_not_candidates() -> Res {
    let b = base()?;
    create(&b, "The Moon Bakery opens at nine.")?;
    assert!(matches!(
        remember(&b, "Dr Martin sees patients on Tuesdays.", &[])?,
        Remembered::Created(_)
    ));
    Ok(())
}

#[test]
fn supersede_keeps_the_chain_both_ways() -> Res {
    let b = base()?;
    let v1 = create(&b, "The Moon Bakery opens at nine.")?;
    let v2 = supersede(
        &b,
        &v1.id,
        "The Moon Bakery opens at ten since November.",
        Some("winter hours"),
    )?;
    assert_eq!(v2.replaces.as_deref(), Some(v1.id.as_str()));
    assert_eq!(v2.status, "active");
    let old = facts::read_fact(&b.conn, &v1.id)?.ok_or("old version lost")?;
    assert_eq!(old.status, "replaced");
    assert_eq!(old.replaced_by.as_deref(), Some(v2.id.as_str()));
    assert_eq!(old.text, "The Moon Bakery opens at nine.", "text untouched");
    assert_eq!(old.status_reason.as_deref(), Some("winter hours"));

    // The history is reachable from any version.
    for id in [&v1.id, &v2.id] {
        let ids: Vec<String> = facts::history(&b.conn, id)?
            .into_iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(ids, vec![v1.id.clone(), v2.id.clone()]);
    }
    Ok(())
}

#[test]
fn supersede_refuses_a_replaced_or_retired_fact() -> Res {
    let b = base()?;
    let v1 = create(&b, "Sam has a black cat.")?;
    supersede(&b, &v1.id, "Sam has two black cats.", None)?;
    assert!(
        supersede(&b, &v1.id, "Sam has no cat any more.", None).is_err(),
        "already replaced"
    );

    let v = create(&b, "Sam plays the violin.")?;
    facts::retire(&b.conn, "owner", &v.id, "mistake")?;
    assert!(
        supersede(&b, &v.id, "Sam plays the piano.", None).is_err(),
        "retired"
    );
    Ok(())
}

#[test]
fn retire_keeps_the_text_and_leaves_searches() -> Res {
    let b = base()?;
    let f = create(&b, "Sam moves to Lyon.")?;
    assert_eq!(recall(&b, "Lyon")?.len(), 1);
    let r = facts::retire(&b.conn, "owner", &f.id, "false information")?;
    assert_eq!(r.status, "retired");
    assert_eq!(r.text, "Sam moves to Lyon.");
    assert!(recall(&b, "Lyon")?.is_empty(), "excluded by default");

    let opts = RecallOptions {
        limit: 5,
        include_retired: true,
        ..Default::default()
    };
    let with = facts::recall(&b.conn, "Lyon", &opts, 1_900_000_000)?;
    assert_eq!(with.len(), 1);
    assert_eq!(with[0].fact.status, "retired");

    assert!(facts::retire(&b.conn, "owner", &f.id, "again").is_err());
    assert!(facts::retire(&b.conn, "owner", "f_unknown", "x").is_err());
    Ok(())
}

#[test]
fn retiring_needs_a_reason() -> Res {
    let b = base()?;
    let f = create(&b, "Sam feels dizzy.")?;
    assert!(facts::retire(&b.conn, "owner", &f.id, "   ").is_err());
    Ok(())
}

#[test]
fn search_folds_accents_plurals_and_stopwords() -> Res {
    let b = base()?;
    create(&b, "Solar énergie feeds the horses of the Moon Bakery.")?;
    create(&b, "Dr Martin sees patients on Tuesday mornings.")?;
    assert_eq!(recall(&b, "energie")?.len(), 1, "accents");
    assert_eq!(recall(&b, "ÉNERGIE")?.len(), 1, "case");
    assert_eq!(recall(&b, "horse")?.len(), 1, "plural");
    assert_eq!(recall(&b, "martin tuesday")?.len(), 1);
    assert!(recall(&b, "the of on")?.is_empty(), "stopwords only");
    assert!(recall(&b, "")?.is_empty());
    assert!(recall(&b, "nonexistentxyz")?.is_empty());
    Ok(())
}

#[test]
fn search_survives_special_characters() -> Res {
    let b = base()?;
    create(&b, "Sam likes tea.")?;
    for q in [
        "\"",
        "tea AND",
        "a OR b NEAR(",
        "*",
        "tea\"; DROP TABLE facts;--",
        "(((",
        "-tea",
    ] {
        let _ = recall(&b, q)?;
    }
    assert_eq!(count(&b)?, 1);
    Ok(())
}

#[test]
fn recall_is_ranked_and_deterministic() -> Res {
    let b = base()?;
    create(&b, "Swimming pool open on Saturday for Sam.")?;
    create(
        &b,
        "Sam swims at the pool every Tuesday and Thursday at the city pool.",
    )?;
    let a = recall(&b, "pool")?;
    assert_eq!(a, recall(&b, "pool")?);
    assert_eq!(a.len(), 2);
    Ok(())
}

#[test]
fn as_of_shows_what_was_true() -> Res {
    let b = base()?;
    let v1 = create(&b, "The Moon Bakery opens at nine.")?;
    // Dates are forced so the test does not depend on the clock.
    b.conn.execute(
        "UPDATE facts SET created_at = 1000, valid_from = 1000 WHERE id = ?1",
        [&v1.id],
    )?;
    let v2 = supersede(&b, &v1.id, "The Moon Bakery opens at ten.", None)?;
    b.conn
        .execute("UPDATE facts SET status_at = 2000 WHERE id = ?1", [&v1.id])?;
    b.conn.execute(
        "UPDATE facts SET created_at = 2000, valid_from = 2000 WHERE id = ?1",
        [&v2.id],
    )?;

    let at = |t| -> Result<Vec<String>, Box<dyn Error>> {
        let opts = RecallOptions {
            limit: 10,
            as_of: Some(t),
            ..Default::default()
        };
        Ok(
            facts::recall(&b.conn, "bakery opens", &opts, 9_999_999_999)?
                .into_iter()
                .map(|h| h.fact.text)
                .collect(),
        )
    };
    assert_eq!(
        at(1500)?,
        vec!["The Moon Bakery opens at nine.".to_string()]
    );
    assert_eq!(at(2500)?, vec!["The Moon Bakery opens at ten.".to_string()]);
    Ok(())
}

#[test]
fn an_expired_fact_is_not_served_by_default() -> Res {
    let b = base()?;
    let mut n = NewFact::agent("Sam is on holiday.", "agent:test");
    n.valid_from = Some(1000);
    n.valid_to = Some(2000);
    let Remembered::Created(_) = facts::remember(&b.conn, &b.space, "owner", n, &[])? else {
        return Err("not created".into());
    };
    let default = RecallOptions {
        limit: 5,
        ..Default::default()
    };
    assert!(facts::recall(&b.conn, "holiday", &default, 3000)?.is_empty());
    let with = RecallOptions {
        limit: 5,
        include_expired: true,
        ..Default::default()
    };
    assert_eq!(facts::recall(&b.conn, "holiday", &with, 3000)?.len(), 1);
    Ok(())
}

#[test]
fn empty_or_control_text_is_refused_and_length_is_free() -> Res {
    let b = base()?;
    assert!(remember(&b, "   ", &[]).is_err());
    assert!(remember(&b, "line\u{0007}bell", &[]).is_err());
    assert!(
        remember(&b, &"a".repeat(100_000), &[]).is_ok(),
        "no size limit"
    );
    Ok(())
}

#[test]
fn events_trace_every_change() -> Res {
    let b = base()?;
    let v1 = create(&b, "Sam has a red bike.")?;
    let v2 = supersede(&b, &v1.id, "Sam has a blue bike.", Some("repainted"))?;
    facts::retire(&b.conn, "owner", &v2.id, "sold")?;
    let actions: Vec<String> = b
        .conn
        .prepare("SELECT action FROM fact_events ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(actions, vec!["create", "create", "supersede", "retire"]);
    Ok(())
}

#[test]
fn no_deletion_is_possible_in_the_code() -> Res {
    // Static check: no DELETE on facts (there is no purge yet).
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/facts.rs"))?;
    assert!(!src.to_uppercase().contains("DELETE FROM FACTS"));
    Ok(())
}

// --- tidying at each write -------------------------------------------------------------------

#[test]
fn a_write_in_order_has_nothing_to_tidy() -> Res {
    let b = base()?;
    let f = create(&b, "Sam plays the violin on Tuesdays")?;
    assert!(facts::tidy_after(&b.conn, &f.id, &f.text, 1_000)?.is_empty());
    Ok(())
}

#[test]
fn a_write_names_its_neighbours_without_touching_them() -> Res {
    let b = base()?;
    let old = create(
        &b,
        "Dr Martin moved the bakery appointment to the Moon Bakery street",
    )?;
    let f = create(
        &b,
        "Dr Martin sees Sam on the Moon Bakery street on Friday afternoons now",
    )?;
    let tidy = facts::tidy_after(&b.conn, &f.id, &f.text, 1_000)?;
    assert_eq!(tidy["close"][0]["id"], old.id.as_str(), "{tidy:?}");
    let same = facts::read_fact(&b.conn, &old.id)?.ok_or("gone")?;
    assert_eq!(same.status, "active", "computed, never applied");
    Ok(())
}

#[test]
fn a_write_finds_what_is_past_its_date_and_a_text_too_long() -> Res {
    let b = base()?;
    let mut new = NewFact::agent("The Moon Bakery lease ends in June", "agent:test");
    new.valid_from = Some(100);
    new.valid_to = Some(200);
    let Remembered::Created(old) = facts::remember(&b.conn, &b.space, "owner", new, &[])? else {
        return Err("expected Created".into());
    };
    let long = format!("Sam likes {}", "violin scales and long walks, ".repeat(20));
    let f = create(&b, &long)?;
    let tidy = facts::tidy_after(&b.conn, &f.id, &f.text, 1_000)?;
    assert_eq!(tidy["expired"][0]["id"], old.id.as_str());
    assert!(tidy.contains_key("too_long"));
    Ok(())
}

#[test]
fn the_report_finds_each_close_pair_once_and_changes_nothing() -> Res {
    let b = base()?;
    let a = create(
        &b,
        "Sam plays the violin at the Moon Bakery on Tuesday evenings with Dr Martin",
    )?;
    let distinct = vec![a.id.clone()];
    let Remembered::Created(c) = remember(
        &b,
        "Sam plays the violin at the Moon Bakery on Tuesday evenings with Dr Martin now",
        &distinct,
    )?
    else {
        return Err("expected Created".into());
    };
    create(&b, "The heat pump needs its filter changed every autumn")?;
    let pairs = facts::close_pairs(&b.conn, facts::SAME_FACT_THRESHOLD)?;
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    let ids = [&pairs[0].first.0, &pairs[0].second.0];
    assert!(ids.contains(&&a.id) && ids.contains(&&c.id));
    assert_eq!(facts::active_digests(&b.conn)?.len(), 3, "nothing changed");
    Ok(())
}

#[test]
fn a_text_longer_than_a_peer_takes_is_refused_when_written() -> Res {
    let b = base()?;
    let most = "a".repeat(facts::MAX_TEXT_BYTES);
    create(&b, &most)?;
    let longer = format!("b{most}");
    assert!(matches!(
        remember(&b, &longer, &[]),
        Err(pensine_core::Error::Invalid(_))
    ));
    let short = create(&b, "Fed the cat")?;
    assert!(matches!(
        facts::supersede(
            &b.conn,
            &b.space,
            "owner",
            &short.id,
            NewFact::agent(&longer, "agent:test"),
            None
        ),
        Err(pensine_core::Error::Invalid(_))
    ));
    Ok(())
}
