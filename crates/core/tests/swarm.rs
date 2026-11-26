//! The swarm of one person's nodes, without a network: a space offered by a peer is made with its layout, the person's
//! Inbox is paired rather than copied, an archive wins over an older copy and loses to a newer one, and a note of
//! Inbox filed on two nodes is one note.

mod common;

use common::{Res, Temp};
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::lifecycle::{Children, Disposition};
use pensine_core::swarm::{self, Answer, Offer};
use pensine_core::sync;
use pensine_core::text::Language;
use pensine_core::users::OWNER;
use pensine_core::{Brain, Space, triage, util};

type Boxed = Box<dyn std::error::Error>;

fn node(t: &Temp, name: &str) -> Result<(Brain, Space), Boxed> {
    let b = Brain::init(&t.path().join(name), &[], Language::English)?;
    let box_ = triage::ensure(&b, OWNER, name)?;
    Ok((b, box_))
}

/// Every event of `space` on `from` applied to `space` on `to` (`peer`: how `to` calls `from`).
fn copy(from: &Brain, to: &Brain, space: &Space, peer: &str) -> Result<sync::Applied, Boxed> {
    let page = sync::events_after(from, space, 0, 10_000, 64 << 20, None)?;
    Ok(sync::apply(to, space, &page.events, peer)?)
}

fn active(b: &Brain, space: &Space) -> Result<Vec<String>, Boxed> {
    let conn = b.open_space(space)?;
    let mut st = conn.prepare("SELECT text FROM facts WHERE status = 'active' ORDER BY text")?;
    Ok(st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?)
}

#[test]
fn an_offered_space_is_made_with_its_label_and_parent_for_the_same_person() -> Res {
    let t = Temp::new("swarm-offer");
    let (a, _) = node(&t, "alma")?;
    let (b, _) = node(&t, "bea")?;
    let hobbies = b.add_custom_space(OWNER, "bea", "Hobbies", None)?;
    let pompiers = b.add_custom_space(OWNER, "bea", "Pompiers", Some(&hobbies))?;
    let offers = swarm::offers(&b, OWNER)?;
    // Parents first, the box apart.
    let names: Vec<&str> = offers.iter().map(|o| o.space.as_str()).collect();
    let at = |n: &str| names.iter().position(|x| *x == n);
    assert!(at(hobbies.as_str()) < at(pompiers.as_str()), "{names:?}");
    for o in &offers {
        let back = Offer::from_json(&o.to_json())?;
        assert_eq!(&back, o);
        let answer = swarm::accept(&a, OWNER, o)?;
        if o.kind == pensine_core::catalogue::TRIAGE {
            // A's own box answers for it: the two are paired, no second box is made.
            assert_eq!(
                answer,
                Answer::Triage(triage::of(&a, OWNER)?.ok_or("no box")?)
            );
        } else {
            assert_eq!(answer, Answer::Created(o.space.clone()));
        }
    }
    let place = a
        .layout(OWNER)?
        .into_iter()
        .find(|p| p.space == pompiers)
        .ok_or("not made")?;
    assert_eq!(place.label, "Pompiers");
    assert_eq!(place.parent, Some(hobbies.clone()));
    // Offered again: already there.
    let again = swarm::accept(&a, OWNER, &offers[0])?;
    assert!(
        matches!(again, Answer::Held(_) | Answer::Triage(_)),
        "{again:?}"
    );
    // The copy keeps the time the origin made it.
    assert_eq!(swarm::since(&a, &pompiers)?, swarm::since(&b, &pompiers)?);
    Ok(())
}

#[test]
fn an_archive_wins_over_an_older_copy_and_a_restored_space_comes_back() -> Res {
    let t = Temp::new("swarm-archive");
    let (a, _) = node(&t, "alma")?;
    let (b, _) = node(&t, "bea")?;
    let garden = a.add_custom_space(OWNER, "alma", "Garden", None)?;
    let offer = swarm::offers(&a, OWNER)?
        .into_iter()
        .find(|o| o.space == garden)
        .ok_or("no offer")?;
    swarm::accept(&b, OWNER, &offer)?;
    let at = util::now() + 5;
    a.delete_space(
        OWNER,
        &garden,
        &Disposition::Archive,
        &Children::Grandparent,
        false,
        at,
    )?;
    assert_eq!(swarm::tombstones(&a, OWNER)?, vec![(garden.clone(), at)]);
    // The same offer again, older than the archive: refused, and B archives its copy (kept in its archive).
    assert_eq!(swarm::accept(&a, OWNER, &offer)?, Answer::Archived(at));
    assert!(swarm::archive_here(&b, OWNER, &garden, at)?);
    assert!(!b.has_space(&garden)?);
    assert_eq!(b.archived_spaces(OWNER)?.len(), 1);
    // A copy that began after the archive wins: the space is made again.
    let newer = Offer {
        since: at + 10,
        ..offer
    };
    assert_eq!(
        swarm::accept(&a, OWNER, &newer)?,
        Answer::Created(garden.clone())
    );
    assert!(swarm::tombstones(&a, OWNER)?.is_empty());
    // An archive older than a copy leaves it alone.
    assert!(!swarm::archive_here(&a, OWNER, &garden, at)?);
    assert!(a.has_space(&garden)?);
    Ok(())
}

#[test]
fn a_note_filed_on_two_nodes_before_they_meet_is_one_note() -> Res {
    let t = Temp::new("swarm-filed-once");
    let (a, box_a) = node(&t, "alma")?;
    let (b, _) = node(&t, "bea")?;
    // The same space on both nodes, and the same note in Inbox on both (as a pass leaves them).
    let home = a.add_custom_space(OWNER, "alma", "Home", None)?;
    let offer = swarm::offers(&a, OWNER)?
        .into_iter()
        .find(|o| o.space == home)
        .ok_or("no offer")?;
    swarm::accept(&b, OWNER, &offer)?;
    // B's box takes A's name here, to copy events plainly (the pass pairs boxes of different names).
    let shared = box_a.clone();
    swarm::adopt(&b, OWNER, &shared, pensine_core::catalogue::TRIAGE, 0)?;
    let conn = a.open_space(&shared)?;
    let mut new = NewFact::agent("Water the tomatoes before the heat", "agent:dictation");
    new.tags = vec!["inbox".into()];
    let Remembered::Created(note) = facts::remember(&conn, &shared, "dictation", new, &[])? else {
        return Err("not created".into());
    };
    copy(&a, &b, &shared, "peer-a")?;
    // Both janitors file it, each on its node.
    let one = triage::send(&a, &shared, &note.id, &home, "janitor-a", OWNER)?;
    let two = triage::send(&b, &shared, &note.id, &home, "janitor-b", OWNER)?;
    assert_eq!(one, two, "the same note sent to the same space has one id");
    assert_eq!(one, triage::filed_id(&note.id, &home));
    copy(&a, &b, &home, "peer-a")?;
    copy(&b, &a, &home, "peer-b")?;
    copy(&a, &b, &shared, "peer-a")?;
    copy(&b, &a, &shared, "peer-b")?;
    for n in [&a, &b] {
        assert_eq!(active(n, &home)?, ["Water the tomatoes before the heat"]);
        assert!(active(n, &shared)?.is_empty());
    }
    Ok(())
}

#[test]
fn a_box_offered_by_a_peer_of_another_person_never_takes_a_space_of_this_one() -> Res {
    let t = Temp::new("swarm-other-box");
    let (a, own_box) = node(&t, "alma")?;
    let garage = a.add_custom_space(OWNER, "alma", "Garage", None)?;
    let bob = pensine_core::users::create_user(&a.open_control()?, OWNER, "bob", "Bob", false)?;
    // A peer of Bob, who has no box here, offers « À trier » under the names of the owner's spaces.
    for name in [&garage, &own_box] {
        let offer = Offer::from_json(&serde_json::json!({
            "space": name.as_str(), "kind": pensine_core::catalogue::TRIAGE,
            "label": null, "parent": null, "position": 0, "since": 1,
        }))?;
        assert!(swarm::accept(&a, &bob.id, &offer).is_err(), "{name} taken");
    }
    // The owner's space keeps its kind, and the owner's box stays the one it was.
    assert_eq!(a.space_kind(&garage)?, "custom");
    assert_eq!(triage::of(&a, OWNER)?, Some(own_box));
    assert_eq!(triage::of(&a, &bob.id)?, None);
    Ok(())
}

#[test]
fn two_passes_that_make_the_same_space_at_once_both_succeed() -> Res {
    let t = Temp::new("swarm-adopt-race");
    let (a, _) = node(&t, "alma")?;
    let dir = t.path().join("alma");
    // Two passes of one node (one per peer) that hear of the same new spaces at the same moment, each with its own
    // handle on the memory, as two `memcp-sync` processes have.
    let names: Vec<Space> = (0..30)
        .map(|i| Space::new(&format!("owner-made-{i}")))
        .collect::<Result<_, _>>()?;
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let (dir, names) = (dir.clone(), names.clone());
            std::thread::spawn(move || -> Vec<String> {
                let Ok(b) = Brain::open(&dir) else {
                    return vec!["open".into()];
                };
                names
                    .iter()
                    .filter_map(|s| {
                        swarm::adopt(&b, OWNER, s, "custom", 1)
                            .err()
                            .map(|e| format!("{s}: {e}"))
                    })
                    .collect()
            })
        })
        .collect();
    let mut failed = vec![];
    for w in workers {
        failed.extend(w.join().map_err(|_| "a worker panicked")?);
    }
    assert!(failed.is_empty(), "{failed:?}");
    for s in &names {
        assert!(a.has_space(s)?, "{s}");
    }
    Ok(())
}

#[test]
fn an_archive_stamped_in_the_future_is_bounded_and_a_restored_space_still_comes_back() -> Res {
    let t = Temp::new("swarm-future");
    let (a, _) = node(&t, "alma")?;
    let (b, _) = node(&t, "bea")?;
    let garden = a.add_custom_space(OWNER, "alma", "Garden", None)?;
    let offer = swarm::offers(&a, OWNER)?
        .into_iter()
        .find(|o| o.space == garden)
        .ok_or("no offer")?;
    swarm::accept(&b, OWNER, &offer)?;
    // A peer with a clock years ahead says the space was archived: B archives its copy, and keeps the mark only as far
    // ahead as a clock may drift.
    let now = util::now();
    let far = now + 10 * 365 * 86_400;
    assert!(swarm::archive_here(&b, OWNER, &garden, far)?);
    let (_, kept) = swarm::archived_at(&b, &garden)?.ok_or("no mark")?;
    assert!(kept <= util::now() + 600, "{kept}");
    // Brought back on B, the space stays, whatever the peer says again of that archive.
    let archived = b.archived_spaces(OWNER)?;
    b.restore_space(OWNER, &archived[0].name, false)?;
    assert!(!swarm::archive_here(&b, OWNER, &garden, far)?);
    assert!(b.has_space(&garden)?);
    // And a node that was told of that archive, as marks travel (the time B keeps), takes B's copy again.
    let (c, _) = node(&t, "cyd")?;
    swarm::archive_here(&c, OWNER, &garden, kept)?;
    let back = swarm::offers(&b, OWNER)?
        .into_iter()
        .find(|o| o.space == garden)
        .ok_or("no offer")?;
    assert_eq!(
        swarm::accept(&c, OWNER, &back)?,
        Answer::Created(garden.clone())
    );
    Ok(())
}
