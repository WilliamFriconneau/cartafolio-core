//! The voice intake against the real memory, with two small scripts standing for `ffmpeg` and `whisper-cli`: the audio of
//! a test *is* the text it says.

// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{memory, unique};
use memcp_bridge::{VoiceSettings, transcribe};
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
