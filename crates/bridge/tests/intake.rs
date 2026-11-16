//! The outbox and the gateway against the real engine, with the network coming and going: a port nobody listens on
//! first (the computer is offline), then the memory started on it.
#![cfg(unix)]
// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

mod common;

use common::{m4a, memory, unique, write};
use memcp_bridge::{Error, Gateway, Heard, Intake, Kind, State};
use serde_json::{Value, json};

#[test]
fn captures_wait_offline_survive_a_restart_and_go_when_the_network_is_back() {
    let mem = memory("offline", &["sam-me/inbox:rw", "sam-me:rw"]);
    let work = unique("files");
    let outbox = unique("outbox");
    let intake = Intake::open(mem.config(0, None, &outbox)).expect("open");
    let note = intake
        .submit_text("Une idée jetée vite, hors réseau.")
        .expect("note");
    let doc = write(&work, "devis.txt", b"un devis en texte");
    intake.submit_file(&doc, "devis du plombier").expect("doc");
    let audio = write(&work, "memo.m4a", &m4a(2 * 1024 * 1024));
    intake
        .submit_audio(&audio, 1_793_622_840, 95, "Réunion du lundi", 2, 3)
        .expect("audio");
    // The originals may go at once: the outbox holds copies.
    std::fs::remove_dir_all(&work).expect("remove originals");

    let offline = intake.flush();
    assert!(offline.unreachable && !offline.refused);
    assert_eq!((offline.sent, offline.waiting), (0, 3));

    // The program is closed and opened again: nothing was lost.
    drop(intake);
    let intake = Intake::open(mem.config(0, None, &outbox)).expect("reopen");
    let items = intake.items();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i.state == State::Queued));
    assert_eq!(
        items.iter().map(|i| i.kind).collect::<Vec<_>>(),
        [Kind::Text, Kind::Document, Kind::Audio]
    );
    assert!(items[0].id == note);

    mem.up();
    let sent = intake.flush();
    assert_eq!(
        (sent.sent, sent.waiting, sent.failed),
        (3, 0, 0),
        "{sent:?}"
    );
    assert!(!sent.unreachable && !sent.refused);
    let after = intake.items();
    assert_eq!(after.len(), 3);
    assert!(after.iter().all(|i| i.state == State::Sent));
    assert!(
        std::fs::read_dir(outbox.join("queue"))
            .expect("queue")
            .next()
            .is_none(),
        "the queue is empty"
    );

    // The memory holds them: the note dated in the user's day, never rewritten; the audio named by its start.
    let inbox = mem.texts("inbox", &["idée", "Réunion", "devis"]);
    assert!(
        inbox
            .iter()
            .any(|t| t.starts_with("2026-") && t.ends_with("Une idée jetée vite, hors réseau."))
    );
    assert!(
        inbox
            .iter()
            .any(|t| t.contains("2026-11-02-13h34-Réunion du lundi-part-2-of-3.m4a")),
        "{inbox:?}"
    );
    assert!(inbox.iter().any(|t| t.contains("devis.txt")));
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn a_flush_twice_sends_nothing_twice_and_a_cut_before_the_answer_keeps_it_once() {
    let mem = memory("twice", &["sam-me/inbox:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let intake = Intake::open(mem.config(0, None, &outbox)).expect("open");
    intake.submit_text("Une seule fois.").expect("note");
    assert_eq!(intake.flush().sent, 1);
    assert_eq!(intake.flush().sent, 0);
    // The same capture put in the queue again (the answer never came back): the memory keeps it once.
    let id = intake
        .queue_call(
            "remember",
            &json!({ "space": "sam-me", "text": "Un appel repris.", "tags": ["inbox"] }),
            None,
        )
        .expect("call");
    assert_eq!(intake.flush().sent, 1);
    intake.forget(&id).expect("forget");
    intake
        .queue_call(
            "remember",
            &json!({ "space": "sam-me", "text": "Un appel repris.", "tags": ["inbox"] }),
            None,
        )
        .expect("again");
    assert_eq!(intake.flush().sent, 1);
    let held: Vec<String> = mem
        .texts("inbox", &["appel"])
        .into_iter()
        .filter(|t| t.contains("Un appel repris"))
        .collect();
    assert_eq!(held.len(), 1, "{held:?}");
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn a_refusal_of_the_memory_fails_that_item_and_the_next_still_goes() {
    let mem = memory("refusal", &["sam-me/inbox:rw", "sam-me:rw"]);
    mem.up();
    let work = unique("files");
    let outbox = unique("outbox");
    let intake = Intake::open(mem.config(0, None, &outbox)).expect("open");
    // Bytes the memory does not keep (not text, not a known type): refused in its own words.
    let junk = write(&work, "binaire.dat", &[0xFF, 0x00, 0xFE, 0x01, 0x02, 0x03]);
    let bad = intake.submit_file(&junk, "").expect("queued");
    intake.submit_text("Celle-ci passe.").expect("note");
    let r = intake.flush();
    assert_eq!((r.sent, r.failed, r.waiting), (1, 1, 0), "{r:?}");
    let failed: Vec<_> = intake
        .items()
        .into_iter()
        .filter(|i| i.state == State::Failed)
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].id, bad);
    assert!(
        failed[0].detail.contains("only images"),
        "{}",
        failed[0].detail
    );
    // It does not come back at the next flush, which counts only its own refusals.
    assert_eq!(intake.flush().failed, 0);
    assert!(
        intake
            .items()
            .iter()
            .any(|i| i.id == bad && i.state == State::Failed)
    );
    // Retried, it is refused again (nothing changed); then the person forgets it.
    assert_eq!(intake.retry(Some(&bad)).expect("retry"), 1);
    assert_eq!(intake.flush().failed, 1);
    assert!(
        intake.forget("../../etc/passwd").is_err(),
        "an id is never a path"
    );
    intake.forget(&bad).expect("forget");
    assert!(intake.items().iter().all(|i| i.state != State::Failed));
    let _ = std::fs::remove_dir_all((outbox, work).0);
}

#[test]
fn a_refused_key_stops_everything_and_keeps_what_waits() {
    let mem = memory("refused", &["sam-me/inbox:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let intake = Intake::open(mem.config(0, Some("mtm_wrong"), &outbox)).expect("open");
    intake.submit_text("Une.").expect("one");
    intake.submit_text("Deux.").expect("two");
    let r = intake.flush();
    assert!(r.refused && !r.unreachable);
    assert_eq!((r.sent, r.waiting), (0, 2));
    assert_eq!(intake.waiting(), 2);
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn what_the_memory_would_refuse_is_refused_before_it_is_kept() {
    let mem = memory("toobig", &["sam-me/inbox:rw", "sam-me:rw"]);
    let work = unique("files");
    let outbox = unique("outbox");
    let intake = Intake::open(mem.config(0, None, &outbox)).expect("open");
    assert!(matches!(intake.submit_text("   "), Err(Error::Invalid(_))));
    let long = write(&work, "long.m4a", &m4a(24 * 1024 * 1024 + 1));
    assert!(matches!(
        intake.submit_audio(&long, 0, 1, "x", 1, 1),
        Err(Error::TooBig(_))
    ));
    assert!(matches!(
        intake.submit_audio(&long, 0, 1, "x", 3, 2),
        Err(Error::Invalid(_))
    ));
    let doc = write(&work, "gros.txt", &vec![b'a'; 4 * 1024 * 1024 + 1]);
    assert!(matches!(
        intake.submit_file(&doc, ""),
        Err(Error::TooBig(_))
    ));
    let empty = write(&work, "vide.txt", b"");
    assert!(matches!(
        intake.submit_file(&empty, ""),
        Err(Error::Invalid(_))
    ));
    assert!(intake.items().is_empty(), "nothing was kept");
    // A text longer than a note is kept as several notes, in order, and none is lost.
    let words = (0..2500)
        .map(|i| format!("mot{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    intake.submit_text(&words).expect("long text");
    assert_eq!(intake.items().len(), 1, "one capture");
    mem.up();
    assert_eq!(intake.flush().sent, 1);
    let held = mem.texts("inbox", &["mot0", "mot2499"]);
    assert_eq!(held.len(), 2, "cut in two notes");
    assert!(held.iter().all(|t| t.chars().count() <= 4000));
    let joined = held
        .iter()
        .map(|t| t.rsplit_once(", ").map_or(t.as_str(), |x| x.1))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("mot0") && joined.ends_with("mot2499"),
        "all of it is there"
    );
    let _ = std::fs::remove_dir_all((outbox, work).0);
}

fn send(g: &mut Gateway, message: &Value) -> Value {
    g.handle(message).expect("an answer")
}

fn call(id: u64, tool: &str, args: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}})
}

fn text_of(reply: &Value) -> Value {
    let t = reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("{}");
    serde_json::from_str(t).unwrap_or(Value::String(t.to_string()))
}

#[test]
fn the_gateway_passes_the_memory_through_and_remembers_what_it_said() {
    let mem = memory("gate-online", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let mut g = Gateway::new(mem.config(0, None, &outbox)).expect("gateway");
    let init = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    );
    let instr = init["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(
        instr.contains("memory."),
        "the memory's own instructions come through"
    );
    assert!(
        instr.contains("kept here and sent when it is back"),
        "and what the bridge adds"
    );
    assert!(
        g.handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none()
    );
    let tools = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"recall") && names.contains(&"deposit"));
    assert!(
        outbox.join("initialize.json").exists() && outbox.join("tools.json").exists(),
        "kept for the next time offline"
    );
    let made = send(
        &mut g,
        &call(
            3,
            "remember",
            &json!({"space":"sam-me","text":"Écrite en ligne.","tags":["inbox"]}),
        ),
    );
    assert_eq!(
        text_of(&made)["result"],
        "created",
        "online, the real answer comes back with its id"
    );
    assert!(text_of(&made)["fact"]["id"].is_string());
    assert_eq!(
        send(&mut g, &json!({"jsonrpc":"2.0","id":4,"method":"ping"}))["result"],
        json!({})
    );
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn the_gateway_offline_starts_keeps_the_writes_without_a_decision_and_says_what_it_cannot_do() {
    let mem = memory("gate-offline", &["sam-me:rw", "sam-me:rw"]);
    let outbox = unique("outbox");
    std::fs::create_dir_all(&outbox).expect("outbox");
    // What the memory said the last time, as a program that already ran would have kept it.
    std::fs::write(outbox.join("initialize.json"), json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"memcp"},"instructions":"Les consignes de Sam."}).to_string()).expect("cache");
    let mut g = Gateway::new(mem.config(0, None, &outbox)).expect("gateway");

    let init = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    );
    let instr = init["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(
        instr.starts_with("Les consignes de Sam."),
        "the last instructions, offline"
    );
    assert!(instr.contains("kept here"));
    let names = |r: &Value| -> Vec<String> {
        r["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect()
    };
    let listed = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    assert_eq!(
        names(&listed),
        [
            "remember",
            "deposit",
            "file_put",
            "capture_text",
            "outbox_status",
            "outbox_flush"
        ],
        "never seen the memory's list: the three that can wait, then the bridge's own (no capture_file without a folder)"
    );

    let q = send(
        &mut g,
        &call(
            3,
            "remember",
            &json!({"space":"sam-me","text":"Gardée hors ligne.","tags":["inbox"]}),
        ),
    );
    assert_eq!(text_of(&q)["result"], "queued");
    assert_eq!(text_of(&q)["queued"], true);
    assert!(
        text_of(&q).get("fact").is_none(),
        "no fact id: nothing is written yet"
    );
    assert_eq!(q["result"]["isError"], false);
    let dep = send(
        &mut g,
        &call(
            4,
            "deposit",
            &json!({"space":"sam-me","records":[{"text":"[Notes] n1","origin":{"app":"notes","id":"n/1"}}]}),
        ),
    );
    assert_eq!(text_of(&dep)["result"], "queued");
    let bytes = m4a(3000);
    use base64::Engine as _;
    let file = send(
        &mut g,
        &call(
            5,
            "file_put",
            &json!({"space":"sam-me","name":"memo.m4a","note":"dictée","tags":["inbox/audio"],"data_base64":base64::engine::general_purpose::STANDARD.encode(&bytes)}),
        ),
    );
    assert_eq!(text_of(&file)["result"], "queued");
    assert!(
        g.intake().items().iter().any(|i| i.bytes == 3000),
        "the bytes are kept as they are"
    );
    // What needs a decision, an order or an answer says so, and is not kept.
    for (tool, args) in [
        ("recall", json!({"query":"x"})),
        ("supersede", json!({"id":"f_1","text":"y"})),
        (
            "remember",
            json!({"space":"sam-me","text":"sans étiquette"}),
        ),
    ] {
        let r = send(&mut g, &call(6, tool, &args));
        assert_eq!(r["result"]["isError"], true, "{tool}");
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("needs it"),
            "{tool}"
        );
    }
    assert_eq!(g.intake().waiting(), 3);

    // The network is back: what was kept goes, and it is in the memory.
    mem.up();
    let report = g.intake().flush();
    assert_eq!(
        (report.sent, report.failed, report.waiting),
        (3, 0, 0),
        "{report:?}"
    );
    assert!(
        mem.texts("inbox", &["Gardée"])
            .iter()
            .any(|t| t == "Gardée hors ligne.")
    );
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn a_refused_key_is_told_to_the_assistant_and_nothing_is_kept() {
    let mem = memory("gate-refused", &["sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let mut g = Gateway::new(mem.config(0, Some("mtm_wrong"), &outbox)).expect("gateway");
    let r = send(
        &mut g,
        &call(
            1,
            "remember",
            &json!({"space":"sam-me","text":"x","tags":["inbox"]}),
        ),
    );
    assert_eq!(r["result"]["isError"], true);
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("refused")
    );
    let init = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}),
    );
    assert_eq!(init["error"]["code"], -32001);
    assert_eq!(g.intake().waiting(), 0);
    let _ = std::fs::remove_dir_all(outbox);
}

fn local(g: &mut Gateway, id: u64, tool: &str, args: &Value) -> (bool, Value) {
    let r = send(g, &call(id, tool, args));
    let is_error = r["result"]["isError"] == true;
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or("");
    (
        is_error,
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())),
    )
}

#[test]
fn the_bridges_own_tools_capture_a_file_by_its_path_so_its_bytes_never_cross_the_assistant() {
    let mem = memory("local-tools", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let work = unique("allowed");
    let outbox = unique("outbox");
    let doc = write(&work, "devis-plombier.txt", b"un devis en texte");
    let mut g = Gateway::new(mem.config(0, None, &outbox))
        .expect("gateway")
        .with_capture_dirs(std::slice::from_ref(&work));

    let tools = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for want in [
        "recall",
        "capture_text",
        "capture_file",
        "outbox_status",
        "outbox_flush",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    // Online, a capture goes at once and says so.
    let (err, note) = local(
        &mut g,
        2,
        "capture_text",
        &json!({"text": "Une idée pour le Mac."}),
    );
    assert!(!err);
    assert_eq!(note["result"], "sent", "{note}");
    let (err, file) = local(
        &mut g,
        3,
        "capture_file",
        &json!({"path": doc.to_str().expect("p"), "note": "devis du plombier"}),
    );
    assert!(!err && file["result"] == "sent", "{file}");
    assert!(
        mem.texts("inbox", &["plombier"])
            .iter()
            .any(|t| t.contains("devis-plombier.txt"))
    );
    assert!(
        mem.texts("inbox", &["idée"])
            .iter()
            .any(|t| t.ends_with("Une idée pour le Mac."))
    );

    // What it will not read: outside the folders, hidden, a link that leads out, not a full path, nothing there.
    let secret = write(&unique("elsewhere"), "secret.txt", b"not to be sent");
    let hidden = write(&work.join(".ssh"), "id_rsa", b"private");
    std::os::unix::fs::symlink(&secret, work.join("lien.txt")).expect("link");
    for (path, why) in [
        (secret.to_str().expect("p").to_string(), "outside"),
        (hidden.to_str().expect("p").to_string(), "visible"),
        (
            work.join("lien.txt").to_str().expect("p").to_string(),
            "outside",
        ),
        ("devis.txt".to_string(), "full path"),
        (
            work.join("absent.txt").to_str().expect("p").to_string(),
            "no such file",
        ),
        (format!("{}/../elsewhere", work.display()), "no such"),
    ] {
        let (err, msg) = local(&mut g, 4, "capture_file", &json!({ "path": path }));
        assert!(err, "{path}: {msg}");
        assert!(msg.as_str().expect("text").contains(why), "{path}: {msg}");
    }
    // A file only its owner may read (a key, a settings file), and one that holds a key: never.
    use std::os::unix::fs::PermissionsExt;
    let private = write(&work, "reglages.conf", b"url = x");
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let keyfile = write(&work, "cle.txt", b"mtm_0123_abcdef");
    std::fs::set_permissions(&keyfile, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    for (path, why) in [(&private, "private"), (&keyfile, "holds a key")] {
        let (err, msg) = local(
            &mut g,
            6,
            "capture_file",
            &json!({ "path": path.to_str().expect("p") }),
        );
        assert!(err && msg.as_str().expect("text").contains(why), "{msg}");
    }
    let big = write(&work, "gros.txt", &vec![b'a'; 4 * 1024 * 1024 + 1]);
    let (err, msg) = local(
        &mut g,
        5,
        "capture_file",
        &json!({"path": big.to_str().expect("p")}),
    );
    assert!(
        err && msg.as_str().expect("text").contains("too big"),
        "{msg}"
    );
    assert_eq!(g.intake().waiting(), 0, "nothing refused was kept");
    let _ = std::fs::remove_dir_all((outbox, work).0);
}

#[test]
fn without_a_named_folder_capture_file_is_not_even_offered() {
    let mem = memory("no-roots", &["sam-me:rw", "sam-me:rw"]);
    mem.up();
    let outbox = unique("outbox");
    let work = unique("files");
    let doc = write(&work, "a.txt", b"x");
    let mut g = Gateway::new(mem.config(0, None, &outbox)).expect("gateway");
    let tools = send(
        &mut g,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"capture_text") && !names.contains(&"capture_file"),
        "{names:?}"
    );
    let (err, msg) = local(
        &mut g,
        2,
        "capture_file",
        &json!({"path": doc.to_str().expect("p")}),
    );
    assert!(
        err && msg.as_str().expect("text").contains("--capture-dir"),
        "{msg}"
    );
    let _ = std::fs::remove_dir_all((outbox, work).0);
}

#[test]
fn offline_the_bridges_own_tools_keep_and_the_status_and_flush_tools_tell_and_send() {
    let mem = memory("local-offline", &["sam-me:rw", "sam-me:rw"]);
    let work = unique("files");
    let outbox = unique("outbox");
    let mp3 = write(&work, "reunion.m4a", &m4a(3000));
    let mut g = Gateway::new(mem.config(0, None, &outbox))
        .expect("gateway")
        .with_capture_dirs(std::slice::from_ref(&work));
    let (err, a) = local(
        &mut g,
        1,
        "capture_text",
        &json!({"text": "Notée hors réseau."}),
    );
    assert!(!err && a["result"] == "queued", "{a}");
    assert!(
        a["message"]
            .as_str()
            .expect("m")
            .contains("Nothing is written")
    );
    let (err, b) = local(
        &mut g,
        2,
        "capture_file",
        &json!({"path": mp3.to_str().expect("p"), "note": "réunion"}),
    );
    assert!(!err && b["result"] == "queued", "{b}");
    let (_, status) = local(&mut g, 3, "outbox_status", &json!({}));
    assert_eq!(status["waiting"], 2);
    assert_eq!(status["items"][1]["kind"], "audio");
    assert_eq!(status["items"][0]["state"], "queued");
    mem.up();
    let (_, flushed) = local(&mut g, 4, "outbox_flush", &json!({}));
    assert_eq!(
        (flushed["sent"].clone(), flushed["waiting"].clone()),
        (json!(2), json!(0)),
        "{flushed}"
    );
    let (_, after) = local(&mut g, 5, "outbox_status", &json!({}));
    assert_eq!(after["waiting"], 0);
    assert_eq!(after["items"][0]["state"], "sent");
    assert!(
        mem.texts("inbox", &["réseau"])
            .iter()
            .any(|t| t.ends_with("Notée hors réseau."))
    );
    let _ = std::fs::remove_dir_all((outbox, work).0);
}

#[test]
fn an_answer_the_network_garbled_keeps_everything_queued() {
    // A captive portal: an HTML page with a 200, not the memory.
    let server = tiny_http::Server::http("127.0.0.1:0").expect("listen");
    let port = server.server_addr().to_ip().expect("ip").port();
    std::thread::spawn(move || {
        for rq in server.incoming_requests() {
            let _ = rq.respond(tiny_http::Response::from_string(
                "<html>Log in to the hotel Wi-Fi</html>",
            ));
        }
    });
    let outbox = unique("outbox");
    let intake = Intake::open(memcp_bridge::Config {
        url: format!("http://127.0.0.1:{port}/mcp/x"),
        key: "mtm_x".into(),
        outbox: outbox.clone(),
        utc_offset_minutes: 60,
    })
    .expect("open");
    intake.submit_text("Une note à l'hôtel.").expect("note");
    for _ in 0..8 {
        let r = intake.flush_quick();
        assert!(r.unreachable && r.failed == 0, "{r:?}");
    }
    assert!(intake.items().iter().all(|i| i.state == State::Queued));
    let _ = std::fs::remove_dir_all(outbox);
}

#[test]
fn a_recording_heard_here_goes_with_its_text_and_stays_to_be_listened_to() {
    let mem = memory("heard", &["sam-me/inbox:rw", "sam-me:rw"]);
    let work = unique("heard-files");
    let outbox = unique("heard-outbox");
    let intake = Intake::open(mem.config(0, None, &outbox)).expect("open");
    let audio = write(&work, "memo.m4a", &m4a(64 * 1024));
    let id = intake
        .submit_audio_heard(
            &audio,
            1_793_622_840,
            95,
            "Point du jeudi",
            1,
            1,
            &Heard {
                text: "On décale la réunion à vendredi.".into(),
                summary: "Réunion décalée à vendredi.".into(),
                send_audio: true,
                send_text: true,
                send_summary: true,
            },
        )
        .expect("audio");
    mem.up();
    let sent = intake.flush();
    assert_eq!(
        (sent.sent, sent.failed),
        (1, 0),
        "{sent:?} {:?}",
        intake.items()
    );
    let item = intake
        .items()
        .into_iter()
        .find(|i| i.id == id)
        .expect("item");
    assert_eq!(item.state, State::Sent);
    assert_eq!(item.heard, "On décale la réunion à vendredi.");
    assert_eq!(item.summary, "Réunion décalée à vendredi.");
    assert!(
        item.audio.ends_with(".m4a") && std::path::Path::new(&item.audio).exists(),
        "{item:?}"
    );
    // The note names the file and its id: the memory's side does not transcribe it again, and erases it a week after
    // its text was filed.
    let inbox = mem.texts("inbox", &["vendredi"]);
    assert!(
        inbox.iter().any(|t| t.starts_with(
            "Dictée du 2026-11-02 à 13h34, 95 s, audio 2026-11-02-13h34-Point du jeudi.m4a (file_"
        ) && t.ends_with(") : On décale la réunion à vendredi.")),
        "{inbox:?}"
    );
    let summaries = mem.texts("inbox", &["décalée"]);
    assert!(
        summaries
            .iter()
            .any(|t| t
                .starts_with("Résumé de l'enregistrement du 2026-11-02 à 13h34 : Réunion décalée")),
        "{summaries:?}"
    );
    // Without its sound, a recording goes as its notes only, and the memory's side has nothing to transcribe.
    let quiet = intake
        .submit_audio_heard(
            &audio,
            1_793_622_900,
            30,
            "Sans le son",
            1,
            1,
            &Heard {
                text: "Le texte seul part.".into(),
                send_text: true,
                ..Heard::default()
            },
        )
        .expect("notes only");
    assert_eq!(intake.flush().failed, 0);
    let q = intake
        .items()
        .into_iter()
        .find(|i| i.id == quiet)
        .expect("quiet");
    assert!(q.state == State::Sent && q.audio.is_empty(), "{q:?}");
    assert!(
        mem.texts("inbox", &["seul"])
            .iter()
            .any(|t| t.ends_with("Le texte seul part."))
    );
    // Forgotten, its recording goes too.
    let kept = item.audio.clone();
    intake.forget(&id).expect("forget");
    assert!(!std::path::Path::new(&kept).exists());
    let _ = std::fs::remove_dir_all(outbox);
    let _ = std::fs::remove_dir_all(work);
}
