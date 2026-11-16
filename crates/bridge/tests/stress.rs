//! Under load: captures written from several threads while flushes run, two handles on one outbox (the app and the
//! command line), and many Markdown notes of the same name at once. Fictional data only.
#![allow(clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::thread;

use common::{memory, unique};
use memcp_bridge::Intake;
use memcp_bridge::markdown::Markdown;

const WRITERS: usize = 4;
const EACH: usize = 30;

#[test]
fn captures_from_many_threads_while_flushing_all_arrive_once() {
    let mem = memory("stress", &["sam-me/inbox:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("stress-outbox");
    // Two handles on one outbox, as the app and the command line.
    let a = Arc::new(Intake::open(mem.config(0, None, &outbox)).expect("open a"));
    let b = Arc::new(Intake::open(mem.config(0, None, &outbox)).expect("open b"));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flusher = {
        let (b, stop) = (Arc::clone(&b), Arc::clone(&stop));
        thread::spawn(move || {
            let mut sent = 0u32;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                sent += b.flush().sent;
            }
            sent
        })
    };
    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let a = Arc::clone(&a);
            thread::spawn(move || {
                (0..EACH)
                    .map(|i| {
                        a.submit_text(&format!("Note fictive charge{w}x{i} pour le test"))
                            .expect("submit")
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let ids: Vec<String> = writers
        .into_iter()
        .flat_map(|t| t.join().expect("writer"))
        .collect();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut sent = flusher.join().expect("flusher");
    for _ in 0..20 {
        let r = a.flush();
        assert_eq!(r.failed, 0, "{r:?}");
        sent += r.sent;
        if a.waiting() == 0 {
            break;
        }
    }
    let total = u32::try_from(WRITERS * EACH).expect("total");
    let unique_ids: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique_ids.len(), ids.len(), "every capture has its own id");
    assert_eq!(a.waiting(), 0, "nothing left waiting");
    assert_eq!(sent, total, "each capture sent once, none twice");
    // A sample in the memory: each found once.
    for (w, i) in [(0, 0), (1, EACH / 2), (WRITERS - 1, EACH - 1)] {
        let word = format!("charge{w}x{i}");
        let found = mem.texts("inbox", &[&word]);
        assert_eq!(
            found.iter().filter(|t| t.contains(&word)).count(),
            1,
            "{word}: {found:?}"
        );
    }
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn many_markdown_notes_of_one_name_at_once_never_overwrite_each_other() {
    let folder = unique("stress-md");
    std::fs::create_dir_all(&folder).expect("folder");
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let folder = folder.clone();
            thread::spawn(move || {
                let md = Markdown {
                    folder,
                    utc_offset_minutes: 60,
                };
                (0..40)
                    .map(|i| {
                        md.note(
                            &format!("Même titre fictif\nfil {t} note {i}"),
                            1_793_881_800,
                        )
                        .expect("note")
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let paths: Vec<_> = threads
        .into_iter()
        .flat_map(|t| t.join().expect("thread"))
        .collect();
    let distinct: std::collections::HashSet<_> = paths.iter().collect();
    assert_eq!(distinct.len(), paths.len(), "two notes got the same file");
    let files = std::fs::read_dir(&folder).expect("read").count();
    assert_eq!(files, 320, "a note was written over another");
    let _ = std::fs::remove_dir_all(folder);
}
