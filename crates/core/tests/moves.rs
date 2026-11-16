//! Moving notes from a space to its sub-space by tag: previewed, then done, and never an erasure.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::layout::Change;
use memcp_core::users::OWNER;

fn put(b: &memcp_core::Brain, space: &str, text: &str, tag: &str) -> Res {
    let conn = b.open_space(&sp(space))?;
    let mut n = NewFact::agent(text, "agent:test");
    n.tags = vec![tag.into()];
    match facts::remember(&conn, &sp(space), "tester", n, &[])? {
        Remembered::Created(_) => Ok(()),
        other => Err(format!("not created: {other:?}").into()),
    }
}

#[test]
fn notes_of_a_tag_go_to_the_sub_space_and_the_source_says_where() -> Res {
    let t = Temp::new("moves");
    let b = brain(&t)?;
    // `home` is the parent; `personal` plays the sub-space of cars here.
    b.set_layout(
        OWNER,
        &sp("personal"),
        &Change {
            parent: Some(Some(sp("home"))),
            ..Default::default()
        },
    )?;
    put(
        &b,
        "home",
        "The ID4 charges at 11 kW on the wallbox.",
        "home/voitures/id4",
    )?;
    put(
        &b,
        "home",
        "Winter tyres go on in November.",
        "home/voitures",
    )?;
    put(
        &b,
        "home",
        "The boiler is serviced in March.",
        "home/chauffage",
    )?;
    // A preview changes nothing.
    let seen = b.move_by_tag(
        &sp("home"),
        &sp("personal"),
        "home/voitures",
        "tester",
        true,
    )?;
    assert_eq!((seen.matched, seen.moved), (2, 0));
    assert_eq!(seen.sample.len(), 2);
    let dst = b.open_space(&sp("personal"))?;
    assert_eq!(facts::counts(&dst)?["active"], serde_json::Value::Null);
    // The move writes them again in the target and retires them in the source, with the reason.
    let done = b.move_by_tag(
        &sp("home"),
        &sp("personal"),
        "home/voitures",
        "tester",
        false,
    )?;
    assert_eq!((done.matched, done.moved, done.failed), (2, 2, 0));
    assert_eq!(facts::counts(&dst)?["active"], 2);
    let src = b.open_space(&sp("home"))?;
    assert_eq!(facts::counts(&src)?["active"], 1, "the boiler stays");
    let reason: String = src.query_row(
        "SELECT status_reason FROM facts WHERE status = 'retired' LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(reason, "moved to personal");
    // The tags travel with the notes, and doing it again moves nothing.
    let tags: Vec<String> = dst
        .prepare("SELECT DISTINCT tag FROM fact_tags ORDER BY tag")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert!(tags.contains(&"home/voitures/id4".to_string()), "{tags:?}");
    let again = b.move_by_tag(
        &sp("home"),
        &sp("personal"),
        "home/voitures",
        "tester",
        false,
    )?;
    assert_eq!((again.matched, again.moved), (0, 0));
    Ok(())
}

#[test]
fn a_space_s_own_settings_never_move_with_its_notes() -> Res {
    let t = Temp::new("moves");
    let b = brain(&t)?;
    put(&b, "home", "{\"version\":1,\"sections\":[]}", "dashboard")?;
    let done = b.move_by_tag(&sp("home"), &sp("personal"), "dashboard", "tester", false)?;
    assert_eq!(done.matched, 0);
    Ok(())
}
