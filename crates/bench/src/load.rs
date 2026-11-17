//! Phase 2: one node, 32 clients at once (each with its own connection) calling `remember`, `recall` and
//! `living_get` in turn for 60 s. Throughput, latency per tool, the node's memory and open files before and after, and
//! whether it still answers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::node::{Answer, Env, Node, R, Row, call, call_retry, open_files, row, rss_kib, words};

const PHASE: &str = "load";
const SPACES: [&str; 3] = ["personal", "home", "work"];
const TOOLS: [&str; 3] = ["remember", "recall", "living_get"];

pub struct Settings {
    pub clients: usize,
    pub secs: u64,
    pub prefill: usize,
}

#[derive(Default)]
struct Tally {
    /// Latency in microseconds of each answered call, per tool, from the first try to the answer.
    lat: [Vec<u64>; 3],
    ok: [usize; 3],
    tool_refused: [usize; 3],
    busy: usize,
    limited: usize,
    other: usize,
    down: usize,
    first_other: Option<String>,
}

fn pct(v: &[u64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i.min(v.len() - 1)] as f64 / 1000.0
}

pub fn run(env: &Env, s: &Settings) -> R<Vec<Row>> {
    let mut rows = vec![];
    let mut n = Node::init(env, "load", true)?;
    n.serve()?;
    // A memory that is not empty: notes written by eight clients before the measure.
    let filler = n.connect("filler", &["--all"])?;
    let mut fill = vec![];
    for t in 0..8 {
        let k = filler.clone();
        let count = s.prefill / 8;
        fill.push(std::thread::spawn(move || {
            let agent = crate::node::http();
            for i in 0..count {
                let seed = 50_000_000 + (t * 1_000_000 + i) as u64;
                let text = format!("{} {}", words(seed, 5), words(seed ^ 0xabc, 2));
                let _ = call_retry(
                    &agent,
                    &k,
                    "remember",
                    &json!({ "space": SPACES[i % 3], "text": text }),
                    60,
                );
            }
        }));
    }
    for f in fill {
        let _ = f.join();
    }
    let pid = n.pid().ok_or("the node is not running")?;
    std::thread::sleep(Duration::from_secs(1));
    let (rss0, fd0) = (rss_kib(pid), open_files(pid));
    let mut keys = vec![];
    for c in 0..s.clients {
        keys.push(n.connect(&format!("client{c}"), &["--all"])?);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let mut handles = vec![];
    for (c, k) in keys.into_iter().enumerate() {
        let stop = stop.clone();
        handles.push(std::thread::spawn(move || {
            let agent = crate::node::http();
            let mut t = Tally::default();
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let which = (i + c) % 3;
                let space = SPACES[i % 3];
                let seed = 90_000_000 + (c * 10_000_000 + i) as u64;
                let args = match which {
                    0 => json!({ "space": space, "text": format!("{} {}", words(seed, 5), words(seed ^ 0x77, 2)) }),
                    1 => json!({ "query": words(seed % 5000 + 50_000_000, 1), "limit": 10 }),
                    _ => json!({ "space": space, "page": "next" }),
                };
                let t0 = Instant::now();
                loop {
                    match call(&agent, &k.url, Some(&k.key), TOOLS[which], &args) {
                        Answer::Ok(_) => {
                            t.ok[which] += 1;
                            t.lat[which].push(t0.elapsed().as_micros() as u64);
                        }
                        Answer::Refused(r) => {
                            // `needs_decision` and the like are answers of the tool, not failures of the node.
                            t.tool_refused[which] += 1;
                            t.lat[which].push(t0.elapsed().as_micros() as u64);
                            if t.first_other.is_none() && which != 0 {
                                t.first_other = Some(r.chars().take(120).collect());
                            }
                        }
                        Answer::Http(503, _) => {
                            t.busy += 1;
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Answer::Http(429, _) => {
                            t.limited += 1;
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                        Answer::Http(code, v) => {
                            t.other += 1;
                            if t.first_other.is_none() {
                                t.first_other = Some(format!("{code} {v}").chars().take(120).collect());
                            }
                        }
                        Answer::Down(e) => {
                            t.down += 1;
                            if t.first_other.is_none() {
                                t.first_other = Some(e.chars().take(120).collect());
                            }
                        }
                    }
                    break;
                }
                i += 1;
            }
            t
        }));
    }
    std::thread::sleep(Duration::from_secs(s.secs));
    stop.store(true, Ordering::Relaxed);
    let mut all = Tally::default();
    for h in handles {
        let Ok(t) = h.join() else { continue };
        for k in 0..3 {
            all.lat[k].extend(&t.lat[k]);
            all.ok[k] += t.ok[k];
            all.tool_refused[k] += t.tool_refused[k];
        }
        all.busy += t.busy;
        all.limited += t.limited;
        all.other += t.other;
        all.down += t.down;
        if all.first_other.is_none() {
            all.first_other = t.first_other;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let answered: usize = (0..3).map(|k| all.ok[k] + all.tool_refused[k]).sum();
    rows.push(row(
        PHASE,
        &format!("{} clients for {} s: calls answered", s.clients, s.secs),
        Some(all.other == 0 && all.down == 0 && answered > 0),
        format!(
            "{answered} ({:.0}/s); turned away busy (503, retried) {}, rate-limited (429) {}, errors {}, no answer {}{}",
            answered as f64 / secs,
            all.busy,
            all.limited,
            all.other,
            all.down,
            all.first_other.as_ref().filter(|_| all.other + all.down > 0).map(|f| format!(" (first: {f})")).unwrap_or_default()
        ),
    ));
    for (k, tool) in TOOLS.iter().enumerate() {
        let v = &mut all.lat[k];
        v.sort_unstable();
        let n_calls = all.ok[k] + all.tool_refused[k];
        rows.push(row(
            PHASE,
            &format!("{tool} latency p50 / p95 / p99"),
            None,
            format!(
                "{:.1} / {:.1} / {:.1} ms; {:.0} calls/s{}",
                pct(v, 0.50),
                pct(v, 0.95),
                pct(v, 0.99),
                n_calls as f64 / secs,
                if all.tool_refused[k] > 0 {
                    format!(
                        " ({} answered by the tool with a refusal)",
                        all.tool_refused[k]
                    )
                } else {
                    String::new()
                }
            ),
        ));
    }
    std::thread::sleep(Duration::from_secs(2));
    let (rss1, fd1) = (rss_kib(pid), open_files(pid));
    let show =
        |v: Option<u64>| v.map_or("?".to_string(), |k| format!("{:.1} MiB", k as f64 / 1024.0));
    let showf = |v: Option<usize>| v.map_or("?".to_string(), |k| k.to_string());
    rows.push(row(
        PHASE,
        "node memory before / after",
        None,
        format!("{} / {}", show(rss0), show(rss1)),
    ));
    let fd_ok = match (fd0, fd1) {
        (Some(a), Some(b)) => Some(b <= a + 16),
        _ => None,
    };
    rows.push(row(
        PHASE,
        "node open files before / after (2 s after the load)",
        fd_ok,
        format!("{} / {}", showf(fd0), showf(fd1)),
    ));
    let agent = crate::node::http();
    let probe = n.connect("after-load", &["--all"])?;
    let t0 = Instant::now();
    let r1 = call(
        &agent,
        &probe.url,
        Some(&probe.key),
        "remember",
        &json!({ "space": "work", "text": format!("{} {}", words(7_777_777, 5), words(7_777_778, 2)) }),
    );
    let r2 = call(
        &agent,
        &probe.url,
        Some(&probe.key),
        "recall",
        &json!({ "query": "ka" }),
    );
    rows.push(row(
        PHASE,
        "the node still answers after the load",
        Some(r1.is_ok() && r2.is_ok() && n.alive()),
        format!(
            "remember and recall in {:.0} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        ),
    ));
    n.kill9();
    Ok(rows)
}
