//! The background task that keeps the folders of the spaces in step with the memory (see docs/FOLDERS.md).
//!
//! Every few seconds (`MEMCP_FOLDER_EVERY`, default 10) the node makes one pass over each space that has a folder on
//! this node. A node with no folder set does one cheap query per pass and nothing else. A pass can also be asked for by
//! hand: `memcp folder sync`, or the page. Only one pass runs at a time on a node.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pensine_core::Brain;
use pensine_core::folders::{self, Options};
use pensine_core::util;

/// Seconds between two passes.
pub const EVERY: u64 = 10;

/// The task, until [`FolderTask::stop`] (or the end of the process).
pub struct FolderTask {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FolderTask {
    /// Asks the task to end and waits for the pass in progress.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The seconds between two passes: `MEMCP_FOLDER_EVERY` if it is a number of at least one, else [`EVERY`].
pub fn every() -> u64 {
    pensine_core::env::var("MEMCP_FOLDER_EVERY")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(EVERY)
}

/// Starts the task on a node.
pub fn start(brain: &Brain) -> Option<FolderTask> {
    let brain = brain.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("memcp-folders".into())
        .spawn(move || run(&brain, &flag))
        .ok()?;
    Some(FolderTask {
        stop,
        handle: Some(handle),
    })
}

fn run(brain: &Brain, stop: &AtomicBool) {
    let mut said: BTreeSet<String> = BTreeSet::new();
    let mut next = Instant::now() + Duration::from_secs(2);
    while !stop.load(Ordering::SeqCst) {
        if Instant::now() >= next {
            let mut now_said = BTreeSet::new();
            for (space, result) in folders::sync_all(brain, util::now(), Options::default()) {
                match result {
                    Ok(r) => {
                        if r.written
                            + r.edited
                            + r.new_notes
                            + r.new_files
                            + r.conflicts
                            + r.retired
                            > 0
                        {
                            eprintln!(
                                "folders: {space}: {} written, {} edited, {} new notes, {} new files, {} conflicts, {} retired",
                                r.written,
                                r.edited,
                                r.new_notes,
                                r.new_files,
                                r.conflicts,
                                r.retired
                            );
                        }
                        for e in r.errors {
                            now_said.insert(format!("{space}: {e}"));
                        }
                    }
                    Err(e) => {
                        now_said.insert(format!("{space}: {e}"));
                    }
                }
            }
            // A problem is said once, until it is gone.
            for line in now_said.difference(&said) {
                eprintln!("folders: {line}");
            }
            said = now_said;
            next = Instant::now() + Duration::from_secs(every());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
