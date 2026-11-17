//! The inbox: quick notes written as they come and sorted later. They skip the "close facts"
//! question, a plain recall does not return them, the inbox itself can be asked for, and a
//! connection is told how many wait.

mod common;

use common::{Res, Temp, sp};
use memcp_core::Brain;
use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};
use memcp_core::text::Language;

const NOW: i64 = 1_900_000_000;

fn note(
    conn: &rusqlite::Connection,
    text: &str,
    tags: &[&str],
) -> Result<Remembered, Box<dyn std::error::Error>> {
    let mut n = NewFact::agent(text, "agent:test");
    n.tags = tags.iter().map(|s| s.to_string()).collect();
    Ok(facts::remember(conn, &sp("health"), "test", n, &[])?)
}

fn found(conn: &rusqlite::Connection, tags: &[&str]) -> Result<usize, Box<dyn std::error::Error>> {
    let opts = RecallOptions {
        limit: 10,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    Ok(facts::recall(conn, "dizzy", &opts, NOW)?.len())
}

#[test]
fn a_quick_note_is_taken_as_it_comes_and_kept_out_of_a_plain_recall() -> Res {
    let t = Temp::new("inbox");
    let brain = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = brain.open_space(&sp("health"))?;
    note(&conn, "Felt dizzy on standing this morning", &["inbox"])?;
    // Alike, yet no "close facts" question: sorting it out is for later.
    let again = note(&conn, "Felt dizzy on standing this evening", &["inbox"])?;
    assert!(matches!(again, Remembered::Created(_)), "{again:?}");
    assert_eq!(
        found(&conn, &[])?,
        0,
        "a plain recall leaves the inbox alone"
    );
    assert_eq!(
        found(&conn, &["inbox"])?,
        2,
        "asking for the inbox finds it"
    );
    let (n, oldest) = facts::inbox(&conn)?;
    assert_eq!(n, 2);
    assert!(oldest.is_some());
    Ok(())
}

#[test]
fn sorting_a_note_takes_it_out_of_the_inbox_and_keeps_the_raw_one_in_the_history() -> Res {
    let t = Temp::new("inbox-sort");
    let brain = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = brain.open_space(&sp("health"))?;
    let Remembered::Created(raw) = note(&conn, "dizzy on standing", &["inbox"])? else {
        return Err("not created".into());
    };
    let mut clean = NewFact::agent(
        "Dizziness on standing, noted 2026-11-01, no fall",
        "agent:test",
    );
    clean.tags = vec!["symptoms/dizziness".into()];
    let sorted = facts::supersede(&conn, &sp("health"), "test", &raw.id, clean, Some("sorted"))?;
    assert_eq!(facts::inbox(&conn)?.0, 0);
    assert_eq!(found(&conn, &[])?, 1, "now a plain recall finds it");
    let old = facts::read_fact(&conn, &raw.id)?.ok_or("raw gone")?;
    assert_eq!(old.status, "replaced");
    assert_eq!(sorted.replaces.as_deref(), Some(raw.id.as_str()));
    Ok(())
}

#[test]
fn an_inbox_path_counts_as_the_inbox() -> Res {
    let t = Temp::new("inbox-path");
    let brain = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = brain.open_space(&sp("health"))?;
    note(&conn, "dizzy at the pool", &["inbox/voice"])?;
    assert_eq!(found(&conn, &[])?, 0);
    assert_eq!(found(&conn, &["inbox"])?, 1);
    Ok(())
}

#[test]
fn the_inbox_is_read_by_asking_for_its_tag_with_no_words() -> Res {
    let t = Temp::new("inbox-list");
    let brain = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = brain.open_space(&sp("health"))?;
    for (i, text) in [
        "First quick thought",
        "Second quick thought",
        "Third quick thought",
    ]
    .iter()
    .enumerate()
    {
        let Remembered::Created(f) = note(&conn, text, &["inbox"])? else {
            return Err("a note was not created".into());
        };
        let id = f.id;
        // Distinct instants, so "newest first" is a thing to check.
        conn.execute(
            "UPDATE facts SET created_at = ?2 WHERE id = ?1",
            rusqlite::params![id, NOW - 1000 + i as i64],
        )?;
    }
    note(&conn, "A sorted fact about the same", &[])?;
    let Remembered::Created(audio) = note(&conn, "An audio descriptor", &["inbox/audio"])? else {
        return Err("the descriptor was not created".into());
    };
    conn.execute(
        "UPDATE facts SET created_at = ?2 WHERE id = ?1",
        rusqlite::params![audio.id, NOW - 990],
    )?;
    let list = |q: &str,
                tags: &[&str],
                retired: bool|
     -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let opts = RecallOptions {
            limit: 10,
            include_retired: retired,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        Ok(facts::recall(&conn, q, &opts, NOW)?
            .into_iter()
            .map(|h| h.fact.text)
            .collect())
    };
    // No word: what is under the tag, newest first, the audio descriptor included (it is under `inbox/`).
    let all = list("", &["inbox"], false)?;
    assert_eq!(all.len(), 4, "{all:?}");
    assert_eq!(all[0], "An audio descriptor");
    assert!(
        all[1].starts_with("Third") && all[3].starts_with("First"),
        "{all:?}"
    );
    // Only common words count as no word at all.
    assert_eq!(list("the of a", &["inbox"], false)?.len(), 4);
    // A narrower tag narrows the list; no tag and no word is still nothing.
    assert_eq!(list(" ", &["inbox/audio"], false)?, ["An audio descriptor"]);
    assert!(list("", &[], false)?.is_empty());
    // Retired notes leave the list, and come back only when asked for.
    facts::retire(&conn, "test", &audio.id, "sorted")?;
    assert_eq!(list("", &["inbox"], false)?.len(), 3);
    assert_eq!(list("", &["inbox"], true)?.len(), 4);
    // Words still search as before.
    assert_eq!(list("Second", &["inbox"], false)?.len(), 1);
    Ok(())
}

#[test]
fn an_old_note_still_waiting_is_listed_behind_many_sorted_ones() -> Res {
    let t = Temp::new("inbox-window");
    let b = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let conn = b.open_space(&sp("health"))?;
    let Remembered::Created(old) = note(&conn, "The oldest note, still waiting", &["inbox"])?
    else {
        return Err("note".into());
    };
    conn.execute(
        "UPDATE facts SET created_at = created_at - 100000 WHERE id = ?1",
        [&old.id],
    )?;
    for i in 0..250 {
        let Remembered::Created(f) = note(&conn, &format!("sorted note number {i}"), &["inbox"])?
        else {
            return Err("note".into());
        };
        facts::retire(&conn, "x", &f.id, "sorted")?;
    }
    let opts = RecallOptions {
        limit: 50,
        tags: vec!["inbox".into()],
        ..Default::default()
    };
    let listed = facts::recall(&conn, "", &opts, NOW)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].fact.id, old.id);
    Ok(())
}

#[test]
fn a_write_into_an_inbox_marks_it_and_tidy_asks_for_order_now() -> Res {
    use memcp_core::access::{self, Rights};
    use memcp_core::tools;
    let t = Temp::new("inbox-mark");
    let b = Brain::init(&t.path().join("data"), &[sp("health")], Language::English)?;
    let ctl = b.open_control()?;
    let token = access::connect(&ctl, "a", &Rights::parse("health:rw")?, None, false, false)?;
    let c = access::authenticate(&ctl, &token, NOW)?.ok_or("refused")?;
    let mark = b.dir().join(tools::INBOX_WRITTEN);
    tools::call(
        &b,
        &c,
        "remember",
        &serde_json::json!({"text": "A plain fact of the vault."}),
    )?;
    assert!(!mark.exists(), "a write outside the inbox wakes nobody");
    tools::call(
        &b,
        &c,
        "remember",
        &serde_json::json!({"text": "A quick note.", "tags": ["inbox"]}),
    )?;
    assert!(mark.exists() && std::fs::read(&mark)?.is_empty());
    let r = tools::call(&b, &c, "tidy", &serde_json::json!({}))?;
    assert_eq!(r["spaces"]["health"]["inbox"], 1, "{r}");
    assert_eq!(std::fs::read(&mark)?, b"now");
    Ok(())
}
