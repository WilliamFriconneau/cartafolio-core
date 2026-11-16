//! An app that deposits under its own record ids: no copy of its own, its own earlier version
//! merged into (never overwritten), a cursor to resume from, and every source kept next to the others.

mod common;

use common::{Res, Temp, sp};
use memcp_core::Brain;
use memcp_core::facts::{self, NewFact, RecallOptions};
use memcp_core::origin::{self, Deposit, Origin};
use memcp_core::text::Language;

const NOW: i64 = 1_900_000_000;

struct Bench {
    _t: Temp,
    conn: rusqlite::Connection,
}

fn bench() -> Result<Bench, Box<dyn std::error::Error>> {
    let t = Temp::new("origin");
    let brain = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = brain.open_space(&sp("health"))?;
    Ok(Bench { _t: t, conn })
}

fn send(
    b: &Bench,
    app: &str,
    id: &str,
    at: &str,
    text: &str,
) -> Result<Deposit, Box<dyn std::error::Error>> {
    let o = Origin::parse(app, id, Some(at))?;
    Ok(origin::deposit(
        &b.conn,
        &sp("health"),
        "sync",
        NewFact::agent(text, "agent:sync"),
        &o,
        NOW,
    )?)
}

#[test]
fn the_same_record_sent_twice_is_held_once() -> Res {
    let b = bench()?;
    let first = send(
        &b,
        "bevel",
        "2026-11-01/resting-hr",
        "2026-11-01",
        "Resting heart rate 2026-11-01: 58 bpm",
    )?;
    assert!(matches!(first, Deposit::Created(_)));
    let again = send(
        &b,
        "bevel",
        "2026-11-01/resting-hr",
        "2026-11-01",
        "Resting heart rate 2026-11-01: 58 bpm",
    )?;
    assert!(matches!(again, Deposit::Unchanged(_)), "{again:?}");
    assert_eq!(origin::cursor(&b.conn, "bevel")?.facts, 1);
    Ok(())
}

#[test]
fn a_changed_text_record_is_appended_to_not_overwritten() -> Res {
    let b = bench()?;
    send(
        &b,
        "bevel",
        "2026-11-01/steps",
        "2026-11-01",
        "Steps 2026-11-01: 4200 so far",
    )?;
    let Deposit::Updated { fact, replaced } = send(
        &b,
        "bevel",
        "2026-11-01/steps",
        "2026-11-01",
        "Steps 2026-11-01: 9100",
    )?
    else {
        return Err("not updated".into());
    };
    assert!(
        fact.text.contains("4200 so far") && fact.text.contains("9100"),
        "{}",
        fact.text
    );
    let old = facts::read_fact(&b.conn, &replaced)?.ok_or("old gone")?;
    assert_eq!(
        old.status, "replaced",
        "the history keeps the first version"
    );
    assert_eq!(
        origin::cursor(&b.conn, "bevel")?.facts,
        1,
        "one active record"
    );
    // A text already contained in the held one changes nothing.
    let again = send(
        &b,
        "bevel",
        "2026-11-01/steps",
        "2026-11-01",
        "Steps 2026-11-01: 9100",
    )?;
    assert!(matches!(again, Deposit::Unchanged(_)), "{again:?}");
    // Another reading is appended again.
    let third = send(
        &b,
        "bevel",
        "2026-11-01/steps",
        "2026-11-01",
        "Steps 2026-11-01: 9400",
    )?;
    assert!(matches!(third, Deposit::Updated { .. }));
    Ok(())
}

fn active_text(
    b: &Bench,
    app: &str,
    id: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let fid: String = b.conn.query_row(
        "SELECT o.fact_id FROM fact_origin o JOIN facts f ON f.id = o.fact_id WHERE o.app = ?1 AND o.ext_id = ?2 AND f.status = 'active'",
        [app, id],
        |r| r.get(0),
    )?;
    let f = facts::read_fact(&b.conn, &fid)?.ok_or("gone")?;
    Ok(serde_json::from_str(&f.text)?)
}

#[test]
fn a_json_record_merges_field_by_field_and_a_null_erases_nothing() -> Res {
    let b = bench()?;
    let id = "vitals_day/2026-11-05";
    send(
        &b,
        "kalios",
        id,
        "2026-11-05",
        r#"{"date":"2026-11-05","steps":100,"sleep_total_h":7.5,"resting_heart_rate_bpm":50,"naps":[{"min":20}],"extra":{"a":1,"b":2}}"#,
    )?;
    // A thinner reading: a new field is added, a non-null value updates, null / empty / absent erase nothing.
    let Deposit::Updated { replaced, .. } = send(
        &b,
        "kalios",
        id,
        "2026-11-05",
        r#"{"date":"2026-11-05","steps":900,"sleep_total_h":null,"naps":[],"stand":1,"extra":{"b":3,"c":null}}"#,
    )?
    else {
        return Err("not updated".into());
    };
    let day = active_text(&b, "kalios", id)?;
    assert_eq!(day["steps"], 900);
    assert_eq!(day["sleep_total_h"], 7.5);
    assert_eq!(day["resting_heart_rate_bpm"], 50);
    assert_eq!(day["naps"], serde_json::json!([{"min": 20}]));
    assert_eq!(day["stand"], 1);
    assert_eq!(day["extra"], serde_json::json!({"a": 1, "b": 3}));
    // The history keeps the earlier version.
    let old = facts::read_fact(&b.conn, &replaced)?.ok_or("old gone")?;
    assert_eq!(old.status, "replaced");
    assert!(old.text.contains("\"steps\":100"));
    // A reading that adds nothing is not a new version.
    let none = send(
        &b,
        "kalios",
        id,
        "2026-11-05",
        r#"{"date":"2026-11-05","steps":null,"stand":1}"#,
    )?;
    assert!(matches!(none, Deposit::Unchanged(_)), "{none:?}");
    assert_eq!(origin::cursor(&b.conn, "kalios")?.facts, 1);
    Ok(())
}

#[test]
fn the_dossier_layer_is_still_rewritten_whole() -> Res {
    let b = bench()?;
    let dep = |text: &str| -> Result<Deposit, Box<dyn std::error::Error>> {
        let o =
            Origin::parse("kalios", "d/1", Some("2026-11-05"))?.in_layer(origin::Layer::Dossier);
        Ok(origin::deposit(
            &b.conn,
            &sp("health"),
            "sync",
            NewFact::agent(text, "agent:sync"),
            &o,
            NOW,
        )?)
    };
    dep(r#"{"a":1,"b":2}"#)?;
    dep(r#"{"a":3}"#)?;
    let fid: String = b.conn.query_row(
        "SELECT o.fact_id FROM fact_origin o JOIN facts f ON f.id = o.fact_id WHERE o.layer = 'dossier' AND f.status = 'active'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(
        facts::read_fact(&b.conn, &fid)?.ok_or("gone")?.text,
        r#"{"a":3}"#
    );
    Ok(())
}

#[test]
fn the_cursor_is_the_latest_point_held_for_that_app() -> Res {
    let b = bench()?;
    assert_eq!(
        origin::cursor(&b.conn, "bevel")?.last_at,
        None,
        "nothing yet: start from the beginning"
    );
    send(
        &b,
        "bevel",
        "a",
        "2026-10-30T07:00:00Z",
        "Weight 2026-10-30: 71.2 kg",
    )?;
    send(
        &b,
        "bevel",
        "b",
        "2026-11-01T07:00:00Z",
        "Weight 2026-11-01: 71.0 kg",
    )?;
    send(
        &b,
        "bevel",
        "c",
        "2026-10-31T07:00:00Z",
        "Weight 2026-10-31: 71.1 kg",
    )?;
    let c = origin::cursor(&b.conn, "bevel")?;
    assert_eq!(c.last_at.as_deref(), Some("2026-11-01T07:00:00Z"));
    assert_eq!(c.facts, 3);
    let all = origin::sources(&b.conn)?;
    assert_eq!(all.len(), 1);
    Ok(())
}

#[test]
fn two_apps_sending_the_same_measure_are_both_kept() -> Res {
    let b = bench()?;
    send(
        &b,
        "bevel",
        "x",
        "2026-11-01",
        "Resting heart rate 2026-11-01: 58 bpm (Bevel)",
    )?;
    send(
        &b,
        "healthfit",
        "y",
        "2026-11-01",
        "Resting heart rate 2026-11-01: 58 bpm (HealthFit)",
    )?;
    assert_eq!(origin::sources(&b.conn)?.len(), 2);
    // Each is found under its own source, and both under the parent path.
    let find = |tag: &str| -> Result<usize, Box<dyn std::error::Error>> {
        let opts = RecallOptions {
            limit: 10,
            tags: vec![tag.into()],
            ..Default::default()
        };
        Ok(facts::recall(&b.conn, "resting heart rate", &opts, NOW)?.len())
    };
    assert_eq!(find("sources/bevel")?, 1);
    assert_eq!(find("sources/healthfit")?, 1);
    assert_eq!(find("sources")?, 2);
    Ok(())
}

#[test]
fn a_feed_of_alike_records_is_never_asked_to_decide() -> Res {
    let b = bench()?;
    for day in 1..=9 {
        let d = send(
            &b,
            "bevel",
            &format!("2026-11-0{day}/sleep"),
            &format!("2026-11-0{day}"),
            &format!("Sleep 2026-11-0{day}: 7 h 10 min"),
        )?;
        assert!(matches!(d, Deposit::Created(_)), "day {day}: {d:?}");
    }
    assert_eq!(origin::cursor(&b.conn, "bevel")?.facts, 9);
    Ok(())
}

#[test]
fn what_the_app_gives_is_checked() {
    for (app, id, at) in [
        ("", "x", None),
        ("Bad App!", "x", None),
        ("bevel", "", None),
        ("bevel", "x", Some("yesterday")),
        ("bevel", "a\u{7}b", None),
    ] {
        assert!(Origin::parse(app, id, at).is_err(), "{app:?} {id:?} {at:?}");
    }
    assert!(Origin::parse("Health-Fit_2", "id 1", Some("2026-11-01T07:30:00Z")).is_ok());
}

#[test]
fn identical_text_from_another_source_is_said_not_silently_lost() -> Res {
    let b = bench()?;
    send(
        &b,
        "bevel",
        "x",
        "2026-11-01",
        "Resting heart rate 2026-11-01: 58 bpm",
    )?;
    let other = send(
        &b,
        "healthfit",
        "y",
        "2026-11-01",
        "Resting heart rate 2026-11-01: 58 bpm",
    )?;
    assert!(matches!(other, Deposit::SameTextHeld(_)), "{other:?}");
    assert_eq!(
        origin::cursor(&b.conn, "healthfit")?.facts,
        0,
        "its cursor did not move"
    );
    // Naming the source keeps both.
    let named = send(
        &b,
        "healthfit",
        "y",
        "2026-11-01",
        "[HealthFit] Resting heart rate 2026-11-01: 58 bpm",
    )?;
    assert!(matches!(named, Deposit::Created(_)));
    Ok(())
}

#[test]
fn a_database_from_before_origins_gets_the_table_and_keeps_its_facts() -> Res {
    let t = Temp::new("origin-upgrade");
    let brain = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = brain.open_space(&sp("health"))?;
    let f = facts::remember(
        &conn,
        &sp("health"),
        "test",
        NewFact::agent("an old fact", "agent:test"),
        &[],
    )?;
    let facts::Remembered::Created(f) = f else {
        return Err("not created".into());
    };
    drop(conn);
    {
        let c = rusqlite::Connection::open(t.path().join("data").join("health.sqlite"))?;
        c.execute_batch("DROP TABLE library_raised; DROP TABLE library_links; DROP TABLE library_fts; DROP TABLE library_entries; DROP TABLE library_roots; DROP TABLE fact_origin; DROP TABLE app_guides; PRAGMA user_version = 5;")?;
    }
    let conn = brain.open_space(&sp("health"))?;
    let back = facts::read_fact(&conn, &f.id)?.ok_or("fact lost")?;
    assert_eq!(back.text, "an old fact");
    assert!(origin::sources(&conn)?.is_empty());
    Ok(())
}

#[test]
fn a_standard_code_files_the_same_measure_from_every_source_together() -> Res {
    let b = bench()?;
    let send_coded = |app: &str, id: &str, text: &str| -> Res {
        let o = Origin::parse(app, id, Some("2026-11-01"))?.with_code("LOINC:8867-4")?;
        origin::deposit(
            &b.conn,
            &sp("health"),
            "sync",
            NewFact::agent(text, "agent:sync"),
            &o,
            NOW,
        )?;
        Ok(())
    };
    send_coded("bevel", "hr", "[Bevel] Heart rate 2026-11-01 08:00: 61 bpm")?;
    send_coded(
        "healthfit",
        "hr",
        "[HealthFit] Heart rate 2026-11-01 08:00: 61 bpm",
    )?;
    let opts = RecallOptions {
        limit: 10,
        tags: vec!["code/loinc/8867-4".into(), "sources".into()],
        ..Default::default()
    };
    assert_eq!(facts::recall(&b.conn, "heart rate", &opts, NOW)?.len(), 2);
    Ok(())
}

#[test]
fn a_malformed_code_is_refused() {
    let o = || Origin::parse("bevel", "x", None);
    for bad in [
        "8867-4",
        "loinc:",
        ":8867",
        "lo inc:1",
        "loinc:a/b",
        "loinc:x y",
    ] {
        assert!(o().and_then(|o| o.with_code(bad)).is_err(), "{bad}");
    }
    assert!(o().and_then(|o| o.with_code("snomed:271649006")).is_ok());
}

#[test]
fn the_dossier_is_a_layer_of_its_own_and_is_read_where_the_raw_is_not() -> Res {
    let b = bench()?;
    // Two apps deposit the same heart rate, a little apart.
    for (app, text) in [
        ("bevel", "[Bevel] Heart rate 2026-10-31 14:56: 61 bpm"),
        (
            "healthfit",
            "[HealthFit] Heart rate 2026-10-31 14:56: 63 bpm",
        ),
    ] {
        let o = Origin::parse(app, "hr-1456", Some("2026-10-31T14:56:00Z"))?
            .with_code("loinc:8867-4")?;
        origin::deposit(
            &b.conn,
            &sp("health"),
            "sync",
            NewFact::agent(text, "agent:sync"),
            &o,
            NOW,
        )?;
    }
    // A plain recall finds neither: the raw is a layer apart.
    let plain = RecallOptions {
        limit: 10,
        ..Default::default()
    };
    assert!(facts::recall(&b.conn, "heart rate", &plain, NOW)?.is_empty());
    // The expert reader reconciles them into one entry, with the spread, in the dossier layer.
    let o = Origin::parse(
        "healthapp",
        "2026-10-31T14:56/hr",
        Some("2026-10-31T14:56:00Z"),
    )?
    .with_code("loinc:8867-4")?
    .in_layer(origin::Layer::Dossier);
    let d = origin::deposit(
        &b.conn,
        &sp("health"),
        "healthapp",
        NewFact::agent(
            "Heart rate 2026-10-31 14:56: 62 bpm (2 sources: 61 Bevel, 63 HealthFit)",
            "agent:healthapp",
        ),
        &o,
        NOW,
    )?;
    let Deposit::Created(f) = d else {
        return Err("not created".into());
    };
    assert!(
        f.tags.iter().any(|t| t == "dossier/healthapp"),
        "{:?}",
        f.tags
    );
    assert!(!f.tags.iter().any(|t| t.starts_with("sources")));
    let hits = facts::recall(&b.conn, "heart rate", &plain, NOW)?;
    assert_eq!(
        hits.len(),
        1,
        "a plain recall now finds the one sorted entry"
    );
    assert!(hits[0].fact.text.contains("62 bpm"));
    // The raw is still there when asked for, and the layers are counted apart.
    let raw = RecallOptions {
        limit: 10,
        tags: vec!["sources".into()],
        ..Default::default()
    };
    assert_eq!(facts::recall(&b.conn, "heart rate", &raw, NOW)?.len(), 2);
    assert_eq!(origin::sources(&b.conn)?.len(), 2);
    assert_eq!(origin::dossiers(&b.conn)?.len(), 1);
    // Rewriting the entry under the same id updates it in place, in its own layer.
    let again = origin::deposit(
        &b.conn,
        &sp("health"),
        "healthapp",
        NewFact::agent(
            "Heart rate 2026-10-31 14:56: 62 bpm (3 sources: 61, 62, 63)",
            "agent:healthapp",
        ),
        &Origin::parse(
            "healthapp",
            "2026-10-31T14:56/hr",
            Some("2026-10-31T14:56:00Z"),
        )?
        .in_layer(origin::Layer::Dossier),
        NOW,
    )?;
    assert!(matches!(again, Deposit::Updated { .. }), "{again:?}");
    Ok(())
}

/// Kalios sends `[Kalios] event <id>: {json}`: a label before the object. Two versions of it merge
/// field by field, like plain JSON, instead of the whole text being appended at every send.
#[test]
fn a_labelled_json_record_merges_field_by_field() -> Res {
    let b = bench()?;
    let label = "[Kalios] event E1: ";
    send(
        &b,
        "kalios",
        "event/E1",
        "2026-11-06",
        &format!("{label}{{\"text\":\"note\",\"mood\":3}}"),
    )?;
    let Deposit::Updated { fact, .. } = send(
        &b,
        "kalios",
        "event/E1",
        "2026-11-06",
        &format!("{label}{{\"text\":\"note\",\"mood\":null,\"energy\":2}}"),
    )?
    else {
        return Err("not updated".into());
    };
    assert_eq!(fact.text.matches("[Kalios]").count(), 1, "{}", fact.text);
    let json: serde_json::Value =
        serde_json::from_str(fact.text.strip_prefix(label).ok_or("label lost")?)?;
    assert_eq!(json["mood"], 3, "a null erases nothing");
    assert_eq!(json["energy"], 2);
    Ok(())
}

/// No size limit: a record that keeps growing is held whole, and a long text is kept as sent.
#[test]
fn there_is_no_size_limit() -> Res {
    let b = bench()?;
    let first = format!("[Kalios] event E2 (part 1/2): {}", "a".repeat(3000));
    let second = format!("[Kalios] event E2 (part 1/2): {}", "b".repeat(3000));
    send(&b, "kalios", "event/E2#1", "2026-11-06", &first)?;
    let Deposit::Updated { fact, .. } = send(&b, "kalios", "event/E2#1", "2026-11-06", &second)?
    else {
        return Err("not updated".into());
    };
    assert!(
        fact.text.contains(&first) && fact.text.contains(&second),
        "nothing dropped"
    );
    let long = "x".repeat(200_000);
    let Deposit::Created(f) = send(&b, "kalios", "event/long", "2026-11-06", &long)? else {
        return Err("not created".into());
    };
    assert_eq!(f.text.len(), 200_000);
    let id = "y".repeat(500);
    send(&b, "kalios", &id, "2026-11-06", "a long id")?;
    Ok(())
}

#[test]
fn a_cursor_also_says_when_the_memory_last_received_something() -> Res {
    let b = bench()?;
    send(
        &b,
        "bevel",
        "a",
        "2999-01-01T00:00:00Z",
        "Weight 2999-01-01: 71.0 kg",
    )?;
    let c = origin::cursor(&b.conn, "bevel")?;
    assert_eq!(c.last_at.as_deref(), Some("2999-01-01T00:00:00Z"));
    let got = c.received_at.ok_or("received_at")?;
    assert!(
        got.as_str() < "2999",
        "a real instant, not the app's word: {got}"
    );
    Ok(())
}
