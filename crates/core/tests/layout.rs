//! The layout of spaces: labels, a tree, an order, new spaces of a person's own.

mod common;

use common::{Res, Temp, brain, sp};
use memcp_core::layout::{Change, MAX_DEPTH};
use memcp_core::users::OWNER;

fn names(b: &memcp_core::Brain) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(b.layout(OWNER)?
        .iter()
        .map(|p| {
            format!(
                "{}<{}",
                p.space,
                p.parent.as_ref().map_or("", |s| s.as_str())
            )
        })
        .collect())
}

#[test]
fn a_label_and_a_parent_change_only_the_layout() -> Res {
    let t = Temp::new("layout");
    let b = brain(&t)?;
    let before = b.spaces()?;
    b.set_layout(
        OWNER,
        &sp("work"),
        &Change {
            label: Some("  Pro ".into()),
            ..Default::default()
        },
    )?;
    b.set_layout(
        OWNER,
        &sp("health"),
        &Change {
            parent: Some(Some(sp("personal"))),
            ..Default::default()
        },
    )?;
    let l = b.layout(OWNER)?;
    let work = l.iter().find(|p| p.space == sp("work")).ok_or("work")?;
    assert_eq!(work.label, "Pro");
    let health = l.iter().find(|p| p.space == sp("health")).ok_or("health")?;
    assert_eq!(health.parent, Some(sp("personal")));
    // Same spaces, same names: no file and no right moved.
    assert_eq!(b.spaces()?, before);
    assert!(b.open_space(&sp("work")).is_ok());
    Ok(())
}

#[test]
fn children_follow_their_parent_in_order() -> Res {
    let t = Temp::new("layout");
    let b = brain(&t)?;
    b.set_layout(
        OWNER,
        &sp("health"),
        &Change {
            parent: Some(Some(sp("personal"))),
            position: Some(2),
            ..Default::default()
        },
    )?;
    b.set_layout(
        OWNER,
        &sp("home"),
        &Change {
            parent: Some(Some(sp("personal"))),
            position: Some(1),
            ..Default::default()
        },
    )?;
    let n = names(&b)?;
    let i = n.iter().position(|x| x == "personal<").ok_or("personal")?;
    assert_eq!(n[i + 1], "home<personal");
    assert_eq!(n[i + 2], "health<personal");
    Ok(())
}

#[test]
fn no_cycle_no_self_parent_no_stranger() -> Res {
    let t = Temp::new("layout");
    let b = brain(&t)?;
    let under = |s: &str, p: &str| {
        b.set_layout(
            OWNER,
            &sp(s),
            &Change {
                parent: Some(Some(sp(p))),
                ..Default::default()
            },
        )
    };
    assert!(under("work", "work").is_err());
    under("health", "work")?;
    assert!(under("work", "health").is_err(), "a cycle");
    assert!(under("home", "nowhere").is_err(), "unknown parent");
    assert!(
        b.set_layout(
            "somebody-else",
            &sp("work"),
            &Change {
                label: Some("x".into()),
                ..Default::default()
            }
        )
        .is_err(),
        "only the owner lays out a space"
    );
    Ok(())
}

#[test]
fn labels_are_bounded() -> Res {
    let t = Temp::new("layout");
    let b = brain(&t)?;
    for bad in ["", "   ", &"x".repeat(65), "a\nb"] {
        assert!(
            b.set_layout(
                OWNER,
                &sp("work"),
                &Change {
                    label: Some(bad.into()),
                    ..Default::default()
                }
            )
            .is_err(),
            "{bad:?}"
        );
    }
    Ok(())
}

#[test]
fn the_tree_is_at_most_eight_deep() -> Res {
    let t = Temp::new("layout");
    let b = brain(&t)?;
    let mut parent = sp("work");
    for i in 0..(MAX_DEPTH - 1) {
        parent = b.add_custom_space(OWNER, "owner", &format!("level {i}"), Some(&parent))?;
    }
    assert!(
        b.add_custom_space(OWNER, "owner", "one too deep", Some(&parent))
            .is_err()
    );
    Ok(())
}

#[test]
fn a_person_makes_a_space_with_any_label() -> Res {
    let t = Temp::new("layout");
    let b = brain(&t)?;
    let a = b.add_custom_space(OWNER, "owner", "Éléments Dev", Some(&sp("work")))?;
    let c = b.add_custom_space(OWNER, "owner", "Éléments Dev", Some(&sp("work")))?;
    assert_ne!(a, c, "names are made unique");
    assert!(a.as_str().starts_with("owner-elements-dev"));
    let l = b.layout(OWNER)?;
    let made = l.iter().find(|p| p.space == a).ok_or("made")?;
    assert_eq!(made.label, "Éléments Dev");
    assert_eq!(made.parent, Some(sp("work")));
    assert_eq!(made.kind, "custom");
    Ok(())
}
