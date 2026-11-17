//! Tags: short hierarchical labels on a fact. A tag matches itself and its descendants,
//! a correction keeps the tags of what it corrects, and the tags in use are listed.

mod common;

use std::error::Error;

use common::{Res, Temp, sp};
use memcp_core::Brain;
use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};
use memcp_core::text::Language;

struct Bench {
    _t: Temp,
    conn: rusqlite::Connection,
}

fn bench() -> Result<Bench, Box<dyn Error>> {
    let t = Temp::new("tags");
    let brain = Brain::init(&t.path().join("data"), &[sp("work")], Language::English)?;
    let conn = brain.open_space(&sp("work"))?;
    Ok(Bench { _t: t, conn })
}

fn tags(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn put(b: &Bench, text: &str, with: &[&str]) -> Result<facts::Fact, Box<dyn Error>> {
    let mut n = NewFact::agent(text, "agent:test");
    n.tags = tags(with);
    match facts::remember(&b.conn, &sp("work"), "test", n, &[])? {
        Remembered::Created(f) => Ok(f),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn find(b: &Bench, query: &str, with: &[&str]) -> Result<Vec<String>, Box<dyn Error>> {
    let opts = RecallOptions {
        limit: 10,
        tags: tags(with),
        ..Default::default()
    };
    Ok(facts::recall(&b.conn, query, &opts, 1_900_000_000)?
        .into_iter()
        .map(|h| h.fact.text)
        .collect())
}

#[test]
fn a_tag_is_normalised_and_the_rules_are_told() -> Res {
    for (raw, want) in [
        (" Work/Dev ", "work/dev"),
        ("Évaluation / Maths", "evaluation/maths"),
        ("home/garden-2_b", "home/garden-2_b"),
        ("@Health", "@health"),
    ] {
        assert_eq!(facts::normalize_tag(raw)?, want, "{raw}");
    }
    for bad in ["", "/a", "a/", "a//b", "a$b", "@", "@a/b", "@9lives"] {
        let e = facts::normalize_tag(bad).err().map(|e| e.to_string());
        assert!(
            e.is_some_and(|m| m.contains("lowercase segments")),
            "{bad:?} should be refused with the rule"
        );
    }
    let many: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
    assert_eq!(
        facts::normalize_tags(&many)?.len(),
        30,
        "no limit on the number of tags"
    );
    assert!(
        !facts::normalize_tag("a/b/c/d/e/f/g")?.is_empty(),
        "nor on the depth"
    );
    assert!(
        facts::normalize_tag(&"x".repeat(200)).is_ok(),
        "nor on the length"
    );
    assert_eq!(
        facts::normalize_tags(&tags(&["b", "A", "a"]))?,
        tags(&["a", "b"]),
        "sorted, without repeats"
    );
    Ok(())
}

#[test]
fn a_tag_matches_itself_and_its_descendants_only() -> Res {
    let b = bench()?;
    put(&b, "the build runs on every push", &["work/dev/ci"])?;
    put(&b, "release notes are written on friday", &["work/writing"])?;
    put(&b, "the build of the shed started", &["home/shed"])?;
    let texts = |t: &[&str]| find(&b, "build", t);
    assert_eq!(texts(&[])?.len(), 2, "no tag: both facts about a build");
    assert_eq!(texts(&["work"])?, ["the build runs on every push"]);
    assert_eq!(texts(&["work/dev"])?, ["the build runs on every push"]);
    assert_eq!(texts(&["work/dev/ci"])?, ["the build runs on every push"]);
    assert!(
        texts(&["work/de"])?.is_empty(),
        "a prefix of letters is not a parent"
    );
    assert!(texts(&["work/dev/ci/x"])?.is_empty());
    assert_eq!(texts(&["home"])?, ["the build of the shed started"]);
    assert!(
        texts(&["work", "home"])?.is_empty(),
        "all the tags must match"
    );
    Ok(())
}

#[test]
fn a_fact_shows_its_tags_and_a_correction_keeps_them() -> Res {
    let b = bench()?;
    let f = put(
        &b,
        "the planning meeting is on monday",
        &["work/meetings", "work/planning"],
    )?;
    assert_eq!(f.tags, tags(&["work/meetings", "work/planning"]));
    // A correction with no tags of its own keeps them.
    let kept = facts::supersede(
        &b.conn,
        &sp("work"),
        "test",
        &f.id,
        NewFact::agent("the planning meeting is on tuesday", "agent:test"),
        None,
    )?;
    assert_eq!(kept.tags, f.tags);
    // One that brings tags replaces them.
    let changed = facts::supersede(
        &b.conn,
        &sp("work"),
        "test",
        &kept.id,
        {
            let mut n = NewFact::agent("the planning meeting is on wednesday", "agent:test");
            n.tags = tags(&["work/planning"]);
            n
        },
        None,
    )?;
    assert_eq!(changed.tags, tags(&["work/planning"]));
    // The history shows each version with its own tags.
    let chain = facts::history(&b.conn, &changed.id)?;
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].tags, f.tags);
    assert_eq!(chain[2].tags, changed.tags);
    // Only the live version answers a search by tag.
    assert_eq!(find(&b, "planning meeting", &["work/meetings"])?.len(), 0);
    assert_eq!(find(&b, "planning meeting", &["work/planning"])?.len(), 1);
    Ok(())
}

#[test]
fn the_tags_in_use_read_as_a_tree_of_counts() -> Res {
    let b = bench()?;
    put(&b, "alpha note one", &["work/dev/ci", "work/dev"])?;
    put(&b, "beta note two", &["work/dev"])?;
    put(&b, "gamma note three", &["work/writing"])?;
    put(&b, "delta note four", &["@health", "home"])?;
    let counts = facts::tag_counts(&b.conn)?;
    let get = |t: &str| counts.iter().find(|(n, _)| n == t).map(|(_, n)| *n);
    assert_eq!(get("work"), Some(3), "a fact counts once for each level");
    assert_eq!(get("work/dev"), Some(2));
    assert_eq!(get("work/dev/ci"), Some(1));
    assert_eq!(get("work/writing"), Some(1));
    assert_eq!(get("home"), Some(1));
    assert_eq!(get("@health"), None, "concern tags are not folders");
    assert_eq!(counts[0].0, "work", "the most used first");
    // A retired fact leaves the count.
    let gone = put(&b, "epsilon note five", &["work/old"])?;
    assert_eq!(
        facts::tag_counts(&b.conn)?
            .iter()
            .find(|(n, _)| n == "work/old")
            .map(|(_, n)| *n),
        Some(1)
    );
    facts::retire(&b.conn, "test", &gone.id, "not needed")?;
    assert!(
        facts::tag_counts(&b.conn)?
            .iter()
            .all(|(n, _)| n != "work/old")
    );
    Ok(())
}

#[test]
fn a_database_from_before_tags_gets_the_table_and_keeps_its_facts() -> Res {
    let t = Temp::new("tags-upgrade");
    let brain = Brain::init(&t.path().join("data"), &[sp("work")], Language::English)?;
    let conn = brain.open_space(&sp("work"))?;
    let f = facts::remember(
        &conn,
        &sp("work"),
        "test",
        NewFact::agent("an old fact", "agent:test"),
        &[],
    )?;
    let Remembered::Created(f) = f else {
        return Err("not created".into());
    };
    drop(conn);
    {
        let c = rusqlite::Connection::open(t.path().join("data").join("work.sqlite"))?;
        c.execute_batch("DROP TABLE window_gone; DROP TABLE vocabulary_terms; DROP TABLE vocabularies; DROP TABLE library_raised; DROP TABLE library_links; DROP TABLE library_fts; DROP TABLE library_entries; DROP TABLE library_roots; DROP TABLE fact_tags; DROP TABLE fact_origin; DROP TABLE app_guides; DROP TABLE sync_log; DROP TABLE sync_fields; DROP TABLE sync_disputes; PRAGMA user_version = 4;")?;
    }
    let conn = brain.open_space(&sp("work"))?;
    let back = facts::read_fact(&conn, &f.id)?.ok_or("fact lost")?;
    assert_eq!(back.text, "an old fact");
    assert!(back.tags.is_empty());
    Ok(())
}
