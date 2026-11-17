//! Files kept in a space: type from the content, stored once, findable, backed up,
//! never erased, and only within the caller's spaces.

mod common;

use std::fs;

use common::{Res, Temp, brain, sp};
use memcp_core::access::{self, Rights};
use memcp_core::files::{self, sniff};
use memcp_core::tools::{self, ToolError};
use serde_json::json;

const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
];

#[test]
fn the_type_comes_from_the_content() {
    assert_eq!(
        sniff(PNG, "x.txt"),
        Some("image/png"),
        "the name does not decide"
    );
    assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0], "a"), Some("image/jpeg"));
    assert_eq!(
        sniff(b"%PDF-1.7 ...", "manual.pdf"),
        Some("application/pdf")
    );
    assert_eq!(sniff(b"# Notes\nboiler", "notes.md"), Some("text/markdown"));
    assert_eq!(sniff(b"plain words", "x"), Some("text/plain"));
    assert_eq!(
        sniff(b"PK\x03\x04\0\0\0\0", "accounts.XLSX"),
        Some(files::XLSX),
        "a workbook is a zip container named .xlsx"
    );
    assert_eq!(
        sniff(b"PK\x03\x04\0\0\0\0", "tool.jar"),
        None,
        "another archive is refused"
    );
    assert_eq!(
        sniff(&[0x4D, 0x5A, 0x90, 0x00, 0x03], "photo.jpg"),
        None,
        "an executable is refused"
    );
}

#[test]
fn a_file_is_kept_once_found_by_its_note_and_never_erased() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let home = sp("home");
    let f = files::put(
        &b,
        &home,
        "sam",
        "fuse-box.png",
        "Fuse box, kitchen circuit labelled",
        PNG,
    )?;
    assert_eq!((f.mime.as_str(), f.size), ("image/png", 16));
    let again = files::put(&b, &home, "sam", "fuse-box-copy.png", "Same picture", PNG)?;
    let blobs = fs::read_dir(b.dir().join("files/home"))?.count();
    assert_eq!(blobs, 1, "same content stored once");
    assert_ne!(f.id, again.id);

    let (info, bytes) = files::get(&b, &home, &f.id)?.ok_or("missing")?;
    assert_eq!(
        (info.name.as_str(), bytes.as_slice()),
        ("fuse-box.png", PNG)
    );
    assert_eq!(files::list(&b, &home, Some("kitchen fuse"), 10)?.len(), 1);

    // recall finds it through its fact.
    let ctl = b.open_control()?;
    let tok = access::connect(&ctl, "a", &Rights::parse("home:rw")?, None, false, false)?;
    let c = access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?;
    let r = tools::call(&b, &c, "recall", &json!({ "query": "fuse box kitchen" }))?;
    assert!(
        r["results"]
            .as_array()
            .is_some_and(|v| v.iter().any(|x| x["type"] == "file")),
        "{r}"
    );

    files::retire(&b, &home, "sam", &f.id, "blurry")?;
    assert!(files::list(&b, &home, Some("kitchen"), 10)?.is_empty());
    assert!(files::get(&b, &home, &f.id)?.is_some(), "still on disk");
    Ok(())
}

fn m4a(len: usize) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftypM4A \0\0\0\0");
    v.resize(len, 7);
    v
}

#[test]
fn a_recording_of_any_size_is_kept() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let home = sp("home");
    // Recognised by its content, whatever the name: a voice memo is far bigger than a photo may be.
    let big = m4a(30 * 1024 * 1024);
    let f = files::put(&b, &home, "sam", "Mémo vocal.m4a", "dictée du matin", &big)?;
    assert_eq!(f.mime, "audio/mp4");
    assert!(f.is_audio() && !f.is_image());
    let (_, back) = files::get(&b, &home, &f.id)?.ok_or("kept")?;
    assert_eq!(back, big);
    // The other kinds, by their first bytes.
    let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
    wav.resize(100, 0);
    assert_eq!(
        files::put(&b, &home, "sam", "a.wav", "", &wav)?.mime,
        "audio/wav"
    );
    let mut ogg = b"OggS".to_vec();
    ogg.resize(100, 1);
    assert_eq!(
        files::put(&b, &home, "sam", "a.opus", "", &ogg)?.mime,
        "audio/ogg"
    );
    // A video that only says `mp42` is not a recording, unless its name says so.
    let mut video = vec![0, 0, 0, 0x20];
    video.extend_from_slice(b"ftypmp42\0\0\0\0");
    video.resize(100, 3);
    assert!(files::put(&b, &home, "sam", "clip.mp4", "", &video).is_err());
    assert_eq!(
        files::put(&b, &home, "sam", "clip.m4a", "", &video)?.mime,
        "audio/mp4"
    );
    // No size limit, for a document either.
    let mut text = vec![b'a'; 6 * 1024 * 1024];
    text[0] = b'b';
    assert!(files::put(&b, &home, "sam", "big.txt", "", &text).is_ok());
    Ok(())
}

#[test]
fn backups_carry_the_files() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let f = files::put(
        &b,
        &sp("home"),
        "sam",
        "manual.pdf",
        "Boiler manual",
        b"%PDF-1.4 fake",
    )?;
    let dest = b.backup(&t.path().join("backups"), 3, 1_000)?;
    assert!(dest.join("files/home").join(&f.sha256).is_file());
    Ok(())
}

#[test]
fn the_tools_stay_within_the_caller_spaces() -> Res {
    use base64::Engine as _;
    let t = Temp::new("files");
    let b = brain(&t)?;
    let ctl = b.open_control()?;
    let mk = |name: &str, spec: &str| -> Result<access::Caller, Box<dyn std::error::Error>> {
        let tok = access::connect(&ctl, name, &Rights::parse(spec)?, None, false, false)?;
        access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or_else(|| "refused".into())
    };
    let home = mk("home-agent", "home:rw")?;
    let health = mk("health-agent", "health:rw")?;
    let data = base64::engine::general_purpose::STANDARD.encode(PNG);
    let put = tools::call(
        &b,
        &home,
        "file_put",
        &json!({ "name": "meter.png", "note": "Electricity meter reading", "data_base64": data }),
    )?;
    let id = put["file"]["id"].as_str().ok_or("id")?.to_string();
    let got = tools::call(&b, &home, "file_get", &json!({ "id": id }))?;
    assert_eq!(got["mcp_content"][0]["type"], "image");
    assert_eq!(got["mcp_content"][0]["mimeType"], "image/png");
    assert!(
        matches!(
            tools::call(&b, &health, "file_get", &json!({ "id": id })),
            Err(ToolError::Invalid(_))
        ),
        "another space: unknown"
    );
    assert_eq!(
        tools::call(&b, &health, "file_list", &json!({}))?["files"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    let bad = tools::call(
        &b,
        &home,
        "file_put",
        &json!({ "name": "x.exe", "data_base64": base64::engine::general_purpose::STANDARD.encode([0x4D, 0x5A, 0x90, 0]) }),
    );
    assert!(matches!(bad, Err(ToolError::Invalid(_))));
    Ok(())
}

// --- erasing a recording once it is transcribed ------------------------------------------------
//
// The one exception to « nothing is erased »: a voice memo waits in the inbox as audio, a
// program transcribes it and deposits the text, and seven days later the recording itself is
// erased, with a trace. Nothing else is ever erased.

const DAY: i64 = 86_400;

fn exists(b: &memcp_core::Brain, space: &memcp_core::Space, sha: &str) -> bool {
    b.dir()
        .join("files")
        .join(space.as_str())
        .join(sha)
        .exists()
}

/// A recording filed as waiting for its transcription, and the fact that holds its text.
fn memo_and_transcript(
    b: &memcp_core::Brain,
    space: &memcp_core::Space,
) -> Result<(files::FileInfo, String), Box<dyn std::error::Error>> {
    use memcp_core::facts::{self, NewFact, Remembered};
    let memo = files::put_tagged(
        b,
        space,
        "bridge",
        "Mémo vocal.m4a",
        "dictée du matin",
        &m4a(200),
        &["inbox/audio".to_string()],
    )?;
    let conn = b.open_space(space)?;
    let mut text = NewFact::agent(
        "Dictée du matin, audio Mémo vocal.m4a: penser à rappeler le plombier.",
        "agent:dictee",
    );
    text.tags = vec!["inbox".into(), "dictee".into()];
    let Remembered::Created(fact) = facts::remember(&conn, space, "dictee", text, &[])? else {
        return Err("the transcript was not created".into());
    };
    Ok((memo, fact.id))
}

fn transcribed_at(
    b: &memcp_core::Brain,
    space: &memcp_core::Space,
    fact: &str,
    at: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = b.open_space(space)?;
    conn.execute(
        "UPDATE facts SET created_at = ?2 WHERE id = ?1",
        rusqlite::params![fact, at],
    )?;
    // The recordings were made before their text.
    conn.execute(
        "UPDATE files SET created_at = min(created_at, ?1 - 60)",
        [at],
    )?;
    Ok(())
}

#[test]
fn a_transcribed_recording_is_erased_after_seven_days_with_a_trace() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let personal = sp("personal");
    let (memo, transcript) = memo_and_transcript(&b, &personal)?;
    let then = memcp_core::util::now() - 30 * DAY;
    transcribed_at(&b, &personal, &transcript, then)?;
    assert!(exists(&b, &personal, &memo.sha256));

    // One second short of seven days: refused, and the bytes stay.
    let early = files::erase_at(
        &b,
        &personal,
        "dictee",
        &memo.id,
        &personal,
        &transcript,
        then + 7 * DAY - 1,
    );
    assert!(
        matches!(&early, Err(e) if e.to_string().contains("7 days")),
        "{early:?}"
    );
    assert!(exists(&b, &personal, &memo.sha256));

    let done = files::erase_at(
        &b,
        &personal,
        "dictee",
        &memo.id,
        &personal,
        &transcript,
        then + 7 * DAY,
    )?;
    assert_eq!(done.status, "retired");
    assert!(!exists(&b, &personal, &memo.sha256), "the bytes are gone");
    assert!(
        files::get(&b, &personal, &memo.id)?.is_none(),
        "nothing to read"
    );
    assert!(files::list(&b, &personal, None, 10)?.is_empty());
    assert!(files::erased(&b, &personal, &memo.id)?);

    // The trace: what, when, why, and the text that replaces it.
    let conn = b.open_space(&personal)?;
    let reason: String = conn.query_row(
        "SELECT retired_reason FROM files WHERE id = ?1",
        [&memo.id],
        |r| r.get(0),
    )?;
    assert!(reason.starts_with("erased: "), "{reason}");
    assert!(reason.contains(&transcript), "{reason}");
    // The record of the file stays (name, size, fingerprint, author), and so does its fact.
    assert_eq!(done.name, "Mémo vocal.m4a");
    assert_eq!(done.size, 200);
    let fact_status: String = conn.query_row(
        "SELECT status FROM facts WHERE id = ?1",
        [memo.fact_id.as_deref().ok_or("fact")?],
        |r| r.get(0),
    )?;
    assert_eq!(fact_status, "retired");
    // The text that replaces it is untouched.
    let kept: String = conn.query_row(
        "SELECT status FROM facts WHERE id = ?1",
        [&transcript],
        |r| r.get(0),
    )?;
    assert_eq!(kept, "active");
    // Twice is an error, not a second erasure.
    assert!(
        files::erase_at(
            &b,
            &personal,
            "dictee",
            &memo.id,
            &personal,
            &transcript,
            then + 8 * DAY
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn only_a_recording_waiting_for_its_transcription_is_ever_erased() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let personal = sp("personal");
    let (memo, transcript) = memo_and_transcript(&b, &personal)?;
    let long_ago = 1_700_000_000;
    transcribed_at(&b, &personal, &transcript, long_ago)?;
    let now = memcp_core::util::now();

    // A photo, however old: never.
    let photo = files::put_tagged(
        &b,
        &personal,
        "bridge",
        "meter.png",
        "",
        PNG,
        &["inbox/audio".to_string()],
    )?;
    let r = files::erase_at(&b, &personal, "x", &photo.id, &personal, &transcript, now);
    assert!(
        matches!(&r, Err(e) if e.to_string().contains("recording")),
        "{r:?}"
    );
    assert!(exists(&b, &personal, &photo.sha256));

    // A recording that is not filed under inbox/audio (a dictation kept on purpose): never.
    let kept = files::put_tagged(
        &b,
        &personal,
        "bridge",
        "Discours.m4a",
        "à garder",
        &m4a(300),
        &["work/talks".to_string()],
    )?;
    let r = files::erase_at(&b, &personal, "x", &kept.id, &personal, &transcript, now);
    assert!(
        matches!(&r, Err(e) if e.to_string().contains("inbox/audio")),
        "{r:?}"
    );
    assert!(exists(&b, &personal, &kept.sha256));

    // No transcript, an unknown one, the file's own fact, a retired one: no erasure.
    for bad in ["", "f_unknown", memo.fact_id.as_deref().ok_or("fact")?] {
        assert!(
            files::erase_at(&b, &personal, "x", &memo.id, &personal, bad, now).is_err(),
            "{bad:?}"
        );
    }
    let conn = b.open_space(&personal)?;
    memcp_core::facts::retire(&conn, "x", &transcript, "wrong")?;
    assert!(files::erase_at(&b, &personal, "x", &memo.id, &personal, &transcript, now).is_err());
    assert!(exists(&b, &personal, &memo.sha256));
    // An unknown file.
    assert!(files::erase_at(&b, &personal, "x", "file_nope", &personal, &transcript, now).is_err());
    Ok(())
}

#[test]
fn bytes_that_another_record_or_space_still_holds_stay() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let personal = sp("personal");
    let work = sp("work");
    let (memo, transcript) = memo_and_transcript(&b, &personal)?;
    transcribed_at(&b, &personal, &transcript, 1_700_000_000)?;
    let now = memcp_core::util::now();
    let tag = ["inbox/audio".to_string()];
    // The same recording under another name in the same space, and in another space.
    let twin = files::put_tagged(&b, &personal, "bridge", "Copie.m4a", "", &m4a(200), &tag)?;
    let other = files::put_tagged(&b, &work, "bridge", "Mémo vocal.m4a", "", &m4a(200), &tag)?;
    transcribed_at(&b, &personal, &transcript, 1_700_000_000)?;
    assert_eq!(twin.sha256, memo.sha256);
    assert_ne!(twin.id, memo.id);

    // One text names both recordings.
    b.open_space(&personal)?.execute(
        "UPDATE facts SET text = text || ' Copie.m4a' WHERE id = ?1",
        [&transcript],
    )?;
    files::erase_at(&b, &personal, "x", &memo.id, &personal, &transcript, now)?;
    assert!(
        exists(&b, &personal, &twin.sha256),
        "the twin record still needs it"
    );
    assert!(files::get(&b, &personal, &twin.id)?.is_some());

    files::erase_at(&b, &personal, "x", &twin.id, &personal, &transcript, now)?;
    assert!(
        !exists(&b, &personal, &twin.sha256),
        "no record left holds it"
    );
    assert!(
        exists(&b, &work, &other.sha256),
        "another space keeps its own copy"
    );
    assert!(files::get(&b, &work, &other.id)?.is_some());
    Ok(())
}

#[test]
fn the_tool_is_for_a_whole_space_with_write_rights_and_stays_in_its_spaces() -> Res {
    let t = Temp::new("files");
    let b = brain(&t)?;
    let personal = sp("personal");
    let (memo, transcript) = memo_and_transcript(&b, &personal)?;
    let ctl = b.open_control()?;
    let mk = |name: &str,
              spec: &str,
              read_only: bool|
     -> Result<access::Caller, Box<dyn std::error::Error>> {
        let tok = access::connect(&ctl, name, &Rights::parse(spec)?, None, read_only, false)?;
        access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or_else(|| "refused".into())
    };
    let worker = mk("worker", "personal:rw", false)?;
    let scoped = mk("bridge", "personal/inbox:rw", false)?;
    let reader = mk("reader", "personal:r", false)?;
    let home = mk("home-agent", "home:rw", false)?;
    let args = json!({ "id": memo.id, "transcript": transcript });
    for (who, name) in [(&scoped, "scoped"), (&reader, "reader")] {
        assert!(
            matches!(
                tools::call(&b, who, "file_erase", &args),
                Err(ToolError::Unknown)
            ),
            "{name} does not get the tool"
        );
        assert!(
            !tools::definitions(who)
                .iter()
                .any(|d| d.name == "file_erase"),
            "{name}"
        );
    }
    assert!(
        tools::definitions(&worker)
            .iter()
            .any(|d| d.name == "file_erase")
    );
    assert!(
        matches!(
            tools::call(&b, &home, "file_erase", &args),
            Err(ToolError::Invalid(_))
        ),
        "another space: unknown file"
    );
    // Too early, said in plain words; the recording stays.
    let early = tools::call(&b, &worker, "file_erase", &args);
    assert!(
        matches!(&early, Err(ToolError::Invalid(m)) if m.contains("7 days")),
        "{early:?}"
    );
    assert!(exists(&b, &personal, &memo.sha256));
    transcribed_at(
        &b,
        &personal,
        &transcript,
        memcp_core::util::now() - 8 * DAY,
    )?;
    let done = tools::call(&b, &worker, "file_erase", &args)?;
    assert_eq!(done["result"], "erased");
    assert!(!exists(&b, &personal, &memo.sha256));
    let got = tools::call(&b, &worker, "file_get", &json!({ "id": memo.id }));
    assert!(
        matches!(got, Err(ToolError::Invalid(_))),
        "nothing left to read"
    );
    Ok(())
}

#[test]
fn the_transcription_may_have_been_sorted_into_another_space() -> Res {
    use memcp_core::facts::{self, NewFact, Remembered};
    let t = Temp::new("files");
    let b = brain(&t)?;
    let personal = sp("personal");
    let work = sp("work");
    let health = sp("health");
    let (memo, _inbox_note) = memo_and_transcript(&b, &personal)?;
    // The text was routed to « work » once written; the inbox note left.
    let conn = b.open_space(&work)?;
    let Remembered::Created(text) = facts::remember(
        &conn,
        &work,
        "dictee",
        NewFact::agent(
            "Réunion, audio Mémo vocal.m4a: décider du budget.",
            "agent:dictee",
        ),
        &[],
    )?
    else {
        return Err("transcript".into());
    };
    transcribed_at(&b, &work, &text.id, memcp_core::util::now() - 9 * DAY)?;
    b.open_space(&personal)?.execute(
        "UPDATE files SET created_at = ?1",
        [memcp_core::util::now() - 10 * DAY],
    )?;
    let ctl = b.open_control()?;
    let mk = |name: &str, spec: &str| -> Result<access::Caller, Box<dyn std::error::Error>> {
        let tok = access::connect(&ctl, name, &Rights::parse(spec)?, None, false, false)?;
        access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or_else(|| "refused".into())
    };
    let args = json!({ "id": memo.id, "transcript": text.id });
    // A key that cannot read « work » does not find the text: nothing is erased.
    let blind = mk("blind", "personal:rw,health:r")?;
    let r = tools::call(&b, &blind, "file_erase", &args);
    assert!(
        matches!(&r, Err(ToolError::Invalid(m)) if m.contains("you can read")),
        "{r:?}"
    );
    assert!(exists(&b, &personal, &memo.sha256));
    let _ = health;
    // One that reads it erases.
    let worker = mk("worker", "personal:rw,work:r")?;
    assert_eq!(
        tools::call(&b, &worker, "file_erase", &args)?["result"],
        "erased"
    );
    assert!(!exists(&b, &personal, &memo.sha256));
    Ok(())
}

#[test]
fn an_unrelated_old_fact_is_not_a_transcript() -> Res {
    use memcp_core::facts::{self, NewFact, Remembered};
    let t = Temp::new("files");
    let b = brain(&t)?;
    let personal = sp("personal");
    let (memo, _) = memo_and_transcript(&b, &personal)?;
    let conn = b.open_space(&personal)?;
    let Remembered::Created(old) = facts::remember(
        &conn,
        &personal,
        "x",
        NewFact::agent("Une vieille note sans rapport.", "agent:x"),
        &[],
    )?
    else {
        return Err("fact".into());
    };
    conn.execute(
        "UPDATE facts SET created_at = 1700000000 WHERE id = ?1",
        [&old.id],
    )?;
    let now = memcp_core::util::now() + 30 * DAY;
    let r = files::erase_at(&b, &personal, "x", &memo.id, &personal, &old.id, now);
    assert!(
        matches!(&r, Err(e) if e.to_string().contains("name this recording")),
        "{r:?}"
    );
    // A text naming it but older than the recording is not its transcript either.
    conn.execute(
        "UPDATE facts SET text = 'audio Mémo vocal.m4a: vieux' WHERE id = ?1",
        [&old.id],
    )?;
    assert!(files::erase_at(&b, &personal, "x", &memo.id, &personal, &old.id, now).is_err());
    assert!(files::get(&b, &personal, &memo.id)?.is_some());
    Ok(())
}

#[test]
fn what_a_browser_records_is_a_recording() {
    let webm = [0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00];
    assert_eq!(
        sniff(&webm, "dictation-2026-11-16.webm"),
        Some("audio/webm")
    );
    assert_eq!(sniff(&webm, "voice note.webm"), Some("audio/webm"));
    // A video is a WebM too: only a name that says recording makes it one.
    assert_eq!(sniff(&webm, "holiday.webm"), None);
    // Safari records fragmented MP4 with another brand; the name says it is a recording.
    let mut mp4 = vec![
        0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'i', b's', b'o', b'5',
    ];
    mp4.extend_from_slice(&[0; 12]);
    assert_eq!(sniff(&mp4, "dictation.m4a"), Some("audio/mp4"));
    assert_eq!(sniff(&mp4, "movie.mp4"), None);
}
