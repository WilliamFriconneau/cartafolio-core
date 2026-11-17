//! Phase 4: SIGKILL in the middle of writes and in the middle of sync passes (only processes this bench started, by
//! PID), restart, and check that nothing acknowledged is lost and that the sync resumes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::node::{
    Answer, Env, Node, R, Row, call, call_retry, kill_pid, parent_of, row, until, words,
};

const PHASE: &str = "crash";

fn same(a: &Node, b: &Node, spaces: &[&str]) -> R<bool> {
    for s in spaces {
        if a.notes_of(s)? != b.notes_of(s)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn run(env: &Env) -> R<Vec<Row>> {
    let mut rows = vec![];
    let spaces = ["personal", "home", "work"];
    let mut a = Node::init(env, "crash-a", true)?;
    a.serve()?;

    // 1. Writers on A, A killed in the middle, restarted: every acknowledged note is there.
    let acked: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(vec![]));
    let stop = Arc::new(AtomicBool::new(false));
    let mut writers = vec![];
    for w in 0..4 {
        let k = a.connect(&format!("crashwriter{w}"), &["--all"])?;
        let (acked, stop) = (acked.clone(), stop.clone());
        writers.push(std::thread::spawn(move || {
            let agent = crate::node::http();
            let mut i = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let seed = 300_000_000 + w * 1_000_000 + i;
                let space = ["personal", "home", "work"][(i % 3) as usize];
                let text = format!("{} {}", words(seed, 5), words(seed ^ 0x99, 2));
                // No retry: a call that got no answer is not acknowledged, and the next note is written.
                if let Answer::Ok(v) = call(
                    &agent,
                    &k.url,
                    Some(&k.key),
                    "remember",
                    &json!({ "space": space, "text": text }),
                ) && v["result"] == json!("created")
                {
                    let id = v["fact"]["id"].as_str().unwrap_or_default().to_string();
                    acked.lock().unwrap_or_else(|e| e.into_inner()).push((
                        space.to_string(),
                        id,
                        text,
                    ));
                } else {
                    std::thread::sleep(Duration::from_millis(50));
                }
                i += 1;
            }
        }));
    }
    let mut kills = 0;
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(1500));
        a.kill9();
        kills += 1;
        std::thread::sleep(Duration::from_millis(300));
        a.serve()?;
    }
    std::thread::sleep(Duration::from_millis(1000));
    stop.store(true, Ordering::Relaxed);
    for w in writers {
        let _ = w.join();
    }
    let acked = acked.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut lost = 0;
    for s in spaces {
        let held = a.notes_of(s)?;
        lost += acked
            .iter()
            .filter(|(sp, id, text)| sp == s && held.get(id) != Some(text))
            .count();
    }
    a.kill9();
    let whole = a.cli_ok(&["doctor"]);
    a.serve()?;
    rows.push(row(
        PHASE,
        "kill -9 during writes: nothing acknowledged lost",
        Some(lost == 0 && whole),
        format!(
            "{kills} kills, {} notes acknowledged, {lost} lost; store {}",
            acked.len(),
            if whole {
                "whole (doctor)"
            } else {
                "DAMAGED (doctor)"
            }
        ),
    ));

    // 2. B joins A and pulls a large memory; the pass is killed, then B itself, then A while it serves.
    let filler = a.connect("crashfill", &["--all"])?;
    let agent = crate::node::http();
    for batch in 0..30u64 {
        let records: Vec<_> = (0..200u64)
            .map(|i| {
                let seed = 400_000_000 + batch * 1000 + i;
                json!({ "text": format!("{} {}", words(seed, 5), words(seed ^ 0x55, 2)),
                        "origin": { "app": "bench", "id": format!("r{batch}-{i}") } })
            })
            .collect();
        let r = call_retry(
            &agent,
            &filler,
            "deposit",
            &json!({ "space": "personal", "records": records }),
            60,
        );
        if !r.is_ok() {
            return Err(format!("deposit: {r:?}"));
        }
    }
    let mut b = Node::init(env, "crash-b", false)?;
    b.serve()?;
    let peer = b.join(&mut a, "crash-b")?;
    let mut cut_at = 0;
    let mut cuts = vec![];
    for round in 0..2 {
        let mut pass = b.sync_spawn(&peer)?;
        let before = b.notes_of("personal")?.len();
        let _ = until(60, "part of the memory to arrive", || {
            b.notes_of("personal").is_ok_and(|m| m.len() > before + 300)
        });
        let _ = pass.kill();
        let _ = pass.wait();
        if round == 1 {
            // The node of B too, while its store was being written by the pass.
            b.kill9();
            b.serve()?;
        }
        cut_at = b.notes_of("personal")?.len();
        cuts.push(cut_at);
    }
    let whole_b = {
        b.kill9();
        let w = b.cli_ok(&["doctor"]);
        b.serve()?;
        w
    };
    let (code, said) = b.sync(&peer)?;
    let total = a.notes_of("personal")?.len();
    let received: usize = said
        .split_whitespace()
        .nth(2)
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX);
    let resumed =
        code == 0 && same(&a, &b, &spaces)? && received <= total.saturating_sub(cut_at) + 600;
    rows.push(row(PHASE, "kill -9 of a pass and of the receiving node: the next pass resumes", Some(resumed && whole_b),
        format!("cut at {cuts:?} of {total}; next pass exit {code}, received {received}; same memory: {}; store of B {}",
            same(&a, &b, &spaces)?, if whole_b { "whole" } else { "DAMAGED" })));

    // 3. A killed while it serves a pass; restarted; the pass after it completes.
    let w = a.connect("crashlate", &["--all"])?;
    for i in 0..300u64 {
        let seed = 500_000_000 + i;
        let _ = call_retry(
            &agent,
            &w,
            "remember",
            &json!({ "space": "home", "text": format!("{} {}", words(seed, 5), words(seed ^ 1, 2)) }),
            30,
        );
    }
    let mut pass = b.sync_spawn(&peer)?;
    std::thread::sleep(Duration::from_millis(150));
    a.kill9();
    // The pass may have finished before the kill, or failed on it: either way it ended.
    let ended = pass.wait().is_ok();
    a.serve()?;
    let (code, said) = b.sync(&peer)?;
    let ok3 = code == 0 && same(&a, &b, &spaces)?;
    rows.push(row(
        PHASE,
        "kill -9 of the serving node during a pass: the next pass completes",
        Some(ok3 && ended),
        format!("exit {code}: {}", said.lines().next().unwrap_or_default()),
    ));

    // 4. The sync loops a node starts by itself (`memcp-sync --every`): none may outlive its node.
    // B keeps its peer in `peers/`: a node started again runs the loop with it. Kill B, look for the loop.
    let before: Vec<u32> = env.node_sync_pids();
    b.kill9();
    b.serve()?;
    until(20, "B to start its sync loop", || {
        env.node_sync_pids().len() > before.len()
    })
    .ok();
    std::thread::sleep(Duration::from_millis(500));
    let b_pid = b.pid().unwrap_or(0);
    b.kill9();
    let t0 = Instant::now();
    let mut orphans: Vec<u32>;
    loop {
        orphans = env
            .node_sync_pids()
            .into_iter()
            .filter(|p| crate::node::ours_alive(*p, &env.work) && parent_of(*p) != Some(b_pid))
            .collect();
        if orphans.is_empty() || t0.elapsed() > Duration::from_secs(20) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let n_orphans = orphans.len();
    for p in &orphans {
        kill_pid(*p, &env.work);
    }
    rows.push(row(
        PHASE,
        "a node's own sync loops end when the node is killed",
        Some(n_orphans == 0),
        if n_orphans == 0 {
            "none left 20 s after".to_string()
        } else {
            format!("{n_orphans} still running 20 s after (killed by the bench)")
        },
    ));
    // And A, restarted, keeps in step with B after all this.
    b.serve()?;
    let (code, _) = b.sync(&peer)?;
    let (code2, said2) = b.sync(&peer)?;
    rows.push(row(
        PHASE,
        "after the crashes, the sync is clean and idle",
        Some(
            code == 0
                && code2 == 0
                && said2.contains("0 received, 0 sent")
                && same(&a, &b, &spaces)?,
        ),
        said2.lines().next().unwrap_or_default().to_string(),
    ));
    a.kill9();
    b.kill9();
    Ok(rows)
}
