//! Big files between nodes, in chunks: the event carries the list of chunks, the receiver applies it only once the
//! whole file is here and matches its digest, a chunk that does not match is refused, what came stays for later, and a
//! peer that takes no chunks gets none of the big files.

mod common;

use common::{Res, Temp, sp};
use pensine_core::chunks::{self, CHUNK_BYTES};
use pensine_core::facts::{self, NewFact, Remembered};
use pensine_core::files::{self, MAX_INLINE};
use pensine_core::sync::{self, Event};
use pensine_core::text::Language;
use pensine_core::{Brain, Space};

type Boxed = Box<dyn std::error::Error>;

fn node(t: &Temp, name: &str) -> Result<Brain, Boxed> {
    Ok(Brain::init(
        &t.path().join(name),
        &[sp("notes")],
        Language::English,
    )?)
}

/// A recording of `len` bytes (a WAV header, then bytes that differ from chunk to chunk), written to a file of `t`.
fn recording(t: &Temp, name: &str, len: usize) -> Result<std::path::PathBuf, Boxed> {
    let mut v = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    let mut x: u32 = 0x1234_5678;
    v.resize_with(len, || {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (x >> 24) as u8
    });
    let path = t.path().join(name);
    std::fs::write(&path, &v)?;
    Ok(path)
}

fn digest_of(path: &std::path::Path) -> Result<String, Boxed> {
    Ok(chunks::digest(&std::fs::read(path)?))
}

/// The file events of `space` after 0, as a peer that takes chunks (or not) gets them.
fn file_events(b: &Brain, s: &Space, with_chunks: bool) -> Result<sync::Page, Boxed> {
    Ok(sync::events_after_with(
        b,
        s,
        0,
        500,
        8 << 20,
        Some("x"),
        with_chunks,
    )?)
}

/// Gives the chunks `wants` asks for from `from` to `to`, as a pass does over the network.
fn give(from: &Brain, to: &Brain, s: &Space, w: &sync::Wanted) -> Res {
    for &n in &w.missing {
        let bytes = chunks::read(from, s, &w.file, n, None)?.ok_or("chunk not on the sender")?;
        chunks::store(to, s, &w.file, &w.chunks[n], &bytes)?;
    }
    Ok(())
}

#[test]
fn a_big_file_is_kept_from_its_path_and_travels_in_chunks_once_whole() -> Res {
    let t = Temp::new("chunks-big");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let s = sp("notes");
    let len = 30 * 1024 * 1024 + 123;
    let path = recording(&t, "long.wav", len)?;
    let f = files::put_path(&a, &s, "device", "long.wav", "", &path, &[])?;
    assert_eq!(f.mime, "audio/wav");
    assert_eq!(f.size, len as i64);
    assert_eq!(f.sha256, digest_of(&path)?);
    // Too big to read in one piece; served from where it is kept.
    assert!(files::get(&a, &s, &f.id).is_err());
    let (_, kept) = files::path_of(&a, &s, &f.id)?.ok_or("kept")?;
    assert_eq!(digest_of(&kept)?, f.sha256);

    // A peer that takes no chunks gets none of it; one that does gets the list of its chunks, never its bytes.
    let old = file_events(&a, &s, false)?;
    assert!(old.events.iter().all(|e| e.kind != "file_put"));
    assert_eq!(old.withheld.len(), 1);
    let new = file_events(&a, &s, true)?;
    let put = new
        .events
        .iter()
        .find(|e| e.kind == "file_put")
        .ok_or("no file")?;
    assert!(put.payload.get("data").is_none());
    assert_eq!(put.payload["chunk_size"], CHUNK_BYTES);
    let list = put.payload["chunks"].as_array().ok_or("chunks")?;
    assert_eq!(list.len(), len.div_ceil(CHUNK_BYTES));

    // B applies it only once every chunk is here: it asks for them all.
    let first = sync::apply(&b, &s, &new.events, "a")?;
    let w = first.wants.clone().ok_or("wants the chunks")?;
    assert_eq!(w.file, f.sha256);
    assert_eq!(w.missing.len(), list.len());
    assert!(first.stopped.is_some());
    assert!(
        files::info(&b, &s, &f.id)?.is_none(),
        "not held before whole"
    );

    // A chunk whose bytes do not match is refused; the good ones stay, even when the transfer stops halfway.
    let bad = vec![7u8; 100];
    assert!(chunks::store(&b, &s, &w.file, &w.chunks[0], &bad).is_err());
    let half = sync::Wanted {
        missing: w.missing[..3].to_vec(),
        ..w.clone()
    };
    give(&a, &b, &s, &half)?;
    let again = sync::apply(&b, &s, &new.events, "a")?;
    let rest = again.wants.ok_or("still wants")?;
    assert_eq!(
        rest.missing,
        w.missing[3..].to_vec(),
        "only the rest is asked"
    );
    assert_eq!(chunks::staged(&b, &s, &w.file), 3);
    give(&a, &b, &s, &rest)?;
    let done = sync::apply(&b, &s, &new.events, "a")?;
    assert!(done.wants.is_none() && done.stopped.is_none(), "{done:?}");
    let got = files::info(&b, &s, &f.id)?.ok_or("held")?;
    assert_eq!((got.size, got.mime.as_str()), (f.size, "audio/wav"));
    let (_, there) = files::path_of(&b, &s, &f.id)?.ok_or("bytes")?;
    assert_eq!(digest_of(&there)?, f.sha256);
    assert_eq!(
        chunks::staged(&b, &s, &w.file),
        0,
        "the chunks aside are gone"
    );

    // Passed on from B to a third node, the list of chunks goes with it.
    let from_b = file_events(&b, &s, true)?;
    let put_b = from_b
        .events
        .iter()
        .find(|e| e.kind == "file_put")
        .ok_or("no file")?;
    assert_eq!(put_b.payload["chunks"], put.payload["chunks"]);
    Ok(())
}

#[test]
fn a_list_of_chunks_that_lies_is_refused_and_its_chunks_go() -> Res {
    let t = Temp::new("chunks-lie");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let s = sp("notes");
    let path = recording(&t, "long.wav", MAX_INLINE + 10)?;
    let f = files::put_path(&a, &s, "device", "long.wav", "", &path, &[])?;
    let page = file_events(&a, &s, true)?;
    let mut events: Vec<Event> = page.events.clone();
    let put = events
        .iter_mut()
        .find(|e| e.kind == "file_put")
        .ok_or("no file")?;
    // The second chunk replaced by other bytes, with their own digest: each chunk matches, the whole does not.
    let other = vec![1u8; 10];
    put.payload["chunks"][1] = serde_json::json!(chunks::digest(&other));
    let w = sync::apply(&b, &s, &events, "a")?.wants.ok_or("wants")?;
    for &n in &w.missing {
        let bytes = if n == 1 {
            other.clone()
        } else {
            chunks::read(&a, &s, &f.sha256, n, None)?.ok_or("chunk")?
        };
        chunks::store(&b, &s, &w.file, &w.chunks[n], &bytes)?;
    }
    let done = sync::apply(&b, &s, &events, "a")?;
    assert_eq!(done.rejected.len(), 1, "{done:?}");
    assert!(files::info(&b, &s, &f.id)?.is_none());
    assert_eq!(chunks::staged(&b, &s, &f.sha256), 0);
    Ok(())
}

#[test]
fn a_chunk_of_a_file_nobody_asked_for_is_not_taken() -> Res {
    let t = Temp::new("chunks-unasked");
    let b = node(&t, "b")?;
    let s = sp("notes");
    let bytes = vec![3u8; 1000];
    let h = chunks::digest(&bytes);
    assert!(chunks::store(&b, &s, &"a".repeat(64), &h, &bytes).is_err());
    assert!(chunks::store(&b, &s, "../x", &h, &bytes).is_err());
    Ok(())
}

#[test]
fn a_big_recording_erased_at_its_origin_goes_everywhere_with_its_chunks() -> Res {
    let t = Temp::new("chunks-erase");
    let (a, b, c) = (node(&t, "a")?, node(&t, "b")?, node(&t, "c")?);
    let s = sp("notes");
    let path = recording(&t, "memo.wav", MAX_INLINE + CHUNK_BYTES * 2)?;
    let rec = files::put_path(
        &a,
        &s,
        "device",
        "memo.wav",
        "",
        &path,
        &["inbox/audio".to_string()],
    )?;
    let page = file_events(&a, &s, true)?;
    // B gets it whole; C gets one chunk only before the recording is erased.
    let w = sync::apply(&b, &s, &page.events, "a")?
        .wants
        .ok_or("wants")?;
    give(&a, &b, &s, &w)?;
    assert!(sync::apply(&b, &s, &page.events, "a")?.wants.is_none());
    assert!(files::path_of(&b, &s, &rec.id)?.is_some());
    let wc = sync::apply(&c, &s, &page.events, "a")?
        .wants
        .ok_or("wants")?;
    give(
        &a,
        &c,
        &s,
        &sync::Wanted {
            missing: vec![0],
            ..wc
        },
    )?;
    assert_eq!(chunks::staged(&c, &s, &rec.sha256), 1);

    let conn = a.open_space(&s)?;
    let Remembered::Created(text) = facts::remember(
        &conn,
        &s,
        "me",
        NewFact::agent(&format!("Memo {}: call the plumber.", rec.name), "agent:t"),
        &[],
    )?
    else {
        return Err("no text".into());
    };
    files::erase(&a, &s, "dictation", &rec.id, &s, &text.id)?;
    let after = file_events(&a, &s, true)?;
    let put = after
        .events
        .iter()
        .find(|e| e.kind == "file_put")
        .ok_or("no file")?;
    assert_eq!(put.payload["erased"], true);
    assert!(put.payload.get("chunks").is_none(), "never travels again");
    // Even to a peer that takes no chunks, the record goes, without its bytes.
    let old = file_events(&a, &s, false)?;
    assert!(old.withheld.is_empty(), "{:?}", old.withheld);

    for n in [&b, &c] {
        let done = sync::apply(n, &s, &after.events, "a")?;
        assert!(done.wants.is_none(), "{done:?}");
        assert!(files::erased(n, &s, &rec.id)?);
        assert!(files::path_of(n, &s, &rec.id)?.is_none());
        assert!(!files::blob_path(n, &s, &rec.sha256).exists());
        assert_eq!(chunks::staged(n, &s, &rec.sha256), 0);
    }
    Ok(())
}

#[test]
fn a_file_bigger_than_a_space_keeps_is_refused_from_its_path_and_in_one_piece() -> Res {
    let t = Temp::new("chunks-limits");
    let a = node(&t, "a")?;
    let s = sp("notes");
    // Up to the size read whole, `put_path` is `put`: same file, same record.
    let small = recording(&t, "small.wav", 1000)?;
    let one = files::put_path(&a, &s, "device", "small.wav", "", &small, &[])?;
    let two = files::put(&a, &s, "device", "small.wav", "", &std::fs::read(&small)?)?;
    assert_eq!(one.id, two.id);
    // The same big file twice is one file.
    let big = recording(&t, "big.wav", MAX_INLINE + 1)?;
    let x = files::put_path(&a, &s, "device", "big.wav", "", &big, &[])?;
    let y = files::put_path(&a, &s, "device", "big.wav", "", &big, &[])?;
    assert_eq!(x.id, y.id);
    // A sparse file of 2 GiB and one byte, never written: refused before a byte is read.
    let huge = t.path().join("huge.wav");
    std::fs::File::create(&huge)?.set_len(files::MAX_BYTES as u64 + 1)?;
    assert!(files::put_path(&a, &s, "device", "huge.wav", "", &huge, &[]).is_err());
    Ok(())
}
