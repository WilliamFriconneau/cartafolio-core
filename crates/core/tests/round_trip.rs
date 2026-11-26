//! Export, adopt and merge as one loop: a memory goes out and comes back, through one installation and another, and
//! nothing is lost; doing it again changes nothing.

mod common;

use std::error::Error;
use std::path::PathBuf;

use common::{Res, Temp, sp};
use pensine_core::Brain;
use pensine_core::export;
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::files;
use pensine_core::merge::{self, Options, Policy};
use pensine_core::text::Language;
use pensine_core::users::{self, OWNER, User};

fn say(b: &Brain, space: &str, text: &str, tags: &[&str]) -> Result<String, Box<dyn Error>> {
    let conn = b.open_space(&sp(space))?;
    let mut n = NewFact::agent(text, "agent:test");
    n.tags = tags.iter().map(|t| (*t).to_string()).collect();
    match facts::remember(&conn, &sp(space), "tester", n, &[])? {
        Remembered::Created(f) => Ok(f.id),
        _ => Err("not created".into()),
    }
}

/// Every fact row, its tags, the history and the files, to see that nothing moved or went missing.
fn digest(b: &Brain, space: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let c = b.open_space(&sp(space))?;
    let mut st = c.prepare(
        "SELECT f.id || '|' || f.text || '|' || f.status || '|' || coalesce(f.replaced_by, '') || '|' ||
                coalesce(f.valid_to, '') || '|' ||
                coalesce((SELECT group_concat(tag) FROM (SELECT tag FROM fact_tags t WHERE t.fact_id = f.id ORDER BY tag)), '')
         FROM facts f ORDER BY f.id",
    )?;
    let mut rows: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let events: i64 = c.query_row("SELECT count(*) FROM fact_events", [], |r| r.get(0))?;
    rows.push(format!("events {events}"));
    for f in files::list(b, &sp(space), None, 100)? {
        let (info, bytes) = files::get(b, &sp(space), &f.id)?.ok_or("file gone")?;
        rows.push(format!("file {} {} {}", info.id, info.name, bytes.len()));
    }
    Ok(rows)
}

struct Chain {
    t: Temp,
    a: Brain,
    sam: User,
}

/// A memory with a little of everything: tags, a replaced fact, a retired one, a validity date, a file.
fn memory() -> Result<Chain, Box<dyn Error>> {
    let t = Temp::new("round-trip");
    let a = Brain::init(&t.path().join("a"), &[], Language::English)?;
    let sam = users::create_user(&a.open_control()?, OWNER, "sam", "Sam", false)?;
    a.add_vault(&sam.id, "sam", "work")?;
    a.add_vault(&sam.id, "sam", "health")?;
    say(
        &a,
        "sam-work",
        "The audit is due on Friday.",
        &["work/audit"],
    )?;
    let old = say(&a, "sam-work", "The printer is in room four.", &[])?;
    let conn = a.open_space(&sp("sam-work"))?;
    facts::supersede(
        &conn,
        &sp("sam-work"),
        "tester",
        &old,
        NewFact::agent("The printer moved to room five.", "agent:test"),
        Some("moved"),
    )?;
    let gone = say(&a, "sam-work", "The ladder is in the shed.", &["home/shed"])?;
    facts::retire(&conn, "tester", &gone, "sold")?;
    say(
        &a,
        "sam-health",
        "Resting heart rate was fifty-eight.",
        &["health/vitals"],
    )?;
    files::put(
        &a,
        &sp("sam-health"),
        "tester",
        "scan.txt",
        "a scan",
        b"scan bytes",
    )?;
    Ok(Chain { t, a, sam })
}

fn second_install(t: &Temp, name: &str) -> Result<(Brain, User), Box<dyn Error>> {
    let b = Brain::init(&t.path().join(name), &[], Language::English)?;
    let kim = users::create_user(&b.open_control()?, OWNER, "kim", "Kim", false)?;
    Ok((b, kim))
}

#[test]
fn a_memory_goes_out_and_back_through_two_installations_and_loses_nothing() -> Res {
    let c = memory()?;
    let first = export::export_person(&c.a, &c.sam, &c.t.path().join("e1"), 1_900_000_000)?;
    let (b, kim) = second_install(&c.t, "b")?;
    export::adopt(&b, &first, &kim, None)?;
    // And on again, from the second installation to a third.
    let second = export::export_person(&b, &kim, &c.t.path().join("e2"), 1_900_000_100)?;
    let (d, lea) = second_install(&c.t, "d")?;
    export::adopt(&d, &second, &lea, None)?;
    for v in ["sam-work", "sam-health"] {
        let want = digest(&c.a, v)?;
        assert_eq!(digest(&b, v)?, want, "{v} after one hop");
        assert_eq!(digest(&d, v)?, want, "{v} after two hops");
    }
    // The replaced fact keeps its history through both hops.
    let conn = d.open_space(&sp("sam-work"))?;
    let chain: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE status = 'replaced' AND replaced_by IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(chain, 1);
    Ok(())
}

#[test]
fn merging_a_memory_into_its_own_copy_changes_nothing_and_a_second_run_still_nothing() -> Res {
    let c = memory()?;
    let first = export::export_person(&c.a, &c.sam, &c.t.path().join("e1"), 1_900_000_000)?;
    let (b, kim) = second_install(&c.t, "b")?;
    export::adopt(&b, &first, &kim, None)?;
    let before: Vec<_> = ["sam-work", "sam-health"]
        .iter()
        .map(|v| digest(&b, v))
        .collect::<Result<_, _>>()?;
    let opts = Options {
        backup_to: c.t.path().join("backups"),
        at: 1_900_000_300,
        stop_after: None,
    };
    for _ in 0..2 {
        let plan = merge::plan(&b, &first, &kim, Policy::KeepBoth)?;
        let report = merge::apply(&b, &plan, &opts)?;
        assert!(!report.changed(), "{report:?}");
        let after: Vec<_> = ["sam-work", "sam-health"]
            .iter()
            .map(|v| digest(&b, v))
            .collect::<Result<_, _>>()?;
        assert_eq!(after, before);
    }
    Ok(())
}

#[test]
fn two_copies_that_drifted_merge_into_the_union_both_ways_and_stay_merged() -> Res {
    let c = memory()?;
    let first = export::export_person(&c.a, &c.sam, &c.t.path().join("e1"), 1_900_000_000)?;
    let (b, kim) = second_install(&c.t, "b")?;
    export::adopt(&b, &first, &kim, None)?;
    // Each side writes on its own.
    say(
        &c.a,
        "sam-work",
        "Only on the first side: the roof was fixed.",
        &["home/roof"],
    )?;
    say(
        &c.a,
        "sam-health",
        "Only on the first side: a flu shot.",
        &[],
    )?;
    say(
        &b,
        "sam-work",
        "Only on the second side: the car passed its test.",
        &["car"],
    )?;
    let opts = |tag: &str| Options {
        backup_to: c.t.path().join(tag),
        at: 1_900_000_500,
        stop_after: None,
    };
    // The second side takes the first's news, the first takes the second's.
    let from_a = export::export_person(&c.a, &c.sam, &c.t.path().join("e2"), 1_900_000_400)?;
    let plan = merge::plan(&b, &from_a, &kim, Policy::KeepBoth)?;
    merge::apply(&b, &plan, &opts("bk-b"))?;
    let from_b = export::export_person(&b, &kim, &c.t.path().join("e3"), 1_900_000_450)?;
    let plan = merge::plan(&c.a, &from_b, &c.sam, Policy::KeepBoth)?;
    merge::apply(&c.a, &plan, &opts("bk-a"))?;
    let active = |br: &Brain, v: &str| -> Result<Vec<String>, Box<dyn Error>> {
        let conn = br.open_space(&sp(v))?;
        let mut st =
            conn.prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
    };
    for v in ["sam-work", "sam-health"] {
        let (x, y) = (active(&c.a, v)?, active(&b, v)?);
        assert_eq!(x, y, "{v}: both sides hold the same notes");
    }
    assert!(active(&b, "sam-work")?.iter().any(|t| t.contains("roof")));
    assert!(
        active(&c.a, "sam-work")?
            .iter()
            .any(|t| t.contains("car passed"))
    );
    assert!(
        active(&c.a, "sam-health")?
            .iter()
            .any(|t| t.contains("flu"))
    );
    // Another round moves nothing.
    let again = export::export_person(&b, &kim, &c.t.path().join("e4"), 1_900_000_600)?;
    let plan = merge::plan(&c.a, &again, &c.sam, Policy::KeepBoth)?;
    let report = merge::apply(&c.a, &plan, &opts("bk-a2"))?;
    assert!(!report.changed(), "{report:?}");
    let _: PathBuf = again;
    Ok(())
}
