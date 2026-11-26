//! Findings of the core audit (docs/reviews/2026-11-17-core-audit.md), one test each, written before the fix.

mod common;

use base64::Engine as _;
use common::{Res, Temp, sp};
use pensine_core::sync::{self, Event};
use pensine_core::text::Language;
use pensine_core::{Brain, util};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn node(t: &Temp) -> Result<Brain, Box<dyn std::error::Error>> {
    Ok(Brain::init(
        &t.path().join("n"),
        &[sp("notes")],
        Language::English,
    )?)
}

fn hlc() -> String {
    format!("{:015}.00000.peer", util::now_ms())
}

fn created(id: &str, tweak: impl FnOnce(&mut Value)) -> Event {
    let mut payload = json!({
        "id": id, "kind": "context", "text": "a note", "source_kind": "agent",
        "source_agent": "peer", "author": "peer", "created_at": util::now(), "tags": [],
    });
    tweak(&mut payload);
    Event {
        seq: 0,
        eid: format!("c:{id}"),
        hlc: hlc(),
        origin: "peer".into(),
        kind: "fact_created".into(),
        at: util::now(),
        payload,
    }
}

fn applied(b: &Brain, ev: Event) -> Result<sync::Applied, Box<dyn std::error::Error>> {
    Ok(sync::apply(b, &sp("notes"), &[ev], "peer")?)
}

#[test]
fn a_peer_cannot_send_a_text_or_a_tag_list_without_limit() -> Res {
    let t = Temp::new("audit-limits");
    let b = node(&t)?;
    let r = applied(
        &b,
        created("big1", |p| p["text"] = json!("x".repeat(2_000_000))),
    )?;
    assert_eq!((r.applied, r.rejected.len()), (0, 1), "{r:?}");
    let many: Vec<String> = (0..200).map(|i| format!("t{i}")).collect();
    let r = applied(&b, created("tags1", |p| p["tags"] = json!(many)))?;
    assert_eq!((r.applied, r.rejected.len()), (0, 1), "{r:?}");
    let r = applied(&b, created("ok1", |_| {}))?;
    assert_eq!(r.applied, 1, "{r:?}");
    Ok(())
}

#[test]
fn a_peer_cannot_forge_an_origin_or_a_clock_stamp_with_odd_characters() -> Res {
    let t = Temp::new("audit-origin");
    let b = node(&t)?;
    let mut ev = created("o1", |_| {});
    ev.origin = "peer\r\nX-Evil: 1".into();
    assert_eq!(applied(&b, ev)?.rejected.len(), 1);
    let mut ev = created("o2", |_| {});
    ev.hlc = format!("{:015}.00000.a\r\nb", util::now_ms());
    assert_eq!(applied(&b, ev)?.rejected.len(), 1);
    let mut ev = created("o3", |p| p["author"] = json!("a".repeat(5000)));
    ev.origin = "peer".into();
    assert_eq!(applied(&b, ev)?.rejected.len(), 1);
    Ok(())
}

fn file_event(id: &str, name: &str, mime: &str, bytes: &[u8]) -> Event {
    let sha: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Event {
        seq: 0,
        eid: format!("f:{id}"),
        hlc: hlc(),
        origin: "peer".into(),
        kind: "file_put".into(),
        at: util::now(),
        payload: json!({
            "id": id, "sha256": sha, "size": bytes.len(), "name": name, "mime": mime, "author": "peer",
            "created_at": util::now(), "note": "",
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    }
}

#[test]
fn a_peer_cannot_store_a_file_under_a_hostile_name_or_type() -> Res {
    let t = Temp::new("audit-file");
    let b = node(&t)?;
    let s = sp("notes");
    let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
    // A name with a line break or a path is cleaned; the type is the one the bytes say, never the one claimed.
    let ev = file_event(
        "file_1",
        "../../a\r\nb.png",
        "text/html\r\nSet-Cookie: x",
        &png,
    );
    assert_eq!(applied(&b, ev)?.applied, 1);
    let f = pensine_core::files::info(&b, &s, "file_1")?.ok_or("no file")?;
    assert_eq!(f.name, "ab.png");
    assert_eq!(f.mime, "image/png");
    // Bytes of a kind that is not kept are refused, as they are when an assistant puts them.
    let ev = file_event(
        "file_2",
        "run.exe",
        "application/x-msdownload",
        b"MZ\x90\x00 binary",
    );
    assert_eq!(applied(&b, ev)?.rejected.len(), 1);
    Ok(())
}

#[test]
fn tags_are_bounded_in_number_and_a_search_in_terms() -> Res {
    let many: Vec<String> = (0..100).map(|i| format!("t{i}")).collect();
    assert!(pensine_core::facts::normalize_tags(&many).is_err());
    let ok: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
    assert_eq!(pensine_core::facts::normalize_tags(&ok)?.len(), 20);
    let words: String = (0..5000).map(|i| format!("word{i} ")).collect();
    assert!(pensine_core::facts::fts_terms(&words) <= 64);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_export_that_holds_a_link_is_not_verified() -> Res {
    use pensine_core::users::{self, OWNER};
    let t = Temp::new("audit-export-link");
    let brain = Brain::init(&t.path().join("data"), &[sp("legacy")], Language::English)?;
    let ctl = brain.open_control()?;
    let sam = users::create_user(&ctl, OWNER, "sam", "Sam", false)?;
    brain.add_vault(&sam.id, "sam", "work")?;
    let out = t.path().join("out");
    let root = pensine_core::export::export_person(&brain, &sam, &out, 1_900_000_000)?;
    pensine_core::export::verify_export(&root)?;
    // The same bytes, reached through a link: the fingerprint matches, the file is still not in the export.
    let readme = root.join("README.txt");
    let elsewhere = t.path().join("elsewhere.txt");
    std::fs::rename(&readme, &elsewhere)?;
    std::os::unix::fs::symlink(&elsewhere, &readme)?;
    assert!(pensine_core::export::verify_export(&root).is_err());
    Ok(())
}
