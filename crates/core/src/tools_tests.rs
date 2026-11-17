//! The leases of Inbox: a note is held by one sorter at a time, a lease ends, and a note that leaves the box while it is
//! held is not given again.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::access::{self, Caller, Rights};
use crate::brain::Brain;
use crate::facts::{self, NewFact, Remembered};
use crate::space::Space;
use crate::text::Language;
use crate::tools::{self, LEASES};
use crate::triage;
use crate::users::OWNER;

struct Box_ {
    dir: std::path::PathBuf,
    brain: Brain,
    space: Space,
    ids: Vec<String>,
}

impl Drop for Box_ {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn filled(tag: &str, notes: usize) -> Box_ {
    let dir = std::env::temp_dir().join(format!("memcp-lease-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let brain = Brain::init(&dir, &[], Language::English).unwrap();
    let space = triage::ensure(&brain, OWNER, "owner").unwrap();
    let conn = brain.open_space(&space).unwrap();
    let words = [
        "plumber", "roof", "garden", "invoice", "holiday", "dentist", "tyres", "boiler",
    ];
    let ids = (0..notes)
        .map(|i| {
            let mut n = NewFact::agent(
                &format!(
                    "Note about the {} number {i} to look at.",
                    words[i % words.len()]
                ),
                "agent:phone",
            );
            n.tags = vec!["inbox".into()];
            match facts::remember(&conn, &space, "phone", n, &[]).unwrap() {
                Remembered::Created(f) => f.id,
                other => panic!("{other:?}"),
            }
        })
        .collect();
    Box_ {
        dir,
        brain,
        space,
        ids,
    }
}

fn sorter(b: &Box_, name: &str) -> Caller {
    let ctl = b.brain.open_control().unwrap();
    let rights = Rights::parse(&format!("{}:rw", b.space.as_str())).unwrap();
    let tok = access::connect_for(&ctl, OWNER, name, &rights, None, false, false).unwrap();
    access::authenticate(&ctl, &tok, crate::util::now())
        .unwrap()
        .unwrap()
}

fn claim(b: &Box_, who: &Caller, args: Value) -> Vec<String> {
    let got = tools::call(&b.brain, who, "triage_claim", &args).unwrap();
    got["claimed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_lease_that_has_ended_gives_the_note_to_the_next_sorter() {
    let b = filled("expiry", 1);
    let (a, c) = (sorter(&b, "lease-a"), sorter(&b, "lease-c"));
    assert_eq!(claim(&b, &a, json!({})), b.ids);
    assert!(claim(&b, &c, json!({})).is_empty(), "held by the first");
    // The lease runs out (what ten minutes do).
    {
        let mut leases = LEASES.lock().unwrap();
        let held = leases.get_mut(&b.ids[0]).unwrap();
        assert_eq!(held.0, "lease-a");
        held.1 = crate::util::now() - 1;
    }
    assert_eq!(
        claim(&b, &c, json!({})),
        b.ids,
        "the second sorter has it now"
    );
    assert!(
        claim(&b, &a, json!({})).is_empty(),
        "and the first no longer does"
    );
    // A new lease is given for the asked time, and the longest asked is kept to an hour.
    let until = LEASES.lock().unwrap().get(&b.ids[0]).unwrap().1;
    let now = crate::util::now();
    assert!(
        (now + 590..=now + 610).contains(&until),
        "ten minutes by default"
    );
    tools::call(&b.brain, &c, "triage_release", &json!({})).unwrap();
    claim(&b, &c, json!({"minutes": 9999}));
    let until = LEASES.lock().unwrap().get(&b.ids[0]).unwrap().1;
    assert!(until <= crate::util::now() + 3600 + 5, "capped at an hour");
}

#[test]
fn two_sorters_never_hold_the_same_note_and_together_cover_the_box() {
    let b = filled("two", 9);
    let (a, c) = (sorter(&b, "two-a"), sorter(&b, "two-c"));
    let first = claim(&b, &a, json!({"limit": 4}));
    let second = claim(&b, &c, json!({"limit": 4}));
    assert_eq!((first.len(), second.len()), (4, 4));
    let (fa, fc): (BTreeSet<_>, BTreeSet<_>) = (first.iter().collect(), second.iter().collect());
    assert!(fa.is_disjoint(&fc));
    // Asking again, the first gets its own notes back (never another's) and the last free one.
    let again: BTreeSet<String> = claim(&b, &a, json!({"limit": 5})).into_iter().collect();
    assert!(first.iter().all(|id| again.contains(id)));
    assert!(again.iter().all(|id| !fc.contains(id)));
    assert_eq!(again.len(), 5);
    // Releasing some ids gives back those only.
    let rel = tools::call(&b.brain, &a, "triage_release", &json!({"ids": [first[0]]})).unwrap();
    assert_eq!(rel["released"], json!(1));
    assert_eq!(
        claim(&b, &c, json!({"limit": 50})).len(),
        4 + 1,
        "its own four and the one released"
    );
}

#[test]
fn sorters_claiming_at_the_same_time_split_the_box_without_overlap() {
    let b = Arc::new(filled("race", 24));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let who = sorter(&b, &format!("race-{i}"));
            let b = Arc::clone(&b);
            std::thread::spawn(move || claim(&b, &who, json!({"limit": 50})))
        })
        .collect();
    let all: Vec<Vec<String>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let total: usize = all.iter().map(Vec::len).sum();
    let union: BTreeSet<&String> = all.iter().flatten().collect();
    assert_eq!(total, 24, "every note was given");
    assert_eq!(union.len(), 24, "none was given twice");
}

#[test]
fn a_note_retired_while_held_is_not_given_again() {
    let b = filled("retired", 3);
    let (a, c) = (sorter(&b, "ret-a"), sorter(&b, "ret-c"));
    let mine = claim(&b, &a, json!({"limit": 3}));
    assert_eq!(mine.len(), 3);
    // Another hand retires one of them (a duplicate, say), while it is held.
    let conn = b.brain.open_space(&b.space).unwrap();
    facts::retire(&conn, "someone", &mine[0], "duplicate").unwrap();
    let after = claim(&b, &a, json!({"limit": 3}));
    assert_eq!(after.len(), 2);
    assert!(!after.contains(&mine[0]));
    // Not for the other sorter either, even once the lease has ended.
    LEASES.lock().unwrap().retain(|id, _| *id != mine[0]);
    assert!(!claim(&b, &c, json!({"limit": 50})).contains(&mine[0]));
    // The retired note's lease does not hold a slot: releasing frees what is left.
    let rel = tools::call(&b.brain, &a, "triage_release", &json!({})).unwrap();
    assert_eq!(rel["released"], json!(2));
}

#[test]
fn a_note_filed_while_held_leaves_the_box_and_cannot_be_claimed() {
    let b = filled("filed", 2);
    let a = sorter(&b, "file-a");
    let c = sorter(&b, "file-c");
    let mine = claim(&b, &a, json!({}));
    assert_eq!(mine.len(), 2);
    // The sorter files one: the note is replaced by a clean fact without the `inbox` tag.
    let conn = b.brain.open_space(&b.space).unwrap();
    let mut clean = NewFact::agent("The plumber comes on Friday.", "agent:sorter");
    clean.tags = vec!["home".into()];
    facts::supersede(&conn, &b.space, "sorter", &mine[0], clean, Some("filed")).unwrap();
    let again = claim(&b, &a, json!({}));
    assert_eq!(
        again,
        [mine[1].clone()],
        "the filed note is not waiting any more"
    );
    // Another sorter takes nothing: the rest is held.
    assert!(claim(&b, &c, json!({})).is_empty());
    // A lease on a note that no longer exists is dropped when it ends, whoever asks.
    {
        let mut leases = LEASES.lock().unwrap();
        for id in &b.ids {
            if let Some(held) = leases.get_mut(id) {
                held.1 = crate::util::now() - 1;
            }
        }
    }
    assert_eq!(claim(&b, &c, json!({})), [mine[1].clone()]);
    assert!(
        !LEASES.lock().unwrap().contains_key(&mine[0]),
        "the stale lease went with its end"
    );
}
