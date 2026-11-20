//! Phase 1: three nodes of one person (A, B, C), joined in a chain (B to A, C to B) and then a triangle (C to A),
//! writers on all three at once, sub-spaces made on B and C during the run, renames and moves on A, `memcp-sync`
//! passes on every link all along. Then the bench waits for the three to hold the same memory and checks it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::node::{Answer, Env, Key, Node, Peering, Place, R, Row, call_retry, fail, row, words};

const PHASE: &str = "swarm";
const NAMED: [&str; 4] = ["personal", "home", "work", "hobbies"];

pub struct Settings {
    pub notes: usize,
    pub writers: usize,
    pub settle_secs: u64,
}

/// A note the node acknowledged: the bench expects it on every node.
#[derive(Clone, Debug)]
struct Acked {
    space: String,
    triage: bool,
    id: String,
    text: String,
}

/// A link of the swarm, kept in step by a loop of passes run by the bench.
struct Link {
    from: usize,
    peer: Peering,
}

pub fn run(env: &Env, s: &Settings) -> R<Vec<Row>> {
    let mut rows = vec![];
    let a = Node::init(env, "alpha", true)?;
    let b = Node::init(env, "beta", false)?;
    let c = Node::init(env, "gamma", false)?;
    let nodes = [a, b, c];
    let nodes: Vec<Arc<Mutex<Node>>> = nodes.into_iter().map(|n| Arc::new(Mutex::new(n))).collect();
    let lock = |i: usize| nodes[i].lock().unwrap_or_else(|e| e.into_inner());
    for i in 0..3 {
        lock(i).serve()?;
    }
    let mut boxes = vec![];
    for i in 0..3 {
        boxes.push(lock(i).triage_box()?);
    }
    // The chain: B joins A, C joins B. A first pass on each, so that every node has the spaces before writing.
    let ab = {
        let mut a = lock(0);
        lock(1).join(&mut a, "beta")?
    };
    let bc = {
        let mut b = lock(1);
        lock(2).join(&mut b, "gamma")?
    };
    for (i, p) in [(1, &ab), (2, &bc), (1, &ab), (2, &bc)] {
        let (code, said) = lock(i).sync(p)?;
        if code != 0 {
            return Err(format!("first pass: {said}"));
        }
    }
    let links: Arc<Mutex<Vec<Link>>> = Arc::new(Mutex::new(vec![
        Link { from: 1, peer: ab },
        Link { from: 2, peer: bc },
    ]));

    // Writers: one connection each, on the nodes in turn.
    let mut keys = vec![];
    for w in 0..s.writers {
        keys.push((
            w % 3,
            lock(w % 3).connect(&format!("writer{w}"), &["--all"])?,
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let passes = Arc::new(AtomicUsize::new(0));
    let failed_passes = Arc::new(Mutex::new(Vec::<String>::new()));
    let written = Arc::new(AtomicUsize::new(0));
    let subspaces: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let acked: Arc<Mutex<Vec<Acked>>> = Arc::new(Mutex::new(vec![]));
    let refused: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));

    // The sync loops: every link, again and again, until the end of the phase.
    let syncers = {
        let (nodes, links, stop, passes, failed) = (
            nodes.clone(),
            links.clone(),
            stop.clone(),
            passes.clone(),
            failed_passes.clone(),
        );
        std::thread::spawn(move || {
            let mut handles: Vec<std::thread::JoinHandle<()>> = vec![];
            let mut running = 0;
            while !stop.load(Ordering::Relaxed) {
                let count = links.lock().unwrap_or_else(|e| e.into_inner()).len();
                while running < count {
                    let i = running;
                    let (nodes, links, stop, passes, failed) = (
                        nodes.clone(),
                        links.clone(),
                        stop.clone(),
                        passes.clone(),
                        failed.clone(),
                    );
                    handles.push(std::thread::spawn(move || {
                        while !stop.load(Ordering::Relaxed) {
                            let (from, peer) = {
                                let l = links.lock().unwrap_or_else(|e| e.into_inner());
                                (l[i].from, l[i].peer.clone())
                            };
                            // The node is only read for its directory and programs: a pass is its own process.
                            let n = nodes[from].lock().unwrap_or_else(|e| e.into_inner());
                            let data = (n.env.clone(), n.data.clone(), n.name.clone());
                            drop(n);
                            let probe = Node::detached(&data.0, &data.2, &data.1);
                            match probe.sync(&peer) {
                                Ok((0, _)) => {}
                                Ok((code, said)) => failed
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push(format!(
                                        "{} -> {}: {code} {}",
                                        data.2,
                                        peer.name,
                                        said.trim()
                                    )),
                                Err(e) => failed.lock().unwrap_or_else(|e| e.into_inner()).push(e),
                            }
                            passes.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }));
                    running += 1;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            for h in handles {
                let _ = h.join();
            }
        })
    };

    let per_writer = s.notes / s.writers;
    let total = per_writer * s.writers;
    let started = Instant::now();
    let mut writers = vec![];
    for (w, (on, key)) in keys.into_iter().enumerate() {
        let (written, subspaces, acked, refused, triage) = (
            written.clone(),
            subspaces.clone(),
            acked.clone(),
            refused.clone(),
            boxes[on].clone(),
        );
        writers.push(std::thread::spawn(move || {
            write_notes(
                w, &key, &triage, per_writer, &written, &subspaces, &acked, &refused,
            );
        }));
    }

    // The person meanwhile: sub-spaces on B and C, the triangle closed, renames and moves on A.
    let at = |fraction: f64| {
        let target = (total as f64 * fraction) as usize;
        while written.load(Ordering::Relaxed) < target
            && started.elapsed() < Duration::from_secs(900)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let mut expected: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    let make = |i: usize, label: &str, parent: &str| -> R<String> {
        let (status, made) = lock(i).admin_post(
            "/spaces/create",
            &json!({ "label": label, "parent": parent }),
        )?;
        if status != 200 {
            return Err(format!("create {label}: {status} {made}"));
        }
        made["space"]["space"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| format!("create {label}: {made}"))
    };
    at(0.2);
    let garage = make(1, "Garage bench", "home")?;
    let atelier = make(2, "Atelier bench", "hobbies")?;
    subspaces
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend([garage.clone(), atelier.clone()]);
    at(0.35);
    // The triangle: C joins A as well.
    let ca = {
        let mut a = lock(0);
        lock(2).join(&mut a, "gamma-a")?
    };
    links
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Link { from: 2, peer: ca });
    at(0.45);
    let projets = make(1, "Projets bench", "work")?;
    // A sub-space of a sub-space, on C, under the one B made: it waits for B's to reach C.
    crate::node::until(120, "the garage of B on C", || {
        lock(2)
            .layout()
            .is_ok_and(|l| l.iter().any(|p| p.space == garage))
    })?;
    let etabli = make(2, "Etabli bench", &garage)?;
    subspaces
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend([projets.clone(), etabli.clone()]);
    at(0.6);
    // On A: renames and moves of spaces made elsewhere, once they are there.
    for sp in [&garage, &atelier, &projets] {
        crate::node::until(120, "a sub-space on A", || {
            lock(0)
                .layout()
                .is_ok_and(|l| l.iter().any(|p| &p.space == sp))
        })?;
    }
    let update = |body: Value| -> R<()> {
        let (status, v) = lock(0).admin_post("/spaces/update", &body)?;
        if status != 200 {
            return Err(format!("update {body}: {status} {v}"));
        }
        Ok(())
    };
    update(json!({ "space": "home", "label": "Maison bench" }))?;
    update(json!({ "space": garage, "label": "Garage renamed on A" }))?;
    update(json!({ "space": atelier, "parent": "personal" }))?;
    update(json!({ "space": projets, "parent": null }))?;
    at(0.8);
    update(json!({ "space": atelier, "label": "Atelier renamed on A" }))?;
    update(json!({ "space": "work", "label": "Travail bench" }))?;
    expected.insert("home".into(), ("Maison bench".into(), None));
    expected.insert("work".into(), ("Travail bench".into(), None));
    expected.insert(
        garage.clone(),
        ("Garage renamed on A".into(), Some("home".into())),
    );
    expected.insert(
        atelier.clone(),
        ("Atelier renamed on A".into(), Some("personal".into())),
    );
    expected.insert(projets.clone(), ("Projets bench".into(), None));
    expected.insert(
        etabli.clone(),
        ("Etabli bench".into(), Some(garage.clone())),
    );

    for w in writers {
        let _ = w.join();
    }
    let wrote_secs = started.elapsed().as_secs_f64();
    let acked = acked.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let refused = refused.lock().unwrap_or_else(|e| e.into_inner()).clone();
    rows.push(row(
        PHASE,
        "notes written (acknowledged) by writers",
        Some(acked.len() == total && refused.is_empty()),
        format!(
            "{} of {total} by {} writers in {wrote_secs:.1} s ({:.0}/s); waited for a sub-space to arrive {}; refused {}{}",
            acked.len(),
            s.writers,
            acked.len() as f64 / wrote_secs,
            WAITED.load(Ordering::Relaxed),
            refused.len(),
            refused
                .first()
                .map(|r| format!(" (first: {r})"))
                .unwrap_or_default()
        ),
    ));

    // Settling: the loops go on until the three nodes hold the same memory and every acknowledged note.
    let settle = Instant::now();
    let mut last = None;
    let mut converged = false;
    while settle.elapsed() < Duration::from_secs(s.settle_secs) {
        let snap: R<Vec<Snapshot>> = (0..3).map(|i| snapshot(&mut lock(i), &boxes[i])).collect();
        if let Ok(snap) = snap {
            let same = snap.windows(2).all(|w| w[0] == w[1]);
            let all_there = missing(&snap[0], &acked).is_empty();
            last = Some(snap);
            if same && all_there {
                converged = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let settle_secs = settle.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    let _ = syncers.join();
    rows.push(row(
        PHASE,
        "time to converge after the last write",
        Some(converged),
        if converged {
            format!("{settle_secs:.1} s")
        } else {
            format!("not within {} s", s.settle_secs)
        },
    ));
    let fails = failed_passes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    rows.push(row(
        PHASE,
        "sync passes (all links, during writes and settling)",
        Some(fails.is_empty()),
        format!(
            "{} passes, {} not clean{}",
            passes.load(Ordering::Relaxed),
            fails.len(),
            fails
                .first()
                .map(|f| format!(" (first: {})", one_line(f)))
                .unwrap_or_default()
        ),
    ));
    // The final checks, on a last snapshot taken with the loops stopped.
    let snap: Vec<Snapshot> = (0..3)
        .map(|i| snapshot(&mut lock(i), &boxes[i]))
        .collect::<R<_>>()
        .or_else(|e| last.ok_or(e))?;
    let names = ["alpha", "beta", "gamma"];
    let same_notes = snap.windows(2).all(|w| w[0].notes == w[1].notes);
    let sizes: Vec<String> = snap
        .iter()
        .zip(names)
        .map(|(s, n)| format!("{n} {}", s.notes.len()))
        .collect();
    rows.push(row(
        PHASE,
        "same notes on A, B, C (id and text)",
        Some(same_notes),
        sizes.join(", "),
    ));
    let lost: Vec<usize> = snap.iter().map(|s| missing(s, &acked).len()).collect();
    rows.push(row(
        PHASE,
        "nothing acknowledged is lost",
        Some(lost.iter().all(|n| *n == 0)),
        format!("missing per node: {lost:?}"),
    ));
    let dups: Vec<usize> = snap.iter().map(duplicates).collect();
    rows.push(row(
        PHASE,
        "no duplicates (a text twice, an id in two spaces)",
        Some(dups.iter().all(|n| *n == 0)),
        format!("per node: {dups:?}"),
    ));
    let same_layout = snap.windows(2).all(|w| w[0].layout == w[1].layout);
    let wrong: Vec<String> = expected
        .iter()
        .filter(|(sp, want)| {
            snap.iter().any(|s| {
                s.layout
                    .iter()
                    .find(|p| &p.space == *sp)
                    .is_none_or(|p| (&p.label, &p.parent) != (&want.0, &want.1))
            })
        })
        .map(|(sp, _)| sp.clone())
        .collect();
    rows.push(row(
        PHASE,
        "same spaces, labels and parents on A, B, C",
        Some(same_layout && wrong.is_empty()),
        format!(
            "{} spaces; renames and moves of A everywhere: {}",
            snap[0].layout.len(),
            if wrong.is_empty() {
                "yes".to_string()
            } else {
                format!("no ({})", wrong.join(", "))
            }
        ),
    ));
    if !same_layout {
        for (s, n) in snap.iter().zip(names) {
            eprintln!("layout of {n}: {:?}", s.layout);
        }
    }
    let boxes_seen: Vec<usize> = snap.iter().map(|s| s.boxes).collect();
    let in_box: Vec<usize> = snap
        .iter()
        .map(|s| s.notes.iter().filter(|(k, _)| k.0 == TRIAGE).count())
        .collect();
    let box_acked = acked.iter().filter(|a| a.triage).count();
    rows.push(row(
        PHASE,
        "À trier is one box on each node, the same notes",
        Some(boxes_seen.iter().all(|n| *n == 1) && in_box.iter().all(|n| *n == box_acked)),
        format!("boxes per node {boxes_seen:?}, notes in the box {in_box:?} (written {box_acked})"),
    ));
    Ok(rows)
}

fn one_line(s: &str) -> String {
    let l: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    l.chars().take(160).collect()
}

#[allow(clippy::too_many_arguments)]
fn write_notes(
    w: usize,
    key: &Key,
    triage: &str,
    n: usize,
    written: &AtomicUsize,
    subspaces: &Mutex<Vec<String>>,
    acked: &Mutex<Vec<Acked>>,
    refused: &Mutex<Vec<String>>,
) {
    let agent = crate::node::http();
    for i in 0..n {
        let seed = (w as u64) * 1_000_000 + i as u64;
        let text = format!("{} {}", words(seed, 5), words(seed ^ 0x5eed, 2));
        let subs = subspaces.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (space, in_box) = match i % 8 {
            0 | 1 => (triage.to_string(), true),
            2 if !subs.is_empty() => (subs[i % subs.len()].clone(), false),
            k => (NAMED[k % NAMED.len()].to_string(), false),
        };
        let mut answer = call_retry(
            &agent,
            key,
            "remember",
            &json!({ "space": space, "text": text }),
            60,
        );
        // A sub-space made on another node may not have reached this one yet (the node answers as for a space it
        // does not give: no write right): the writer waits for it, as a person would, up to two minutes.
        let waited = std::time::Instant::now();
        let mut counted = false;
        while let Answer::Refused(r) = &answer
            && !in_box
            && subs.contains(&space)
            && r.contains("no write right")
            && waited.elapsed() < Duration::from_secs(120)
        {
            if !counted {
                WAITED.fetch_add(1, Ordering::Relaxed);
                counted = true;
            }
            std::thread::sleep(Duration::from_millis(250));
            answer = call_retry(
                &agent,
                key,
                "remember",
                &json!({ "space": space, "text": text }),
                60,
            );
        }
        match answer {
            Answer::Ok(v) if v["result"] == json!("created") => {
                acked.lock().unwrap_or_else(|e| e.into_inner()).push(Acked {
                    space: v["fact"]["space"].as_str().unwrap_or(&space).to_string(),
                    triage: in_box,
                    id: v["fact"]["id"].as_str().unwrap_or_default().to_string(),
                    text: text.clone(),
                });
            }
            other => refused
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("{space}: {}", one_line(&format!("{other:?}")))),
        }
        written.fetch_add(1, Ordering::Relaxed);
    }
}

const TRIAGE: &str = "@inbox";

/// Notes whose sub-space had not reached the writer's node yet when it was written: the writer waited for it.
static WAITED: AtomicUsize = AtomicUsize::new(0);

/// What a node holds: its tree, its notes by (space, id) with « À trier » under one name, and how many boxes.
#[derive(PartialEq, Eq, Debug)]
struct Snapshot {
    layout: Vec<Place>,
    notes: BTreeMap<(String, String), String>,
    boxes: usize,
}

fn snapshot(n: &mut Node, own_box: &str) -> R<Snapshot> {
    let layout: Vec<Place> = n
        .layout()?
        .into_iter()
        .map(|mut p| {
            if p.space == own_box {
                p.space = TRIAGE.into();
            }
            p
        })
        .collect();
    let mut notes = BTreeMap::new();
    let mut boxes = 0;
    for p in &layout {
        if p.kind == "inbox" {
            boxes += 1;
        }
        let name = if p.space == TRIAGE {
            own_box
        } else {
            p.space.as_str()
        };
        for (id, text) in n.notes_of(name).map_err(fail)? {
            notes.insert((p.space.clone(), id), text);
        }
    }
    let mut layout = layout;
    layout.sort();
    Ok(Snapshot {
        layout,
        notes,
        boxes,
    })
}

fn missing(s: &Snapshot, acked: &[Acked]) -> Vec<String> {
    acked
        .iter()
        .filter(|a| {
            let space = if a.triage {
                TRIAGE.to_string()
            } else {
                a.space.clone()
            };
            s.notes.get(&(space, a.id.clone())) != Some(&a.text)
        })
        .map(|a| a.id.clone())
        .collect()
}

fn duplicates(s: &Snapshot) -> usize {
    let mut texts = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut dup = 0;
    for ((_, id), text) in &s.notes {
        if !ids.insert(id.clone()) {
            dup += 1;
        }
        // Notes the node writes itself (boards, rules) may share a text; the bench's own are unique.
        if text.split_whitespace().count() == 7 && !texts.insert(text.clone()) {
            dup += 1;
        }
    }
    dup
}
