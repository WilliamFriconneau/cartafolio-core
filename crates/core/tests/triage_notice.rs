//! The warning a connection gets about the Inbox counts what waits for a decision, as `triage_waiting` does:
//! not every active fact the Inbox space holds.

mod common;

use common::{Res, Temp, brain};
use pensine_core::access::{self, Rights};
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::guide;
use pensine_core::triage;
use pensine_core::users::OWNER;

fn put(b: &pensine_core::Brain, space: &pensine_core::Space, text: &str, tags: &[&str]) -> Res {
    let conn = b.open_space(space)?;
    let mut n = NewFact::agent(text, "agent:phone");
    n.tags = tags.iter().map(|t| t.to_string()).collect();
    assert!(matches!(
        facts::remember(&conn, space, "phone", n, &[])?,
        Remembered::Created(_)
    ));
    Ok(())
}

fn told(
    b: &pensine_core::Brain,
    space: &pensine_core::Space,
) -> Result<String, Box<dyn std::error::Error>> {
    let ctl = b.open_control()?;
    let rights = Rights::parse(&format!("{}:rw", space.as_str()))?;
    let tok = access::connect_for(&ctl, OWNER, "assistant", &rights, None, false, false)?;
    let caller = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    Ok(guide::instructions(b, &caller)?)
}

#[test]
fn facts_of_the_inbox_space_that_are_not_waiting_are_not_announced() -> Res {
    let t = Temp::new("triage-notice-none");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    put(
        &b,
        &space,
        "The janitor's keywords for this space.",
        &["rules"],
    )?;
    put(
        &b,
        &space,
        "What the reader made of a recording.",
        &["dossier/bridge"],
    )?;
    let text = told(&b, &space)?;
    assert!(!text.contains("ATTENTION"), "{text}");
    Ok(())
}

#[test]
fn only_the_notes_waiting_for_a_decision_are_counted() -> Res {
    let t = Temp::new("triage-notice-one");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    put(
        &b,
        &space,
        "The janitor's keywords for this space.",
        &["rules"],
    )?;
    put(
        &b,
        &space,
        "What the reader made of a recording.",
        &["dossier/bridge"],
    )?;
    put(&b, &space, "Call the plumber on Friday.", &["inbox"])?;
    let text = told(&b, &space)?;
    assert!(text.contains("ATTENTION: 1 "), "{text}");
    Ok(())
}
