//! Moving notes from one space to another, by tag: what a person does when they make a sub-space (the cars of the
//! house) and want what already belongs to it to go there.
//!
//! A move is never an erasure: the note is written again in the target space, with its text, its tags, its dates and
//! its author, and **retired** in the source with the reason "moved to …", so the history of the source says where it
//! went. Files and imported passages are left where they are and counted, not moved.
//!
//! # Bounded, resumable, crash-safe
//!
//! A call moves at most [`Limits::limit`] notes and stops when its [`Limits::budget`] is spent; the answer says how
//! many remain, and the caller calls again until none do. Notes go by chunks of [`CHUNK`], each in three short
//! transactions, so no writer waits long for the lock:
//!
//! 1. the source space records the intent (table `move_pending`: the source note, the target space, who moves it);
//! 2. the target space writes the copies, each under an id derived from the note and the target
//!    ([`crate::triage::filed_id`]): written twice, it is one note. No closeness check: the note exists already, it
//!    only changes place;
//! 3. the source space retires the notes and clears their intents, in one transaction.
//!
//! The source and the target are two SQLite files: no transaction spans both. The journal closes the gap. A crash
//! (or `kill -9`) after step 1 or 2 leaves intents behind; [`Brain::finish_moves`] completes them (the copy is written
//! if it is missing, then the source is retired). It runs when the node starts and before every move. Every note
//! therefore ends active exactly once, in the source or in the target: never in both, never in neither. The copy and
//! its entry in the peers' log are written in one transaction, the retirement and its entry in another, so the log
//! tells each of them once.

use std::cell::Cell;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, params};

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::facts::{self, Fact, NewFact, Remembered, Source};
use crate::space::Space;

/// Notes moved by one call when the caller does not say.
pub const DEFAULT_LIMIT: usize = 1000;
/// The most notes one call may move.
pub const MAX_LIMIT: usize = 5000;
/// The time one call may take before it stops and says what remains (a client gives up on a call after about 30 s).
pub const DEFAULT_BUDGET: Duration = Duration::from_secs(20);
/// Notes per chunk, three transactions per chunk: large enough that commits do not dominate, small enough that the
/// write lock is held for a few tens of milliseconds at a time.
pub const CHUNK: usize = 200;
/// Failures listed with their reason in an answer (the count covers them all).
const FAILURES_SHOWN: usize = 10;

/// How much one call may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most notes this call moves (or tries to).
    pub limit: usize,
    /// The call starts no new chunk once this much time has passed.
    pub budget: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            limit: DEFAULT_LIMIT,
            budget: DEFAULT_BUDGET,
        }
    }
}

/// What a move did, or would do.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Moved {
    /// Notes that match the tag (those to move and those left in place).
    pub matched: usize,
    /// Notes this call copied to the target and retired in the source (zero when only previewed).
    pub moved: usize,
    /// Notes another call, or a call cut off before, had already moved: the copy was there, or the source was no
    /// longer active. The source is retired if it was still active. Never an error.
    pub already: usize,
    /// Notes of a move cut off before (a crash, a killed process) that this call finished first.
    pub resumed: usize,
    /// Notes of a file or an imported passage, left in place.
    pub left: usize,
    /// Notes the target refused; they stay active in the source.
    pub failed: usize,
    /// The first few refusals: the note's id and the reason.
    pub failures: Vec<(String, String)>,
    /// Notes still to move after this call: call again until it is 0.
    pub remaining: usize,
    /// The first few, for a preview.
    pub sample: Vec<String>,
}

/// A step of a move after which a test makes it fail, as a crash would.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The intents are recorded, no copy is written.
    AfterJournal,
    /// The copies are written, no source is retired.
    AfterCopies,
}

thread_local! {
    static FAIL_AT: Cell<Option<Step>> = const { Cell::new(None) };
}

/// For the tests: the moves of this thread fail after `step` (`None` to stop).
#[doc(hidden)]
pub fn fail_at(step: Option<Step>) {
    FAIL_AT.with(|f| f.set(step));
}

fn failpoint(step: Step) -> Result<()> {
    if FAIL_AT.with(Cell::get) == Some(step) {
        return Err(Error::Invalid(format!("move stopped at {step:?} (test)")));
    }
    Ok(())
}

fn extract(text: &str, n: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

/// The journal of a space's moves under way. Made the first time the space moves notes (an older program that never
/// moves ignores it, so it needs no schema version).
const JOURNAL: &str = "CREATE TABLE IF NOT EXISTS move_pending (
  source       TEXT PRIMARY KEY,
  target_space TEXT NOT NULL,
  who          TEXT NOT NULL,
  at           INTEGER NOT NULL
) STRICT, WITHOUT ROWID";

fn has_journal(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'move_pending'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn begin(conn: &Connection) -> Result<rusqlite::Transaction<'_>> {
    Ok(rusqlite::Transaction::new_unchecked(
        conn,
        rusqlite::TransactionBehavior::Immediate,
    )?)
}

/// What one chunk did.
#[derive(Default)]
struct Done {
    moved: usize,
    already: usize,
    failures: Vec<(String, String)>,
}

/// The copy of `f` for the target: its text, kind, tags, dates, author and source.
fn copy_of(f: &Fact) -> NewFact {
    let agent = f.source_agent.clone().unwrap_or_else(|| f.author.clone());
    let mut new = NewFact::agent(&f.text, &agent);
    new.kind = f.kind.clone();
    new.valid_from = Some(f.valid_from);
    new.valid_to = f.valid_to;
    new.tags = f.tags.clone();
    new.source = Source::Agent(agent);
    new
}

/// Steps 2 and 3 for `notes` (active sources whose intent is recorded): the copies, then the retirements with the
/// intents cleared. A note the target refuses stays in the source and its intent is cleared: it is not moved.
fn complete(
    src: &Connection,
    dst: &Connection,
    to: &Space,
    who: &str,
    notes: &[Fact],
) -> Result<Done> {
    let mut done = Done::default();
    // Per note: Some(true) when this call wrote the copy (or the target held the same text), Some(false) when the
    // copy was there already, None when the target refused it.
    let mut copied: Vec<Option<bool>> = Vec::with_capacity(notes.len());
    let tx = begin(dst)?;
    for f in notes {
        let id = crate::triage::filed_id(&f.id, to);
        tx.execute_batch("SAVEPOINT copy")?;
        match facts::remember_as_in(&tx, to, &f.author, copy_of(f), &id) {
            Ok(Remembered::Created(_)) => copied.push(Some(true)),
            Ok(Remembered::Duplicate(d)) => copied.push(Some(d.id != id)),
            Ok(Remembered::NeedsDecision(_)) => {
                tx.execute_batch("ROLLBACK TO copy")?;
                copied.push(None);
                done.failures
                    .push((f.id.clone(), "the target asked for a decision".into()));
            }
            // The database itself failed: the chunk stops, its intents stay, and the next pass finishes it.
            Err(Error::Sqlite(e)) => return Err(Error::Sqlite(e)),
            Err(e) => {
                tx.execute_batch("ROLLBACK TO copy")?;
                copied.push(None);
                done.failures.push((f.id.clone(), e.to_string()));
            }
        }
        tx.execute_batch("RELEASE copy")?;
    }
    tx.commit()?;
    failpoint(Step::AfterCopies)?;
    let reason = format!("moved to {to}");
    let tx = begin(src)?;
    for (f, c) in notes.iter().zip(&copied) {
        if let Some(fresh) = c {
            let retired = facts::retire_if_active(&tx, who, &f.id, &reason)?;
            if retired && *fresh {
                done.moved += 1;
            } else {
                done.already += 1;
            }
        }
        tx.execute("DELETE FROM move_pending WHERE source = ?1", [&f.id])?;
    }
    crate::sync::settle_disputes(&tx)?;
    tx.commit()?;
    Ok(done)
}

impl Moved {
    /// The answer of a move, as the tool `notes_move` and the page give it.
    pub fn to_json(&self, from: &Space, to: &Space, tag: &str, dry_run: bool) -> serde_json::Value {
        let failures: Vec<serde_json::Value> = self
            .failures
            .iter()
            .map(|(id, reason)| serde_json::json!({ "id": id, "reason": reason }))
            .collect();
        serde_json::json!({
            "from": from.as_str(), "to": to.as_str(), "tag": tag, "dry_run": dry_run,
            "matched": self.matched, "moved": self.moved, "already": self.already,
            "resumed": self.resumed, "left": self.left, "failed": self.failed,
            "failures": failures, "remaining": self.remaining, "sample": self.sample,
        })
    }
}

impl Brain {
    /// The notes of `from` filed under `tag` (or below it) go to `to`, within the default [`Limits`]. With `dry_run`
    /// nothing changes and the report says what would.
    pub fn move_by_tag(
        &self,
        from: &Space,
        to: &Space,
        tag: &str,
        who: &str,
        dry_run: bool,
    ) -> Result<Moved> {
        self.move_by_tag_within(from, to, tag, who, dry_run, &Limits::default())
    }

    /// [`Brain::move_by_tag`] within `limits`: the answer's `remaining` says how many notes are still to move; call
    /// again until it is 0. Moves cut off before are finished first (see the module documentation).
    pub fn move_by_tag_within(
        &self,
        from: &Space,
        to: &Space,
        tag: &str,
        who: &str,
        dry_run: bool,
        limits: &Limits,
    ) -> Result<Moved> {
        let start = Instant::now();
        let tag = facts::normalize_tag(tag)?;
        if from == to {
            return Err(Error::Invalid(
                "a note cannot move to the space it is in".into(),
            ));
        }
        let mut out = Moved::default();
        if !dry_run {
            out.resumed = self.finish_moves()?;
        }
        let src = self.open_space(from)?;
        let like = format!("{tag}/%");
        let rows: Vec<(String, bool)> = {
            let mut st = src.prepare(
                "SELECT f.id, (COALESCE(f.type, '') = 'file' OR f.source_kind != 'agent') FROM facts f
                 WHERE f.status = 'active' AND f.id IN
                   (SELECT fact_id FROM fact_tags WHERE tag = ?1 OR tag LIKE ?2)
                 AND f.id NOT IN
                   (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules','index','guide'))
                 ORDER BY f.created_at, f.rowid",
            )?;
            st.query_map([&tag, &like], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        out.matched = rows.len();
        let mut ids = Vec::with_capacity(rows.len());
        for (id, stays) in rows {
            if stays {
                out.left += 1;
            } else {
                ids.push(id);
            }
        }
        for id in ids.iter().take(5) {
            if let Some(f) = facts::read_fact(&src, id)? {
                out.sample.push(extract(&f.text, 120));
            }
        }
        if dry_run {
            out.remaining = ids.len();
            return Ok(out);
        }
        let dst = self.open_space(to)?;
        src.execute_batch(JOURNAL)?;
        let cap = ids.len().min(limits.limit);
        let mut next = 0;
        while next < cap && start.elapsed() < limits.budget {
            let end = (next + CHUNK).min(cap);
            let mut notes = vec![];
            for id in &ids[next..end] {
                match facts::read_fact(&src, id)? {
                    Some(f) if f.status == "active" => notes.push(f),
                    // Moved or retired by another call since the list was read.
                    _ => out.already += 1,
                }
            }
            next = end;
            if notes.is_empty() {
                continue;
            }
            let tx = begin(&src)?;
            let at = crate::util::now();
            for f in &notes {
                tx.execute(
                    "INSERT OR IGNORE INTO move_pending (source, target_space, who, at) VALUES (?1, ?2, ?3, ?4)",
                    params![f.id, to.as_str(), who, at],
                )?;
            }
            tx.commit()?;
            failpoint(Step::AfterJournal)?;
            let done = complete(&src, &dst, to, who, &notes)?;
            out.moved += done.moved;
            out.already += done.already;
            out.failed += done.failures.len();
            for f in done.failures {
                if out.failures.len() < FAILURES_SHOWN {
                    out.failures.push(f);
                }
            }
        }
        out.remaining = ids.len() - next;
        Ok(out)
    }

    /// Finishes the moves a crash or a killed process cut off, in every space: each note whose intent is recorded is
    /// copied to its target if the copy is missing, then retired in its source. Returns how many notes it finished.
    /// Safe at any time, and from several processes at once: each step is written once.
    pub fn finish_moves(&self) -> Result<usize> {
        let mut finished = 0;
        for from in self.spaces()? {
            let src = self.open_space(&from)?;
            if !has_journal(&src)? {
                continue;
            }
            let pending: Vec<(String, String, String)> = src
                .prepare("SELECT source, target_space, who FROM move_pending ORDER BY target_space, who, source")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for group in pending.chunk_by(|a, b| a.1 == b.1 && a.2 == b.2) {
                for part in group.chunks(CHUNK) {
                    finished += self.finish_part(&src, &part[0].1, &part[0].2, part)?;
                }
            }
        }
        Ok(finished)
    }

    fn finish_part(
        &self,
        src: &Connection,
        target: &str,
        who: &str,
        part: &[(String, String, String)],
    ) -> Result<usize> {
        let clear = |ids: &[&String]| -> Result<()> {
            let tx = begin(src)?;
            for id in ids {
                tx.execute("DELETE FROM move_pending WHERE source = ?1", [id])?;
            }
            tx.commit()?;
            Ok(())
        };
        // A target that no longer exists takes nothing: the notes stay where they are.
        let to = match Space::new(target) {
            Ok(s) if self.has_space(&s)? => s,
            _ => {
                clear(&part.iter().map(|p| &p.0).collect::<Vec<_>>())?;
                return Ok(0);
            }
        };
        let mut notes = vec![];
        let mut gone = vec![];
        for (id, _, _) in part {
            match facts::read_fact(src, id)? {
                Some(f) if f.status == "active" => notes.push(f),
                // Retired or replaced since: the move ended, or someone decided otherwise. Nothing to write.
                _ => gone.push(id),
            }
        }
        if !gone.is_empty() {
            clear(&gone)?;
        }
        if notes.is_empty() {
            return Ok(0);
        }
        let dst = self.open_space(&to)?;
        let done = complete(src, &dst, &to, who, &notes)?;
        Ok(done.moved + done.already)
    }
}
