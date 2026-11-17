//! `memcp-janitor [--config FILE] [run]`: one pass over the inboxes (notes filed, absorbed pieces turned into facts), then it
//! stops. `--watch FILE`: it stays, and passes once the writing has calmed down (`--quiet SECONDS` after the last write the
//! memory marks in FILE) and once a night (`--night HH:MM`): never for nothing. Run it on a timer. Exit: 0 done, 1 the key is refused, 2 wrong use or settings, 3 done with problems (said on standard error).

use std::path::PathBuf;
use std::process::ExitCode;

use memcp_janitor::{Config, Janitor};

const USAGE: &str = "usage: memcp-janitor [--config FILE] [run]

One pass: every note of À trier is sent to the inbox of a space (or left there when it is unsure), every note of an inbox is
filed in the space of that inbox, its
text unchanged; a piece of a note the bridge absorbed is turned into short, dated facts. If no model answers, the notes stay.
With --watch FILE (the memory's mark of the last write into an inbox), it stays: a pass once the writing has calmed down
(--quiet SECONDS, 120 by default) and one a night (--night HH:MM, 03:00 by default). How the Home Assistant app runs it. The settings are in FILE (default ~/.config/memcp-janitor/janitor.conf); see docs/JANITOR.md.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file = None;
    let mut watch: Option<PathBuf> = None;
    let mut quiet: u64 = 120;
    let mut night: (u32, u32) = (3, 0);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--config" => match it.next() {
                Some(f) => file = Some(PathBuf::from(f)),
                None => {
                    eprintln!("memcp-janitor: --config needs a file");
                    return ExitCode::from(2);
                }
            },
            "run" => {}
            "--watch" => match it.next() {
                Some(f) => watch = Some(PathBuf::from(f)),
                None => {
                    eprintln!("memcp-janitor: --watch needs a file");
                    return ExitCode::from(2);
                }
            },
            "--quiet" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) if (10..=86_400).contains(&n) => quiet = n,
                _ => {
                    eprintln!("memcp-janitor: --quiet needs seconds, 10 to 86400");
                    return ExitCode::from(2);
                }
            },
            "--night" => match it.next().and_then(|v| {
                let (h, m) = v.split_once(':')?;
                Some((h.parse().ok()?, m.parse().ok()?))
            }) {
                Some((h, m)) if h < 24 && m < 60 => night = (h, m),
                _ => {
                    eprintln!("memcp-janitor: --night needs HH:MM");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("memcp-janitor: unknown argument {other:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let file = file.unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".config/memcp-janitor/janitor.conf")
    });
    let mut janitor = match Config::load(&file).and_then(Janitor::new) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("memcp-janitor: {e}");
            return ExitCode::from(2);
        }
    };
    let Some(mark) = watch else {
        let r = janitor.run_once();
        println!(
            "sent on from À trier {}, left there for you {}; filed {}, left in the inbox {}",
            r.sent, r.unsure, r.filed, r.left
        );
        for p in &r.problems {
            eprintln!("memcp-janitor: {p}");
        }
        if r.key_refused {
            eprintln!("memcp-janitor: the key is refused or revoked");
            return ExitCode::from(1);
        }
        // Something went wrong (the memory unreachable, a note that could not be written): the unit shows it.
        return if r.problems.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(3)
        };
    };
    let written_at = |p: &PathBuf| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    };
    // One pass at the start: what came while it was away.
    let mut done_for = written_at(&mark).unwrap_or(0);
    let mut night_done: Option<u64> = None;
    let pass = |janitor: &mut Janitor, why: &str| -> bool {
        let r = if why == "night" {
            janitor.run_night()
        } else {
            janitor.run_once()
        };
        if r.filed > 0 || r.left > 0 || r.sent > 0 || r.unsure > 0 || !r.problems.is_empty() {
            println!(
                "{why}: sent on from À trier {}, left there for you {}; filed {} ({} without the model), left in the inbox {}, keywords added {}",
                r.sent, r.unsure, r.filed, r.without_model, r.left, r.keywords_added
            );
        }
        for p in &r.problems {
            eprintln!("memcp-janitor: {p}");
        }
        if r.key_refused {
            eprintln!("memcp-janitor: the key is refused or revoked");
        }
        r.key_refused
    };
    if pass(&mut janitor, "at start") {
        return ExitCode::from(1);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(15));
        let t = now();
        // After a session of writing: the last write is `quiet` seconds old, and newer than the last pass's.
        // « Put it in order now » (the `tidy` tool) does not wait for the calm.
        let asked_now = std::fs::read(&mark).is_ok_and(|b| b.starts_with(b"now"));
        if let Some(w) = written_at(&mark)
            && w > done_for
            && (asked_now || t.saturating_sub(w) >= quiet)
        {
            done_for = w;
            if pass(&mut janitor, "after writing") {
                return ExitCode::from(1);
            }
        }
        // Once a night, at the hour asked (local time), in case something was left.
        let local = i64::try_from(t).unwrap_or(0) + i64::from(janitor.utc_offset_minutes()) * 60;
        let day = u64::try_from(local.div_euclid(86_400)).unwrap_or(0);
        let minute = local.rem_euclid(86_400) / 60;
        let at = i64::from(night.0 * 60 + night.1);
        if minute >= at && minute < at + 30 && night_done != Some(day) {
            night_done = Some(day);
            if pass(&mut janitor, "night") {
                return ExitCode::from(1);
            }
            for p in janitor.write_learning_notes() {
                eprintln!("memcp-janitor: {p}");
            }
        }
    }
}
