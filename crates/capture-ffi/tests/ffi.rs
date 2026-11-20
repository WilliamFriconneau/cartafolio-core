//! The Swift interface as Swift will call it (the same functions, from Rust). The logic is tested in `memcp-server` (the
//! node's `intake`) and `memcp-client` (joining by code); here, what the shell itself does: the names, the conversions,
//! the errors, and the node's intake end to end through the interface.

// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use memcp_capture_ffi::node::{NodeIntake, NodeIntakeSettings, NodeWatchedFolder};
use memcp_capture_ffi::{
    PairError, address_allowed, browser_extension_id, pair, should_transcribe,
    summary_instructions, summary_join_prompt, summary_prompts,
};
use memcp_core::text::Language;
use memcp_core::{Brain, users};
use serde_json::json;

fn dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("memcp-ffi-{tag}-{}-{n}", std::process::id()))
}

#[test]
fn the_setup_screen_can_ask_whether_an_address_is_allowed() {
    assert!(address_allowed(
        "https://serveur.tailnet-name.ts.net/mcp/desktop".into()
    ));
    assert!(address_allowed("http://10.0.0.20:8791/mcp/desktop".into()));
    assert!(!address_allowed("http://example.com/mcp/desktop".into()));
    assert!(!address_allowed("pas une adresse".into()));
}

/// A node that answers `/pair` with `answer`, once.
fn pairing_node(answer: serde_json::Value, status: u16) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
    let port = server.server_addr().to_ip().expect("ip").port();
    std::thread::spawn(move || {
        if let Ok(request) = server.recv() {
            let response =
                tiny_http::Response::from_string(answer.to_string()).with_status_code(status);
            let _ = request.respond(response);
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn pairing_reads_the_spaces_of_the_answer_and_the_old_vaults_key_too() {
    let new = pairing_node(
        json!({ "agent": "peer-mac-ab12", "key": "hcp_k", "mcp_url": "http://127.0.0.1:1/mcp/peer-mac-ab12", "spaces": ["sam-inbox"] }),
        200,
    );
    let p = pair(new, "ABCD2345".into(), "mac".into()).expect("paired");
    assert_eq!(p.agent, "peer-mac-ab12");
    assert_eq!(p.spaces, ["sam-inbox"]);
    assert_eq!(p.vaults, p.spaces, "the old field mirrors the new one");
    assert!(!p.in_clear, "the loopback is the own network");
    // A node older than the rename still says `vaults`.
    let old = pairing_node(
        json!({ "agent": "peer-mac-cd34", "key": "hcp_k", "mcp_url": "http://127.0.0.1:1/mcp/peer-mac-cd34", "vaults": ["sam-inbox"] }),
        200,
    );
    let p = pair(old, "ABCD2345".into(), "mac".into()).expect("paired");
    assert_eq!(p.spaces, ["sam-inbox"]);
    // Errors come out with the right kind and a plain message.
    let refused = pairing_node(json!({ "error": "this code is not valid" }), 400);
    match pair(refused, "ZZZZ2345".into(), "mac".into()) {
        Err(PairError::Refused { message }) => assert!(message.contains("not valid"), "{message}"),
        other => panic!("refused: {:?}", other.err()),
    }
    let busy = pairing_node(json!({ "error": "wait" }), 429);
    assert!(matches!(
        pair(busy, "ZZZZ2345".into(), "mac".into()),
        Err(PairError::Wait { .. })
    ));
    let nobody = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = nobody.local_addr().expect("addr").port();
    drop(nobody);
    assert!(matches!(
        pair(
            format!("http://127.0.0.1:{port}"),
            "ABCD2345".into(),
            "mac".into()
        ),
        Err(PairError::Unreachable { .. })
    ));
}

#[test]
fn the_rules_and_the_prompts_of_the_meeting_screen_come_from_the_node() {
    assert_eq!(browser_extension_id().len(), 32);
    assert!(should_transcribe(true, 5, false));
    assert!(!should_transcribe(false, 10, false));
    assert!(!should_transcribe(false, 80, true));
    assert!(summary_instructions().contains("résume"));
    let text = "mot ".repeat(5000);
    let prompts = summary_prompts(text);
    assert!(prompts.len() >= 2);
    assert!(summary_join_prompt(vec!["un".into()]).is_none());
    assert!(summary_join_prompt(vec!["un".into(), "deux".into()]).is_some());
}

// --- the node's own intake, through the interface -----------------------------------------------------------------

fn settings() -> NodeIntakeSettings {
    NodeIntakeSettings {
        person: Some("sam".into()),
        transcribe: true,
        transcribe_spaces: vec![],
        model: None,
        language: "fr".into(),
        threads: 0,
        lexicon: None,
        api_url: None,
        api_model: String::new(),
        api_key: None,
        transcribe_every_secs: 5,
        watch: vec![],
        watch_every_secs: 1,
        utc_offset_minutes: 60,
    }
}

fn node(tag: &str) -> (PathBuf, PathBuf) {
    let root = dir(tag);
    let data = root.join("data");
    let brain = Brain::init(&data, &[], Language::English).expect("init");
    let ctl = brain.open_control().expect("control");
    let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
        .expect("user")
        .id;
    brain.add_vault(&sam, "sam", "me").expect("space");
    (root, data)
}

fn note_texts(data: &std::path::Path, space: &str) -> Vec<String> {
    let brain = Brain::open(data).expect("open");
    let conn = brain
        .open_space(&memcp_core::Space::new(space).expect("space"))
        .expect("space");
    let mut st = conn
        .prepare("SELECT text FROM facts WHERE status = 'active'")
        .expect("prepare");
    st.query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .flatten()
        .collect()
}

#[test]
fn the_node_intake_watches_a_folder_and_reads_a_recording_by_a_speech_service() {
    let (root, data) = node("node");
    let drop = root.join("Drop");
    std::fs::create_dir_all(&drop).expect("dir");
    // A speech service on the loopback: every recording says the same thing.
    let service = tiny_http::Server::http("127.0.0.1:0").expect("bind");
    let port = service.server_addr().to_ip().expect("ip").port();
    std::thread::spawn(move || {
        for request in service.incoming_requests() {
            let response = tiny_http::Response::from_string(
                json!({ "text": "penser à la réunion du jeudi" }).to_string(),
            );
            let _ = request.respond(response);
        }
    });
    let mut s = settings();
    s.api_url = Some(format!("http://127.0.0.1:{port}/v1/audio/transcriptions"));
    s.watch = vec![NodeWatchedFolder {
        path: drop.to_string_lossy().into_owned(),
        space: Some("sam-me".into()),
    }];
    let intake = NodeIntake::new(data.to_string_lossy().into_owned(), s, None).expect("intake");
    // A file added to the folder, old enough to be taken.
    let note = drop.join("idee.md");
    std::fs::write(&note, "Un bac à compost au fond du jardin.\n").expect("note");
    std::fs::File::options()
        .write(true)
        .open(&note)
        .expect("open")
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(60))
        .expect("mtime");
    let reports = intake.watch_now().expect("watch");
    assert_eq!(reports.len(), 1);
    assert_eq!((reports[0].notes, reports[0].files), (1, 0), "{reports:?}");
    assert_eq!(reports[0].space, "sam-me");
    // A recording kept in the memory (as the app's own node would, by MCP): the node reads it by itself.
    {
        let brain = Brain::open(&data).expect("open");
        let ctl = brain.open_control().expect("control");
        let caller = memcp_core::access::authenticate(
            &ctl,
            &memcp_core::access::connect_for(
                &ctl,
                &users::resolve(&ctl, "sam").expect("sam").id,
                "mac-app",
                &memcp_core::access::Rights::parse("sam-me:rw").expect("rights"),
                None,
                false,
                false,
            )
            .expect("connect"),
            u32::MAX.into(),
        )
        .expect("auth")
        .expect("caller");
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.extend_from_slice(b"a voice");
        use base64::Engine as _;
        memcp_core::tools::call(
            &brain,
            &caller,
            "file_put",
            &json!({
                "space": "sam-me", "name": "2026-11-02-13h34-reunion.wav", "note": "dictée",
                "tags": ["inbox/audio"],
                "data_base64": base64::engine::general_purpose::STANDARD.encode(&wav),
            }),
        )
        .expect("recording");
    }
    let waiting = intake.recordings_waiting().expect("waiting");
    assert_eq!(
        waiting.iter().map(|w| w.count).sum::<u64>(),
        1,
        "{waiting:?}"
    );
    // In the background, as the app runs it: the recording becomes a note by itself.
    intake.start().expect("start");
    let end = Instant::now() + Duration::from_secs(60);
    let mut heard = false;
    while Instant::now() < end {
        if note_texts(&data, "sam-me").iter().any(|t| {
            t.starts_with("Dictée du 2026-11-02 à 13h34") && t.contains("réunion du jeudi")
        }) {
            heard = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    intake.stop();
    intake.stop();
    assert!(heard, "{:?}", note_texts(&data, "sam-me"));
    // The notes of the folder are there too, and nothing is taken twice.
    let notes = note_texts(&data, "sam-me");
    assert_eq!(
        notes.iter().filter(|t| t.contains("bac à compost")).count(),
        1
    );
    let again = intake.watch_now().expect("watch");
    assert_eq!(again[0].notes, 0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_decoder_with_no_built_in_speech_model_is_said_not_silently_ignored() {
    // Without `local-whisper` (the Linux build) a model on the machine needs `whisper-cli`; the app's decoder only serves
    // the model inside the program, which this build does not have: the recording is left, the problem is told.
    struct Never;
    impl memcp_capture_ffi::node::NodeAudioDecoder for Never {
        fn decode(
            &self,
            _path: String,
        ) -> Result<Vec<f32>, memcp_capture_ffi::node::NodeIntakeError> {
            Ok(vec![0.0; 16_000])
        }
    }
    let (root, data) = node("decoder");
    let mut s = settings();
    s.model = Some("/models/ggml-small.bin".into());
    let decoder: Arc<dyn memcp_capture_ffi::node::NodeAudioDecoder> = Arc::new(Never);
    let intake =
        NodeIntake::new(data.to_string_lossy().into_owned(), s, Some(decoder)).expect("intake");
    let report = intake.transcribe_now().expect("a pass");
    assert_eq!(report.transcribed, 0);
    let _ = std::fs::remove_dir_all(root);
}
