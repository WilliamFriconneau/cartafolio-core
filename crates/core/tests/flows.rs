//! The main flows of the core, end to end, as `docs/CORE-API.md` describes them: triage claim, filing
//! rules, and joining a node then syncing with a peer.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::pairing;
use memcp_core::sync::{self, Cursor};
use memcp_core::text::Language;
use memcp_core::tools;
use memcp_core::triage;
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space};
use serde_json::json;

const NOW: i64 = 1_900_000_000;

#[test]
fn triage_claim_then_sort_with_supersede_and_rules() -> Res {
    let t = Temp::new("flow-triage");
    let b = brain(&t)?;
    let boxx = triage::ensure(&b, OWNER, "owner")?;
    let conn = b.open_space(&boxx)?;
    let mut n = NewFact::agent("Call the plumber on Friday.", "agent:phone");
    n.tags = vec!["inbox".into()];
    let Remembered::Created(note) = facts::remember(&conn, &boxx, "phone", n, &[])? else {
        return Err("the note was not written".into());
    };
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:rw", boxx.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "sorter", &rights, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, NOW)?.ok_or("refused")?;
    // 1. Rules: the sorter writes the keywords of the box (a new version of its `rules` note).
    let s = boxx.as_str();
    tools::call(
        &b,
        &c,
        "rules_set",
        &json!({ "space": s, "keywords": ["plumber"] }),
    )?;
    assert_eq!(
        tools::call(&b, &c, "rules_get", &json!({ "space": s }))?["rules"]["keywords"],
        json!(["plumber"])
    );
    // 2. Claim, then sort: a supersede with the clean fact and its real tags.
    let claimed = tools::call(&b, &c, "triage_claim", &json!({ "limit": 5 }))?;
    assert_eq!(claimed["claimed"][0]["id"], json!(note.id));
    let done = tools::call(
        &b,
        &c,
        "supersede",
        &json!({ "id": note.id, "text": "2026-11-16, call the plumber on Friday.", "tags": ["home/plumbing"] }),
    )?;
    assert_eq!(done["result"], "superseded");
    // 3. Nothing is erased: the note is still readable, replaced.
    let old = tools::call(&b, &c, "read_fact", &json!({ "id": note.id }))?;
    assert_eq!(old["fact"]["status"], "replaced");
    Ok(())
}

#[test]
fn join_a_node_with_a_code_then_sync_with_the_peer() -> Res {
    let t = Temp::new("flow-peer");
    let a = brain(&t)?;
    // Join: the person makes a code on the page, the device exchanges it once for a key.
    let ctl = a.open_control()?;
    let code = pairing::create(&ctl, OWNER, &["health".into()], NOW)?;
    let paired = pairing::redeem(&ctl, &code, "Second node", NOW + 1)?;
    assert!(paired.agent.starts_with("peer-second-node-"));
    // Sync: the second node reads the log of the first after its cursor and applies it.
    let b = Brain::init(&t.path().join("b"), &[sp("health")], Language::English)?;
    let space: Space = sp("health");
    let conn = a.open_space(&space)?;
    let Remembered::Created(f) = facts::remember(
        &conn,
        &space,
        "me",
        NewFact::agent("Resting heart rate is 58.", "agent:t"),
        &[],
    )?
    else {
        return Err("not created".into());
    };
    let (pulled, _) = sync::cursors(&b, "node-a", &space)?;
    let page = sync::events_after(&a, &space, pulled, 100, 1 << 20, None)?;
    let applied = sync::apply(&b, &space, &page.events, "node-a")?;
    assert_eq!(applied.applied, 1);
    sync::set_cursor(&b, "node-a", &space, Cursor::Pulled, page.next)?;
    // Idempotent: the same page again changes nothing.
    let again = sync::apply(&b, &space, &page.events, "node-a")?;
    assert_eq!((again.applied, again.skipped), (0, 1));
    assert!(facts::read_fact(&b.open_space(&space)?, &f.id)?.is_some());
    Ok(())
}
