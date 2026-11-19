//! Crash safety of the files: an operation stopped after any of its steps (as a crash, a `kill -9` or a power loss
//! would stop it) ends done once or not done once the node starts again (`Brain::recover`): the record, its note, its
//! event in the peers' log and its bytes agree, nothing is doubled, and an erased recording's bytes are gone here and
//! on the peers. See docs/CRASH.md.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Res, Temp, sp};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::failpoint::{self, at};
use memcp_core::files::{self, FileInfo};
use memcp_core::moves::{self, Step};
use memcp_core::sync::{self, Cursor};
use memcp_core::text::Language;
use memcp_core::{Brain, Space};

type Out<T> = Result<T, Box<dyn std::error::Error>>;

fn node(t: &Temp, name: &str) -> Out<Brain> {
    Ok(Brain::init(
        &t.path().join(name),
        &[sp("notes"), sp("home")],
        Language::English,
    )?)
}

fn notes() -> Space {
    sp("notes")
}

fn recording(seed: u8) -> Vec<u8> {
    let mut memo = vec![0, 0, 0, 0x20];
    memo.extend_from_slice(b"ftypM4A \0\0\0\0");
    memo.resize(300, seed);
    memo
}

/// Stops the operation at `point`, then starts the node again: what a crash there leaves, recovered.
fn crash<T>(
    b: &Brain,
    point: &'static str,
    op: impl FnOnce() -> memcp_core::Result<T>,
) -> Out<Brain> {
    failpoint::arm(Some(point));
    let r = op();
    failpoint::arm(None);
    let e = r.err().ok_or("the operation went through its failpoint")?;
    assert!(e.to_string().contains("failpoint"), "{e}");
    let b = Brain::open(b.dir())?;
    b.recover()?;
    Ok(b)
}

fn count(b: &Brain, sql: &str, arg: &str) -> Out<i64> {
    Ok(b.open_space(&notes())?
        .query_row(sql, [arg], |r| r.get(0))?)
}

fn events(b: &Brain, eid: &str) -> Out<i64> {
    count(b, "SELECT count(*) FROM sync_log WHERE eid = ?1", eid)
}

/// A record of a file: id, status, digest, retirement reason, note.
type Record = (String, String, String, Option<String>, Option<String>);

/// Every record of the space has its `f:` event once, a retired one its `fr:` once, its note is typed and in the same
/// state, and no bytes lie on disk that no record needs.
fn consistent(b: &Brain) -> Res {
    let conn = b.open_space(&notes())?;
    let rows: Vec<Record> = conn
        .prepare("SELECT id, status, sha256, retired_reason, fact_id FROM files")?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (id, status, sha, reason, fact) in &rows {
        assert_eq!(events(b, &format!("f:{id}"))?, 1, "f:{id}");
        let retired = i64::from(status == "retired");
        assert_eq!(events(b, &format!("fr:{id}"))?, retired, "fr:{id}");
        if let Some(f) = fact {
            let (t, s): (Option<String>, String) =
                conn.query_row("SELECT type, status FROM facts WHERE id = ?1", [f], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
            assert_eq!(t.as_deref(), Some("file"), "{f}");
            assert_eq!(s == "active", status == "active", "note {f} of {id}");
        }
        let erased = reason.as_deref().is_some_and(|r| r.starts_with("erased: "));
        let held = rows.iter().any(|(_, st, s, rr, _)| {
            s == sha
                && !(st == "retired" && rr.as_deref().is_some_and(|r| r.starts_with("erased: ")))
        });
        assert_eq!(
            files::blob_path(b, &notes(), sha).exists(),
            held,
            "bytes of {id} (erased {erased})"
        );
    }
    let dir = b.dir().join("files").join("notes");
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        assert!(!name.starts_with('.'), "half-written {name}");
        assert!(
            rows.iter().any(|r| r.2 == name),
            "bytes {name} named by no record"
        );
    }
    Ok(())
}

fn put(b: &Brain, name: &str, bytes: &[u8]) -> memcp_core::Result<FileInfo> {
    files::put_tagged(
        b,
        &notes(),
        "device",
        name,
        "",
        bytes,
        &["inbox/audio".to_string()],
    )
}

fn write(b: &Brain, text: &str) -> Out<String> {
    let conn = b.open_space(&notes())?;
    match facts::remember(&conn, &notes(), "me", NewFact::agent(text, "agent:t"), &[])? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn notes_named(b: &Brain, name: &str) -> Out<i64> {
    count(
        b,
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE ?1",
        &format!("File: {name}%"),
    )
}

#[test]
fn a_put_cut_off_at_any_step_is_not_done_and_its_retry_is_done_once() -> Res {
    for point in [at::FILE_PUT_BYTES, at::FILE_PUT_COMMIT] {
        let t = Temp::new("crash-put");
        let b = node(&t, "a")?;
        let bytes = recording(1);
        let b = crash(&b, point, || put(&b, "memo.m4a", &bytes))?;
        assert_eq!(
            count(&b, "SELECT count(*) FROM files WHERE name = ?1", "memo.m4a")?,
            0
        );
        assert_eq!(
            notes_named(&b, "memo.m4a")?,
            0,
            "{point}: no note without its file"
        );
        consistent(&b)?;
        let f = put(&b, "memo.m4a", &bytes)?;
        assert_eq!(
            put(&b, "memo.m4a", &bytes)?.id,
            f.id,
            "the same file twice is one"
        );
        assert_eq!(
            notes_named(&b, "memo.m4a")?,
            1,
            "{point}: one note, not fingerprinted"
        );
        assert!(files::get(&b, &notes(), &f.id)?.is_some());
        consistent(&b)?;
    }
    Ok(())
}

#[test]
fn a_big_put_cut_off_leaves_no_bytes_and_no_half_written_file() -> Res {
    let t = Temp::new("crash-big");
    let b = node(&t, "a")?;
    let src = t.path().join("big.txt");
    std::fs::write(&src, vec![b'a'; files::MAX_INLINE + 4096])?;
    // A copy left half-written by a killed process.
    let dir = b.dir().join("files").join("notes");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(".in-killed.tmp"), b"half")?;
    let put_big = |b: &Brain| files::put_path(b, &notes(), "device", "big.txt", "", &src, &[]);
    let b = crash(&b, at::FILE_PUT_BYTES, || put_big(&b))?;
    assert_eq!(
        count(&b, "SELECT count(*) FROM files WHERE name = ?1", "big.txt")?,
        0
    );
    consistent(&b)?;
    let f = put_big(&b)?;
    let page = sync::events_after_with(&b, &notes(), 0, 100, 1 << 20, None, true)?;
    let ev = page
        .events
        .iter()
        .find(|e| e.eid == format!("f:{}", f.id))
        .ok_or("no event")?;
    assert!(
        ev.payload["chunks"].is_array(),
        "a big file travels in chunks"
    );
    consistent(&b)?;
    Ok(())
}

#[test]
fn a_retirement_cut_off_is_not_done_and_its_retry_retires_once() -> Res {
    let t = Temp::new("crash-retire");
    let b = node(&t, "a")?;
    let f = put(&b, "memo.m4a", &recording(2))?;
    let b = crash(&b, at::FILE_RETIRE_COMMIT, || {
        files::retire(&b, &notes(), "me", &f.id, "not needed")
    })?;
    assert_eq!(
        files::info(&b, &notes(), &f.id)?.ok_or("gone")?.status,
        "active"
    );
    consistent(&b)?;
    files::retire(&b, &notes(), "me", &f.id, "not needed")?;
    assert_eq!(
        files::info(&b, &notes(), &f.id)?.ok_or("gone")?.status,
        "retired"
    );
    consistent(&b)?;
    Ok(())
}

/// Two nodes; `a` holds a recording `b` received, and its transcript.
fn erasable(t: &Temp) -> Out<(Brain, Brain, FileInfo, String)> {
    let (a, b) = (node(t, "a")?, node(t, "b")?);
    let f = put(&a, "memo.m4a", &recording(3))?;
    catch_up(&a, &b)?;
    assert!(
        files::get(&b, &notes(), &f.id)?.is_some(),
        "b holds the bytes"
    );
    let text = write(&a, &format!("Memo, audio {}: call the plumber.", f.name))?;
    Ok((a, b, f, text))
}

fn catch_up(from: &Brain, to: &Brain) -> Res {
    loop {
        let (pulled, _) = sync::cursors(to, "a", &notes())?;
        let page = sync::events_after(from, &notes(), pulled, 50, 1 << 22, Some("b"))?;
        let r = sync::apply(to, &notes(), &page.events, "a")?;
        assert!(r.stopped.is_none(), "{:?}", r.stopped);
        sync::set_cursor(to, "a", &notes(), Cursor::Pulled, page.next)?;
        if !page.more {
            return Ok(());
        }
    }
}

fn erase(b: &Brain, f: &FileInfo, text: &str) -> memcp_core::Result<FileInfo> {
    files::erase(b, &notes(), "dictation", &f.id, &notes(), text)
}

#[test]
fn an_erasure_cut_off_before_its_commit_is_not_done_and_its_retry_reaches_the_peer() -> Res {
    let t = Temp::new("crash-erase-tx");
    let (a, b, f, text) = erasable(&t)?;
    let a = crash(&a, at::FILE_ERASE_COMMIT, || erase(&a, &f, &text))?;
    assert!(!files::erased(&a, &notes(), &f.id)?);
    assert!(files::get(&a, &notes(), &f.id)?.is_some(), "untouched");
    consistent(&a)?;
    erase(&a, &f, &text)?;
    consistent(&a)?;
    catch_up(&a, &b)?;
    assert!(files::erased(&b, &notes(), &f.id)?);
    assert!(
        !files::blob_path(&b, &notes(), &f.sha256).exists(),
        "the peer erased its bytes"
    );
    Ok(())
}

#[test]
fn an_erasure_cut_off_after_its_commit_loses_its_bytes_at_restart_and_reaches_the_peer() -> Res {
    let t = Temp::new("crash-erase-bytes");
    let (a, b, f, text) = erasable(&t)?;
    let a = crash(&a, at::FILE_ERASE_BYTES, || erase(&a, &f, &text))?;
    assert!(files::erased(&a, &notes(), &f.id)?);
    assert_eq!(
        events(&a, &format!("fr:{}", f.id))?,
        1,
        "the erasure is told once"
    );
    consistent(&a)?; // the bytes are gone: the restart swept them
    // Asked again (the answer never came): done already, not refused.
    assert!(erase(&a, &f, &text).is_ok());
    assert_eq!(events(&a, &format!("fr:{}", f.id))?, 1);
    // The peer erases them too, even when it is cut off right after the commit of what it received.
    let page = sync::events_after(
        &a,
        &notes(),
        sync::cursors(&b, "a", &notes())?.0,
        50,
        1 << 22,
        Some("b"),
    )?;
    let b = crash(&b, at::SYNC_APPLY_BYTES, || {
        sync::apply(&b, &notes(), &page.events, "a")
    })?;
    sync::set_cursor(&b, "a", &notes(), Cursor::Pulled, page.next)?;
    assert!(files::erased(&b, &notes(), &f.id)?);
    assert!(
        !files::blob_path(&b, &notes(), &f.sha256).exists(),
        "the peer's bytes went at its restart"
    );
    assert_eq!(events(&b, &format!("fr:{}", f.id))?, 1);
    consistent(&b)?;
    Ok(())
}

#[test]
fn a_record_an_earlier_version_left_unlogged_is_logged_once_at_start() -> Res {
    let t = Temp::new("crash-legacy");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    // What a crash between the steps of an earlier version left: records with no event (the log is append-only, so
    // they are written as that version did), and a note left untyped.
    let (kept, gone) = (recording(4), recording(5));
    let digest = |b: &[u8]| memcp_core::chunks::digest(b);
    let dir = a.dir().join("files").join("notes");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(digest(&kept)), &kept)?;
    std::fs::write(dir.join(digest(&gone)), &gone)?;
    let note = write(&a, "File: kept.m4a")?;
    let conn = a.open_space(&notes())?;
    let row = "INSERT INTO files (id, name, mime, size, sha256, note, fact_id, author, created_at, status, retired_reason)
               VALUES (?1, ?2, 'audio/mp4', 300, ?3, '', ?4, 'device', 1, ?5, ?6)";
    conn.execute(
        row,
        rusqlite::params![
            "file_kept",
            "kept.m4a",
            digest(&kept),
            note,
            "active",
            None::<String>
        ],
    )?;
    conn.execute(
        row,
        rusqlite::params![
            "file_gone",
            "gone.m4a",
            digest(&gone),
            None::<String>,
            "retired",
            "old"
        ],
    )?;
    let (kept, gone) = (
        files::info(&a, &notes(), "file_kept")?.ok_or("no record")?,
        files::info(&a, &notes(), "file_gone")?.ok_or("no record")?,
    );
    let r = a.recover()?;
    assert_eq!(r.events, 3, "{r:?}");
    assert_eq!(a.recover()?.events, 0, "twice is once");
    consistent(&a)?;
    catch_up(&a, &b)?;
    assert!(
        files::get(&b, &notes(), &kept.id)?.is_some(),
        "the peer gets the file"
    );
    assert_eq!(
        files::info(&b, &notes(), &gone.id)?
            .ok_or("not there")?
            .status,
        "retired"
    );
    Ok(())
}

#[test]
fn the_sweep_removes_what_writes_left_half_done_and_keeps_a_transfer_under_way() -> Res {
    let t = Temp::new("crash-sweep");
    let b = node(&t, "a")?;
    let f = put(&b, "memo.m4a", &recording(6))?;
    let dir = b.dir().join("files").join("notes");
    std::fs::write(dir.join(format!(".{}.assembling", "c".repeat(64))), b"x")?;
    std::fs::write(dir.join(format!(".{}.tmp", "d".repeat(64))), b"x")?;
    std::fs::write(dir.join("e".repeat(64)), b"no record names me")?;
    let staging = b.dir().join("sync-chunks").join("notes");
    // Chunks of a file whose bytes are whole: useless. Chunks of a file still coming: kept, but not a half chunk.
    std::fs::create_dir_all(staging.join(&f.sha256))?;
    let coming = staging.join("f".repeat(64));
    std::fs::create_dir_all(&coming)?;
    std::fs::write(coming.join("1".repeat(64)), b"chunk")?;
    std::fs::write(coming.join(format!(".{}.tmp", "2".repeat(64))), b"half")?;
    let r = b.recover()?;
    assert!(r.swept);
    assert_eq!((r.blobs, r.temporary, r.staged), (1, 3, 1), "{r:?}");
    assert!(coming.join("1".repeat(64)).exists());
    consistent(&b)?;
    assert!(b.recover()?.is_clean());
    Ok(())
}

#[test]
fn a_mark_of_what_was_left_out_for_a_peer_stays_until_cleared_with_its_position() -> Res {
    let t = Temp::new("crash-withheld");
    let b = node(&t, "a")?;
    sync::note_withheld(&b, &notes(), "peer", 7)?;
    // Read twice, still there: a crash after reading loses nothing.
    assert_eq!(sync::withheld(&b, &notes(), "peer")?, Some(7));
    assert_eq!(sync::withheld(&b, &notes(), "peer")?, Some(7));
    sync::note_withheld(&b, &notes(), "peer", 4)?;
    sync::clear_withheld(&b, &notes(), "peer", 7)?;
    assert_eq!(
        sync::withheld(&b, &notes(), "peer")?,
        Some(4),
        "an earlier mark noted meanwhile stays"
    );
    sync::clear_withheld(&b, &notes(), "peer", 4)?;
    assert_eq!(sync::withheld(&b, &notes(), "peer")?, None);
    Ok(())
}

#[test]
fn a_note_whose_move_was_cut_off_is_left_to_the_move_then_corrected() -> Res {
    let t = Temp::new("crash-move-supersede");
    let b = node(&t, "a")?;
    let conn = b.open_space(&notes())?;
    let mut new = NewFact::agent("2026-11-19, the boiler is serviced in March.", "agent:t");
    new.kind = Some("decision".into());
    new.tags = vec!["house".into()];
    let Remembered::Created(f) = facts::remember(&conn, &notes(), "me", new, &[])? else {
        return Err("not created".into());
    };
    moves::fail_at(Some(Step::AfterCopies));
    let r = b.move_by_tag(&notes(), &sp("home"), "house", "me", false);
    moves::fail_at(None);
    assert!(r.is_err());
    let fix = NewFact::agent("2026-11-19, the boiler is serviced in April.", "agent:t");
    let refused = facts::supersede(&conn, &notes(), "me", &f.id, fix.clone(), None);
    assert!(refused.is_err_and(|e| e.to_string().contains("being moved")));
    assert!(facts::retire(&conn, "me", &f.id, "wrong").is_err());
    // The node starts again: the move ends, and the note is corrected where it went, as a decision still.
    let b = Brain::open(b.dir())?;
    assert_eq!(b.recover()?.moves, 1);
    let home = b.open_space(&sp("home"))?;
    let copy = memcp_core::triage::filed_id(&f.id, &sp("home"));
    let fixed = facts::supersede(&home, &sp("home"), "me", &copy, fix, None)?;
    assert_eq!(fixed.kind, "decision", "an omitted kind keeps the old one");
    let active: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE '%boiler%'",
        [],
        |r| r.get(0),
    )?;
    let there: i64 = home.query_row(
        "SELECT count(*) FROM facts WHERE status = 'active' AND text LIKE '%boiler%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!((active, there), (0, 1));
    Ok(())
}

/// The child of `kill_9_while_big_files_are_put`: puts big files from the directory it is given until it is killed.
/// Does nothing in a plain test run.
#[test]
fn child_putting_big_files() -> Res {
    let Ok(dir) = std::env::var("MEMCP_CRASH_CHILD_DIR") else {
        return Ok(());
    };
    let b = Brain::open(Path::new(&dir))?;
    for i in 0..1000 {
        let src = Path::new(&dir).join(format!("src-{}.txt", i % 4));
        let name = format!("big-{i}.txt");
        files::put_path(&b, &notes(), "device", &name, "", &src, &[])?;
        let f = files::list(&b, &notes(), Some(&name), 1)?;
        if i % 3 == 0
            && let Some(f) = f.first()
        {
            files::retire(&b, &notes(), "device", &f.id, "test")?;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn kill_9_while_big_files_are_put() -> Res {
    let t = Temp::new("crash-kill");
    let b = node(&t, "a")?;
    for i in 0..4u8 {
        let mut bytes = vec![b'a' + i; files::MAX_INLINE + 1024 * 1024];
        bytes[0] = b'0' + i;
        std::fs::write(b.dir().join(format!("src-{i}.txt")), bytes)?;
    }
    let exe = std::env::current_exe()?;
    for wait_ms in [30u64, 80, 150, 250, 400, 700] {
        let mut child = std::process::Command::new(&exe)
            .args([
                "child_putting_big_files",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MEMCP_CRASH_CHILD_DIR", b.dir())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        std::thread::sleep(Duration::from_millis(wait_ms));
        child.kill()?; // SIGKILL
        child.wait()?;
        let b = Brain::open(b.dir())?;
        let r = b.recover()?;
        println!("kill after {wait_ms} ms: {}", r.summary());
        assert!(r.swept);
        consistent(&b)?;
        let notes_without_file: i64 = count(
            &b,
            "SELECT count(*) FROM facts f WHERE f.text LIKE ?1
             AND NOT EXISTS (SELECT 1 FROM files WHERE fact_id = f.id)",
            "File: %",
        )?;
        assert_eq!(notes_without_file, 0, "a note with no file");
    }
    Ok(())
}

#[test]
fn a_pruning_cut_off_after_its_commit_loses_the_bytes_at_restart() -> Res {
    let t = Temp::new("crash-prune");
    let b = node(&t, "a")?;
    let f = put(&b, "memo.m4a", &recording(7))?;
    // A peer that took everything: the old notes may go from this replica.
    sync::set_cursor(
        &b,
        "peer",
        &notes(),
        Cursor::Pushed,
        sync::head(&b, &notes())?,
    )?;
    let later = memcp_core::util::now() + 90 * 86_400;
    let b = crash(&b, at::SYNC_PRUNE_BYTES, || {
        sync::prune_window(&b, &notes(), 30, later)
    })?;
    assert!(files::info(&b, &notes(), &f.id)?.is_none(), "pruned");
    assert!(
        !files::blob_path(&b, &notes(), &f.sha256).exists(),
        "its bytes went at restart"
    );
    consistent(&b)?;
    Ok(())
}
