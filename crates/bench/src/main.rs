//! `pensine-bench --bin DIR --work DIR [--only swarm,load,security,crash] [--notes N] [--writers N] [--clients N]
//! [--secs N] [--settle N] [--summary FILE]`: real nodes, started from the programs in `--bin`, in `--work` (a
//! directory of the bench's own, on loopback ports), driven from outside like a person and their devices drive them.
//! Prints a summary table; exit 1 when a check does not hold. `scripts/bench.sh` builds the programs and runs it.

mod crash;
mod load;
mod node;
mod security;
mod swarm;

use std::path::PathBuf;
use std::process::ExitCode;

use node::{Env, Row, row};

const USAGE: &str = "usage: pensine-bench --bin DIR --work DIR [--only swarm,load,security,crash] [--notes 3000] [--writers 8] [--clients 32] [--secs 60] [--settle 300] [--summary FILE]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let number = |flag: &str, default: u64| {
        value(flag)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(default)
    };
    let (Some(bin), Some(work)) = (value("--bin"), value("--work")) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let env = Env {
        bin: PathBuf::from(bin),
        work: PathBuf::from(work),
    };
    if std::fs::create_dir_all(&env.work).is_err() {
        eprintln!("pensine-bench: cannot use {}", env.work.display());
        return ExitCode::from(2);
    }
    let only = value("--only").unwrap_or_else(|| "swarm,load,security,crash".into());
    let phases: Vec<&str> = only.split(',').map(str::trim).collect();
    let mut rows: Vec<Row> = vec![];
    let mut run = |name: &'static str, f: &dyn Fn() -> node::R<Vec<Row>>| {
        if !phases.contains(&name) {
            return;
        }
        eprintln!("== {name}");
        let t0 = std::time::Instant::now();
        match f() {
            Ok(r) => rows.extend(r),
            Err(e) => rows.push(row(name, "the phase ran to its end", Some(false), e)),
        }
        eprintln!("   {name}: {:.0} s", t0.elapsed().as_secs_f64());
    };
    run("swarm", &|| {
        swarm::run(
            &env,
            &swarm::Settings {
                notes: number("--notes", 3000) as usize,
                writers: number("--writers", 8).max(1) as usize,
                settle_secs: number("--settle", 300),
            },
        )
    });
    run("load", &|| {
        load::run(
            &env,
            &load::Settings {
                clients: number("--clients", 32).max(1) as usize,
                secs: number("--secs", 60),
                prefill: 2000,
            },
        )
    });
    run("security", &|| security::run(&env));
    run("crash", &|| crash::run(&env));
    rows.push(tokens_in_logs(&env));
    // What the nodes started by themselves and is still running (their sync loops) goes with the bench.
    for pid in env.node_sync_pids() {
        node::kill_pid(pid, &env.work);
    }
    for pid in node::STARTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        node::kill_pid(*pid, &env.work);
    }
    let mut table = Vec::new();
    node::print_table(&rows, &mut table);
    let table = String::from_utf8_lossy(&table).into_owned();
    println!("{table}");
    if let Some(f) = value("--summary") {
        let _ = std::fs::write(f, &table);
    }
    if rows.iter().any(|r| r.ok == Some(false)) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// No token the bench was given (keys, console tokens, peer keys) nor the wrong ones it sent appears in a node's log.
fn tokens_in_logs(env: &Env) -> Row {
    let tokens = node::TOKENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let mut logs = 0;
    let mut bytes = 0;
    let mut found = vec![];
    if let Ok(dir) = std::fs::read_dir(&env.work) {
        for e in dir.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "log") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            logs += 1;
            bytes += text.len();
            for t in &tokens {
                // The secret part of a key (after its public prefix) is what must never be written.
                let secret = t.rsplit('_').next().unwrap_or(t);
                if secret.len() >= 16 && text.contains(secret) {
                    found.push(format!(
                        "{}",
                        p.file_name()
                            .map(|f| f.to_string_lossy())
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    row(
        "security",
        "tokens never appear in node logs",
        Some(found.is_empty() && logs > 0),
        if found.is_empty() {
            format!(
                "{} tokens looked for in {logs} logs ({bytes} bytes)",
                tokens.len()
            )
        } else {
            format!("found in {}", found.join(", "))
        },
    )
}
