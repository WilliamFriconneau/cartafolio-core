//! The sync policy per space and per node: what this node does with each machine of the person, space by space
//! (`both`, `send`, `receive`, `off`) and for the spaces made later. With nothing set, everything goes both ways,
//! as before; a choice is the person's, on their own spaces and their own machines; a machine that this node does not
//! send a space to never counts as holding it when the window prunes.

mod common;

use common::{Res, Temp, sp};
use memcp_core::access::{self, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::policy::{self, Policy};
use memcp_core::sync::{self, Cursor};
use memcp_core::text::Language;
use memcp_core::users::{self, OWNER};
use memcp_core::{Brain, Space};

type Boxed = Box<dyn std::error::Error>;

const DAY: i64 = 86_400;
const NOW: i64 = 1_900_000_000;
const LAPTOP: &str = "peer-laptop-ab12";

fn node(t: &Temp) -> Result<Brain, Boxed> {
    let brain = Brain::init(
        &t.path().join("node"),
        &[sp("health"), sp("finances"), sp("work")],
        Language::English,
    )?;
    access::connect_peer(
        &brain.open_control()?,
        OWNER,
        LAPTOP,
        &Rights::parse("all")?,
        false,
    )?;
    Ok(brain)
}

fn owner(b: &Brain) -> Result<users::User, Boxed> {
    users::user(&b.open_control()?, OWNER)?.ok_or_else(|| "no owner".into())
}

fn of(b: &Brain, peer: &str, space: &str) -> Result<Policy, Boxed> {
    Ok(policy::of(&b.open_control()?, peer, space)?)
}

#[test]
fn with_nothing_set_every_space_goes_both_ways_now_and_later() -> Res {
    let t = Temp::new("policy-default");
    let b = node(&t)?;
    for s in ["health", "finances", "work", "not-made-yet"] {
        assert_eq!(of(&b, LAPTOP, s)?, Policy::Both, "{s}");
    }
    assert_eq!(policy::later(&b.open_control()?, LAPTOP)?, Policy::Both);
    assert!(Policy::Both.gives() && Policy::Both.takes());
    assert!(Policy::Send.gives() && !Policy::Send.takes());
    assert!(!Policy::Receive.gives() && Policy::Receive.takes());
    assert!(!Policy::Off.gives() && !Policy::Off.takes());
    for w in ["both", "send", "receive", "off"] {
        assert_eq!(Policy::parse(w)?.as_str(), w);
    }
    assert!(Policy::parse("sometimes").is_err());
    Ok(())
}

#[test]
fn a_choice_holds_for_one_space_and_one_machine_and_the_page_view_marks_health_and_money() -> Res {
    let t = Temp::new("policy-set");
    let b = node(&t)?;
    let me = owner(&b)?;
    policy::set(&b, &me, LAPTOP, &sp("health"), Policy::Off)?;
    policy::set(&b, &me, LAPTOP, &sp("work"), Policy::Send)?;
    assert_eq!(of(&b, LAPTOP, "health")?, Policy::Off);
    assert_eq!(of(&b, LAPTOP, "work")?, Policy::Send);
    assert_eq!(of(&b, LAPTOP, "finances")?, Policy::Both);
    // Another machine is not touched.
    assert_eq!(of(&b, "peer-phone-cd34", "health")?, Policy::Both);
    let v = policy::view(&b, &me, LAPTOP)?;
    assert_eq!(v["later"], "both");
    let row = |s: &str| {
        v["spaces"]
            .as_array()
            .and_then(|a| a.iter().find(|r| r["space"] == s).cloned())
            .unwrap_or_default()
    };
    assert_eq!(row("health")["policy"], "off");
    assert_eq!(row("health")["sensitive"], true);
    assert_eq!(row("finances")["sensitive"], true);
    assert_eq!(row("work")["sensitive"], false);
    assert_eq!(row("work")["policy"], "send");
    // Back to both: as if nothing had been set.
    policy::set(&b, &me, LAPTOP, &sp("health"), Policy::Both)?;
    assert_eq!(of(&b, LAPTOP, "health")?, Policy::Both);
    Ok(())
}

#[test]
fn the_choice_for_spaces_made_later_leaves_the_spaces_already_here_as_they_were() -> Res {
    let t = Temp::new("policy-later");
    let b = node(&t)?;
    let me = owner(&b)?;
    policy::set(&b, &me, LAPTOP, &sp("work"), Policy::Receive)?;
    policy::set_later(&b, &me, LAPTOP, Policy::Off)?;
    assert_eq!(policy::later(&b.open_control()?, LAPTOP)?, Policy::Off);
    assert_eq!(of(&b, LAPTOP, "health")?, Policy::Both, "already here");
    assert_eq!(of(&b, LAPTOP, "work")?, Policy::Receive, "its own choice");
    // A space made after the choice follows it.
    b.add_space_for(&sp("travel"), OWNER)?;
    assert_eq!(of(&b, LAPTOP, "travel")?, Policy::Off);
    // So does a space this node does not hold yet (one a peer will offer).
    assert_eq!(of(&b, LAPTOP, "garden")?, Policy::Off);
    Ok(())
}

#[test]
fn only_the_person_sets_a_policy_on_their_own_spaces_and_machines() -> Res {
    let t = Temp::new("policy-who");
    let b = node(&t)?;
    let ctl = b.open_control()?;
    let bob = users::create_user(&ctl, OWNER, "bob", "Bob", false)?;
    b.add_space_for(&sp("bobs"), &bob.id)?;
    drop(ctl);
    // Not Bob's machine, not Bob's space.
    assert!(policy::set(&b, &bob, LAPTOP, &sp("bobs"), Policy::Off).is_err());
    assert!(policy::set(&b, &bob, LAPTOP, &sp("health"), Policy::Off).is_err());
    assert!(policy::set_later(&b, &bob, LAPTOP, Policy::Off).is_err());
    assert!(policy::view(&b, &bob, LAPTOP).is_err());
    // The owner's machine, but Bob's space.
    let me = owner(&b)?;
    assert!(policy::set(&b, &me, LAPTOP, &sp("bobs"), Policy::Off).is_err());
    // A name that is not a machine.
    assert!(policy::set(&b, &me, "helper", &sp("health"), Policy::Off).is_err());
    assert_eq!(of(&b, LAPTOP, "health")?, Policy::Both);
    Ok(())
}

#[test]
fn a_renamed_machine_keeps_its_policy() -> Res {
    let t = Temp::new("policy-rename");
    let b = node(&t)?;
    let me = owner(&b)?;
    policy::set(&b, &me, LAPTOP, &sp("health"), Policy::Off)?;
    policy::set_later(&b, &me, LAPTOP, Policy::Receive)?;
    assert!(access::rename(
        &b.open_control()?,
        LAPTOP,
        "peer-study-ab12"
    )?);
    assert_eq!(of(&b, "peer-study-ab12", "health")?, Policy::Off);
    assert_eq!(
        policy::later(&b.open_control()?, "peer-study-ab12")?,
        Policy::Receive
    );
    assert_eq!(of(&b, LAPTOP, "health")?, Policy::Both);
    Ok(())
}

fn old_note(b: &Brain, space: &Space, text: &str) -> Result<String, Boxed> {
    let conn = b.open_space(space)?;
    let Remembered::Created(f) =
        facts::remember(&conn, space, "me", NewFact::agent(text, "agent:t"), &[])?
    else {
        return Err("not created".into());
    };
    conn.execute(
        "UPDATE facts SET created_at = ?2, valid_from = ?2 WHERE id = ?1",
        rusqlite::params![f.id, NOW - 90 * DAY],
    )?;
    Ok(f.id)
}

#[test]
fn a_machine_this_node_does_not_send_a_space_to_never_counts_as_holding_it() -> Res {
    let t = Temp::new("policy-window");
    let b = node(&t)?;
    let me = owner(&b)?;
    let work = sp("work");
    old_note(&b, &work, "The badge is renewed every year")?;
    // Two machines this replica pushes to; the laptop has everything, the server nothing yet.
    let head = sync::head(&b, &work)?;
    sync::set_cursor(&b, LAPTOP, &work, Cursor::Pushed, head)?;
    sync::set_cursor(&b, "peer-server", &work, Cursor::Pushed, 0)?;
    assert_eq!(sync::prune_window(&b, &work, 30, NOW)?.facts, 0);
    // The server is set off for work: it is not a holder, and not a reason to wait. The laptop holds it.
    policy::set(&b, &me, "peer-server", &work, Policy::Off)?;
    assert_eq!(sync::prune_window(&b, &work, 30, NOW)?.facts, 1);
    Ok(())
}

#[test]
fn a_machine_this_node_only_receives_from_holds_nothing_for_it() -> Res {
    let t = Temp::new("policy-window-alone");
    let b = node(&t)?;
    let me = owner(&b)?;
    let work = sp("work");
    old_note(&b, &work, "The desk moves to the second floor")?;
    let head = sync::head(&b, &work)?;
    sync::set_cursor(&b, LAPTOP, &work, Cursor::Pushed, head)?;
    // Receiving only from the laptop: it is sent nothing, so it holds nothing for this node, and no other machine
    // does: nothing is pruned.
    policy::set(&b, &me, LAPTOP, &work, Policy::Receive)?;
    assert_eq!(sync::prune_window(&b, &work, 30, NOW)?.facts, 0);
    Ok(())
}
