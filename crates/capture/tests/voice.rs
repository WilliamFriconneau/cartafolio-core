//! The voice intake against the real memory, with two small scripts standing for `ffmpeg` and `whisper-cli`: the audio of
//! a test *is* the text it says.
#![cfg(unix)]
// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{memory, unique};
use memcp_capture::{VoiceSettings, transcribe};
use serde_json::json;

const FFMPEG: &str = r#"#!/bin/sh
in=""; prev=""; last=""
for a in "$@"; do [ "$prev" = "-i" ] && in="$a"; [ "$a" = "volumedetect" ] && vol=1; prev="$a"; last="$a"; done
if [ -n "$vol" ]; then
  if grep -q SILENCE "$in"; then echo "max_volume: -91.0 dB" >&2; else echo "max_volume: -12.0 dB" >&2; fi
  exit 0
fi
if grep -q BROKEN "$in"; then echo "not an audio" >&2; exit 1; fi
cp "$in" "$last"
"#;

const WHISPER: &str = r#"#!/bin/sh
wav=""; base=""; prev=""
for a in "$@"; do [ "$prev" = "-f" ] && wav="$a"; [ "$prev" = "-of" ] && base="$a"; prev="$a"; done
tail -c +13 "$wav" > "$base.txt"
"#;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).expect("script");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    p
}

fn audio(text: &str) -> Vec<u8> {
    let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
    b.extend_from_slice(text.as_bytes());
    b
}

#[test]
fn recordings_with_no_text_are_read_here_and_the_rest_left_alone() {
    let mem = memory("voice", &["sam-me/inbox:rw", "sam-me:rw"]);
    mem.up();
    let dir = unique("voice");
    std::fs::create_dir_all(&dir).expect("dir");
    let settings = VoiceSettings {
        whisper_cmd: script(&dir, "whisper-cli", WHISPER).display().to_string(),
        ffmpeg_cmd: script(&dir, "ffmpeg", FFMPEG).display().to_string(),
        whisper_model: PathBuf::from("/nonexistent"),
        ..VoiceSettings::default()
    };
    let mut edge = memcp_client::Client::new(&mem.url(0), &mem.tokens[0]);
    let long = "mot ".repeat(2500);
    for (name, said) in [
        (
            "2026-11-02-13h34-reunion.wav",
            format!("réunion alpha {long}"),
        ),
        ("2026-11-02-14h00-vide.wav", "SILENCE".to_string()),
        ("2026-11-02-15h00-deja.wav", "A-NE-PAS-LIRE".to_string()),
        ("2026-11-02-16h00-broken.wav", "BROKEN".to_string()),
    ] {
        edge.file_put("sam-me", name, "dictée", &["inbox/audio"], &audio(&said))
            .expect("recording");
    }
    edge.call(
        "remember",
        json!({"space": "sam-me", "text": "Dictée du 2026-11-02, audio 2026-11-02-15h00-deja.wav : déjà transcrite.", "tags": ["inbox"]}),
    )
    .expect("note");
    let mut voice = memcp_client::Client::new(&mem.url(1), &mem.tokens[1]);
    let state = dir.join("state");
    let r = transcribe(&mut voice, &["sam-me".to_string()], &settings, &state, 60).expect("pass");
    assert_eq!((r.transcribed, r.nothing_audible), (1, 1), "{r:?}");
    assert!(r.problems.iter().any(|p| p.contains("broken")), "{r:?}");
    let parts = mem.texts("inbox", &["réunion"]);
    let parts: Vec<&String> = parts
        .iter()
        .filter(|t| t.contains("Dictée du 2026-11-02 à 13h34"))
        .collect();
    assert!(
        parts.len() >= 3
            && parts
                .iter()
                .all(|t| t.contains("(partie ") && t.contains("reunion.wav (file_")),
        "{parts:?}"
    );
    assert!(
        mem.texts("inbox", &["A-NE-PAS-LIRE"]).is_empty(),
        "a recording already named is not read again"
    );
    assert!(
        !mem.texts("inbox/audio", &["vide"])
            .iter()
            .any(|t| t.contains("vide.wav")),
        "silence left the inbox"
    );
    // A second pass reads nothing again; the broken one is tried until it is left alone.
    let again =
        transcribe(&mut voice, &["sam-me".to_string()], &settings, &state, 60).expect("pass");
    assert_eq!(
        (again.transcribed, again.nothing_audible),
        (0, 0),
        "{again:?}"
    );
    let _ = transcribe(&mut voice, &["sam-me".to_string()], &settings, &state, 60);
    let fourth =
        transcribe(&mut voice, &["sam-me".to_string()], &settings, &state, 60).expect("pass");
    assert!(
        fourth.problems.is_empty(),
        "left alone after three tries: {fourth:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_space_the_key_writes_to_is_read_once_and_a_failure_can_be_retried() {
    let mem = memory("voice-all", &["sam-me/inbox:rw", "all"]);
    // A space made after the key: read all the same.
    {
        let ctl = mem.app.brain.open_control().expect("control");
        let sam: String = ctl
            .query_row("SELECT id FROM users WHERE name = 'sam'", [], |r| r.get(0))
            .expect("sam");
        mem.app
            .brain
            .add_vault(&sam, "sam", "work")
            .expect("new space");
    }
    mem.up();
    let dir = unique("voice-all");
    std::fs::create_dir_all(&dir).expect("dir");
    let settings = VoiceSettings {
        whisper_cmd: script(&dir, "whisper-cli", WHISPER).display().to_string(),
        ffmpeg_cmd: script(&dir, "ffmpeg", FFMPEG).display().to_string(),
        whisper_model: PathBuf::from("/nonexistent"),
        ..VoiceSettings::default()
    };
    let mut voice = memcp_client::Client::new(&mem.url(1), &mem.tokens[1]);
    voice.retry.tries = 1;
    for (space, name, said) in [
        ("sam-me", "2026-11-02-13h34-un.wav", "memo alpha"),
        ("sam-work", "2026-11-02-14h00-deux.wav", "memo beta"),
        ("sam-work", "2026-11-02-15h00-casse.wav", "BROKEN"),
    ] {
        voice
            .file_put(space, name, "dictée", &["inbox/audio"], &audio(said))
            .expect("recording");
    }
    let waiting = memcp_capture::waiting(&mut voice).expect("waiting");
    assert!(
        waiting.iter().any(|(s, n, _)| s == "sam-work" && *n == 2),
        "{waiting:?}"
    );
    let state = dir.join("state");
    let all = vec!["all".to_string()];
    let r = transcribe(&mut voice, &all, &settings, &state, 60).expect("pass");
    assert_eq!(r.transcribed, 2, "{r:?}");
    let in_space = |space: &str, tags: &[&str], q: &str| -> Vec<String> {
        let mut c = memcp_client::Client::new(&mem.url(1), &mem.tokens[1]);
        let a = c
            .call(
                "recall",
                json!({ "space": space, "query": q, "tags": tags, "limit": 50 }),
            )
            .expect("recall");
        a["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["text"].as_str().map(String::from))
            .collect()
    };
    assert_eq!(in_space("sam-work", &["inbox"], "memo").len(), 1);
    assert_eq!(in_space("sam-me", &["inbox"], "memo").len(), 1);
    assert!(in_space("sam-work", &["inbox"], "deux")[0].contains("deux.wav"));
    // Again: nothing is read twice. The broken one is given up after three tries and says so.
    for _ in 0..3 {
        let again = transcribe(&mut voice, &all, &settings, &state, 60).expect("pass");
        assert_eq!(again.transcribed, 0, "{again:?}");
    }
    let marks = in_space("sam-work", &["inbox/audio/failed"], "recording");
    assert_eq!(marks.len(), 1, "{marks:?}");
    let quiet = transcribe(&mut voice, &all, &settings, &state, 60).expect("pass");
    assert!(quiet.problems.is_empty(), "left alone: {quiet:?}");
    // Retry: the person retires the failure note, the next pass tries again.
    let mut c = memcp_client::Client::new(&mem.url(1), &mem.tokens[1]);
    let hit = c
        .call(
            "recall",
            json!({ "space": "sam-work", "query": "transcribed", "tags": ["inbox/audio/failed"], "limit": 5 }),
        )
        .expect("recall");
    let id = hit["results"][0]["id"].as_str().expect("id").to_string();
    c.call(
        "retire",
        json!({ "space": "sam-work", "id": id, "reason": "retry" }),
    )
    .expect("retire");
    let retried = transcribe(&mut voice, &all, &settings, &state, 60).expect("pass");
    assert!(
        retried.problems.iter().any(|p| p.contains("casse")),
        "tried again: {retried:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
