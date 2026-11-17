//! The Swift interface as Swift will call it (the same functions, from Rust). The logic is tested in
//! `memcp-capture`; here, what the shell itself does: the names, the conversions, the errors.

// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

use memcp_capture_ffi::{Intake, IntakeConfig, IntakeError, ItemKind, ItemState, address_allowed};

fn dir(tag: &str) -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("memcp-ffi-{tag}-{}-{n}", std::process::id()))
}

fn config(outbox: &std::path::Path) -> IntakeConfig {
    // A port nobody listens on: the computer is offline.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    IntakeConfig {
        url: format!("http://127.0.0.1:{port}/mcp/bridge"),
        key: "mtm_key".into(),
        outbox: outbox.to_str().expect("path").into(),
        utc_offset_minutes: 60,
    }
}

#[test]
fn a_capture_is_kept_listed_and_forgotten_through_the_interface() {
    let outbox = dir("kept");
    let intake = Intake::new(config(&outbox)).expect("open");
    let note = intake
        .submit_text("Une idée pour le Mac.".into())
        .expect("note");
    let audio_path = outbox.join("memo.m4a");
    let mut m4a = vec![0, 0, 0, 0x20];
    m4a.extend_from_slice(b"ftypM4A \0\0\0\0");
    m4a.resize(4000, 7);
    std::fs::write(&audio_path, &m4a).expect("write");
    let audio = intake
        .submit_audio(
            audio_path.to_str().expect("p").into(),
            1_793_622_840,
            61,
            "Idée".into(),
            1,
            1,
            memcp_capture_ffi::HeardInput {
                text: String::new(),
                summary: String::new(),
                send_audio: true,
                send_text: true,
                send_summary: true,
            },
        )
        .expect("audio");
    assert_eq!(intake.waiting(), 2);
    let items = intake.items();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0].id.as_str(), items[0].kind, items[0].state),
        (note.as_str(), ItemKind::Text, ItemState::Queued)
    );
    assert_eq!(
        (items[1].id.as_str(), items[1].kind, items[1].bytes),
        (audio.as_str(), ItemKind::Audio, 4000)
    );
    // Offline: nothing goes, nothing is lost, and the flush says so.
    let r = intake.flush();
    assert!(r.unreachable && !r.refused);
    assert_eq!((r.sent, r.waiting, r.failed), (0, 2, 0));
    intake.forget(note);
    assert_eq!(intake.waiting(), 1);
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn errors_come_out_with_a_plain_message_and_the_right_kind() {
    let outbox = dir("errors");
    let intake = Intake::new(config(&outbox)).expect("open");
    assert!(matches!(
        intake.submit_text("  ".into()),
        Err(IntakeError::Invalid { .. })
    ));
    assert!(matches!(
        intake.submit_audio("/nowhere/x.m4a".into(), 0, 1, "x".into(), 1, 1, heard()),
        Err(IntakeError::Storage { .. })
    ));
    assert!(matches!(
        intake.submit_audio("/nowhere/x.m4a".into(), 0, 1, "x".into(), 2, 1, heard()),
        Err(IntakeError::Invalid { .. })
    ));
    let big = outbox.join("gros.txt");
    std::fs::write(&big, vec![b'a'; 4 * 1024 * 1024 + 1]).expect("write");
    let e = intake.submit_file(big.to_str().expect("p").into(), String::new());
    match e {
        Err(IntakeError::TooBig { message }) => assert!(message.contains("MB"), "{message}"),
        _ => panic!("too big"),
    }
    // An address that would send the key in clear is taken, and said, so that the app warns: never refused.
    let mut clear = config(&outbox);
    clear.url = "http://example.com/mcp/bridge".into();
    assert!(Intake::new(clear).expect("taken").in_clear());
    assert!(!intake.in_clear());
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn the_setup_screen_can_ask_whether_an_address_is_allowed() {
    assert!(address_allowed(
        "https://serveur.tailnet-name.ts.net/mcp/bridge".into()
    ));
    assert!(address_allowed("http://10.0.0.20:8791/mcp/bridge".into()));
    assert!(!address_allowed("http://example.com/mcp/bridge".into()));
    assert!(!address_allowed("pas une adresse".into()));
}

#[test]
fn each_connection_has_its_own_outbox_and_all_old_captures_move_into_it() {
    use memcp_capture_ffi::{adopt_old_captures, connection_outbox};
    let base = dir("base");
    let a = connection_outbox(
        base.display().to_string(),
        "https://m.example/mcp/mac".into(),
    );
    let b = connection_outbox(
        base.display().to_string(),
        "https://m.example/mcp/phone".into(),
    );
    assert_ne!(a, b);
    assert!(
        a.starts_with(&base.display().to_string()) && !a.contains("m.example"),
        "{a}"
    );
    assert_eq!(
        a,
        connection_outbox(
            base.display().to_string(),
            "https://m.example/mcp/mac/".into(),
        ),
        "the same connection, the same folder"
    );
    // Captures kept in the old shared folder, whatever vault they were made for: all of them move, once.
    let old = dir("old");
    let mut c = config(&old);
    let intake = Intake::new(c.clone()).expect("old");
    let first = intake.submit_text("Avant.".into()).expect("note");
    let second = intake.submit_text("Pour le travail.".into()).expect("note");
    drop(intake);
    // One made by a version that still named a vault: it goes to À trier too.
    let q = old.join("queue");
    let made_for_work = std::fs::read_dir(&q)
        .expect("queue")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains("Pour le travail")))
        .expect("the second capture");
    let t = std::fs::read_to_string(&made_for_work).expect("read");
    assert!(t.contains("\"texts\""), "{t}");
    std::fs::write(
        &made_for_work,
        t.replacen("\"texts\"", "\"vault\":\"work\",\"texts\"", 1),
    )
    .expect("write");
    assert_eq!(
        adopt_old_captures(old.display().to_string(), a.clone()).expect("moved"),
        2
    );
    assert_eq!(
        adopt_old_captures(old.display().to_string(), a.clone()).expect("again"),
        0
    );
    c.outbox = a.clone();
    let new = Intake::new(c).expect("new");
    let mut ids: Vec<String> = new.items().iter().map(|i| i.id.clone()).collect();
    ids.sort();
    let mut want = vec![first, second];
    want.sort();
    assert_eq!(ids, want);
    let _ = std::fs::remove_dir_all(base);
    let _ = std::fs::remove_dir_all(old);
}

fn heard() -> memcp_capture_ffi::HeardInput {
    memcp_capture_ffi::HeardInput {
        text: String::new(),
        summary: String::new(),
        send_audio: true,
        send_text: true,
        send_summary: true,
    }
}

#[test]
fn the_window_of_a_replica_is_read_and_written() {
    use memcp_capture_ffi::{prune_replica_window, replica_window, set_replica_window};
    use memcp_core::{Brain, Space, text::Language};

    let d = dir("window");
    let space = Space::new("notes").expect("space");
    Brain::init(&d, &[space], Language::English).expect("init");
    let path = d.to_str().expect("path").to_string();
    let w = replica_window(path.clone()).expect("read");
    assert_eq!((w.days, w.label.as_str()), (None, "window: all"));
    assert_eq!(
        prune_replica_window(path.clone(), 1_900_000_000).expect("prune"),
        0
    );
    set_replica_window(path.clone(), Some(21)).expect("set");
    let w = replica_window(path.clone()).expect("read");
    assert_eq!((w.days, w.label.as_str()), (Some(21), "window: 21 days"));
    assert!(set_replica_window(path.clone(), Some(0)).is_err());
    set_replica_window(path.clone(), None).expect("off");
    assert_eq!(replica_window(path).expect("read").days, None);
    let _ = std::fs::remove_dir_all(d);
}
