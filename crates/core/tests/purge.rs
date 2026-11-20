//! A purge removes notes for good, every old version and every trace, and leaves the rest and what is protected.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Caller, Rights};
use memcp_core::sync::purge_facts;
use memcp_core::tools;
use memcp_core::users::OWNER;
use memcp_core::{Brain, util};
use serde_json::{Value, json};

fn caller(b: &Brain) -> Result<Caller, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(
        &ctl,
        OWNER,
        "tester",
        &Rights::parse("all")?,
        None,
        false,
        false,
    )?;
    Ok(access::authenticate(&ctl, &tok, util::now())?.ok_or("refused")?)
}

fn put(
    b: &Brain,
    c: &Caller,
    text: &str,
    tags: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let r = tools::call(
        b,
        c,
        "remember",
        &json!({ "space": "home", "text": text, "tags": tags }),
    )?;
    Ok(r["fact"]["id"].as_str().ok_or("id")?.to_string())
}

fn count(b: &Brain, sql: &str) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(b.open_space(&sp("home"))?
        .query_row(sql, [], |r| r.get(0))?)
}

#[test]
fn a_purge_removes_the_notes_their_versions_and_their_traces_and_spares_the_rest() -> Res {
    let t = Temp::new("purge");
    let b = brain(&t)?;
    let c = caller(&b)?;
    let gone = put(&b, &c, "Brut jetable numéro un, à purger", &["lot/brut"])?;
    // A note with an old version: the correction is the head, the first text is its past.
    let old = put(
        &b,
        &c,
        "Brut jetable numéro deux, première version",
        &["lot/brut"],
    )?;
    let sup: Value = tools::call(
        &b,
        &c,
        "supersede",
        &json!({ "id": old, "text": "Brut jetable numéro deux, corrigé" }),
    )?;
    let head = sup["fact"]["id"].as_str().ok_or("head")?.to_string();
    let kept = put(&b, &c, "À garder, rien à voir", &["lot/gardé"])?;
    let journal = put(
        &b,
        &c,
        "Journal du jour, protégé",
        &["journal/2026-11-19", "lot/brut"],
    )?;

    let ids = vec![
        gone.clone(),
        head.clone(),
        journal.clone(),
        "f_unknown".to_string(),
    ];
    // A count first: nothing changes.
    let dry = purge_facts(&b, &sp("home"), &ids, &[], false)?;
    assert_eq!(
        (dry.facts, dry.skipped_protected, dry.missing),
        (3, 1, 1),
        "{dry:?}"
    );
    assert_eq!(
        count(&b, "SELECT COUNT(*) FROM facts")?,
        4 + 1,
        "the count removed nothing"
    );

    let done = purge_facts(&b, &sp("home"), &ids, &[], true)?;
    assert_eq!(done.facts, 3, "{done:?}");
    // Gone from the facts, the search index, the tags, the history and the log of the other machines.
    assert_eq!(
        count(
            &b,
            "SELECT COUNT(*) FROM facts WHERE text LIKE 'Brut jetable%'"
        )?,
        0
    );
    assert_eq!(
        count(
            &b,
            "SELECT COUNT(*) FROM facts_fts WHERE facts_fts MATCH 'jetable'"
        )?,
        0
    );
    assert_eq!(
        count(
            &b,
            "SELECT COUNT(*) FROM fact_tags WHERE tag = 'lot/brut' AND fact_id NOT IN (SELECT id FROM facts)"
        )?,
        0
    );
    assert_eq!(
        count(
            &b,
            &format!(
                "SELECT COUNT(*) FROM fact_events WHERE fact_id IN ('{gone}','{old}','{head}')"
            )
        )?,
        0
    );
    assert_eq!(
        count(
            &b,
            &format!(
                "SELECT COUNT(*) FROM sync_log WHERE eid LIKE 'c:{gone}%' OR eid LIKE 'c:{old}%' OR eid LIKE 'c:{head}%'"
            )
        )?,
        0
    );
    // Their ids are remembered, so that a machine that still has them does not bring them back.
    assert_eq!(
        count(
            &b,
            &format!(
                "SELECT COUNT(*) FROM window_gone WHERE kind = 'fact' AND id IN ('{gone}','{old}','{head}')"
            )
        )?,
        3
    );
    // The rest stays, the protected note too, and the audit says how many with no content.
    assert!(tools::call(&b, &c, "read_fact", &json!({ "id": kept })).is_ok());
    assert!(tools::call(&b, &c, "read_fact", &json!({ "id": journal })).is_ok());
    assert_eq!(
        count(
            &b,
            "SELECT COUNT(*) FROM audit WHERE tool = 'purge' AND detail LIKE '3 notes purged%'"
        )?,
        1
    );
    // Again: nothing left to do.
    let again = purge_facts(&b, &sp("home"), &ids, &[], true)?;
    assert_eq!(again.facts, 0, "{again:?}");
    Ok(())
}
