//! « À trier » lists what waits for a person, not the recordings whose text is already there.

mod common;

use common::{Res, Temp, brain};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::files;
use memcp_core::triage;
use memcp_core::users::OWNER;

fn m4a() -> Vec<u8> {
    let mut b = vec![
        0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ',
    ];
    b.extend_from_slice(&[0; 32]);
    b
}

#[test]
fn a_recording_with_its_text_does_not_wait_for_the_person_but_one_without_does() -> Res {
    let t = Temp::new("triage-audio");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let tags = vec!["inbox/audio".to_string()];
    files::put_tagged(
        &b,
        &space,
        "bridge",
        "dictation-1.m4a",
        "kept",
        &m4a(),
        &tags,
    )?;
    let mut bytes = m4a();
    bytes.push(1);
    files::put_tagged(
        &b,
        &space,
        "bridge",
        "dictation-2.m4a",
        "kept",
        &bytes,
        &tags,
    )?;
    // The text of the first is already here.
    let conn = b.open_space(&space)?;
    let mut n = NewFact::agent(
        "Dictation, audio dictation-1.m4a : call the plumber on Friday.",
        "agent:bridge",
    );
    n.tags = vec!["inbox".into()];
    assert!(matches!(
        facts::remember(&conn, &space, "bridge", n, &[])?,
        Remembered::Created(_)
    ));
    let w = triage::waiting(&b, &space, 50)?;
    let names: Vec<&str> = w["files"]
        .as_array()
        .ok_or("files")?
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["dictation-2.m4a"],
        "only the one nobody has read"
    );
    assert_eq!(w["transcribed"], 1);
    Ok(())
}

#[test]
fn a_recording_given_up_on_is_tagged_and_counted_apart_and_retried_when_the_tag_goes() -> Res {
    let t = Temp::new("triage-failed");
    let b = brain(&t)?;
    let space = triage::ensure(&b, OWNER, "owner")?;
    let tags = vec!["inbox/audio".to_string()];
    let rec = files::put_tagged(&b, &space, "bridge", "memo.m4a", "kept", &m4a(), &tags)?;
    files::put_tagged(
        &b,
        &space,
        "edge",
        "scan.txt",
        "a scan",
        b"plain words",
        &["inbox".to_string()],
    )?;
    let before = triage::waiting(&b, &space, 50)?;
    assert_eq!(
        before["count"], 2,
        "the recording waits until it is given up"
    );
    files::mark_transcription_failed(&b, &space, &rec.id, true)?;
    let w = triage::waiting(&b, &space, 50)?;
    assert_eq!(w["count"], 1, "only the document waits for a decision");
    assert_eq!(w["failed_transcription"].as_array().map(Vec::len), Some(1));
    assert_eq!(w["files"][0]["reason"], "not looked at by the janitor yet");
    assert!(w["files"][0]["age_seconds"].is_number());
    files::mark_transcription_failed(&b, &space, &rec.id, false)?;
    assert_eq!(triage::waiting(&b, &space, 50)?["count"], 2);
    Ok(())
}
