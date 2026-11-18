//! The intake, as functions of the node: voice notes (two small scripts stand for `ffmpeg` and `whisper-cli`: the audio of a
//! test *is* the text it says), watched folders, absorbed folders, and the settings that drive them. Everything runs on a
//! real memory, in the same process, with the rights of the person it works for.
#![cfg(unix)]
// A setup that fails is a test that fails: `expect` says where.
#![allow(clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use memcp_core::text::Language;
use memcp_core::{Brain, users};
use memcp_server::config::Settings as File;
use memcp_server::intake::whisper::VoiceSettings;
use memcp_server::intake::{self, Intake, Memory, Settings, voice};
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

fn unique(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("memcp-intake-{tag}-{}-{n}", std::process::id()))
}

/// A memory with one person, Sam, who owns `sam-me`, and `sam-work` when asked.
struct World {
    dir: PathBuf,
    brain: Brain,
}

fn world(tag: &str, work: bool) -> World {
    let dir = unique(tag);
    let brain = Brain::init(&dir.join("data"), &[], Language::English).expect("init");
    {
        let ctl = brain.open_control().expect("control");
        let sam = users::create_user(&ctl, users::OWNER, "sam", "Sam", false)
            .expect("user")
            .id;
        brain.add_vault(&sam, "sam", "me").expect("space");
        if work {
            brain.add_vault(&sam, "sam", "work").expect("space");
        }
    }
    World { dir, brain }
}

impl World {
    fn memory(&self) -> Memory {
        Memory::open(&self.brain, Some("sam")).expect("memory")
    }

    /// What the memory holds under `tag` of `space`, found by `word`.
    fn texts(&self, space: &str, tag: &str, word: &str) -> Vec<String> {
        let a = self
            .memory()
            .call(
                "recall",
                json!({ "space": space, "query": word, "tags": [tag], "limit": 50 }),
            )
            .expect("recall");
        a["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["text"].as_str().map(String::from))
            .collect()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

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

fn speech(dir: &Path) -> VoiceSettings {
    VoiceSettings {
        whisper_cmd: script(dir, "whisper-cli", WHISPER).display().to_string(),
        ffmpeg_cmd: script(dir, "ffmpeg", FFMPEG).display().to_string(),
        whisper_model: PathBuf::from("/nonexistent"),
        ..VoiceSettings::default()
    }
}

/// Makes a file old enough for a watched folder to take it.
fn aged(path: &Path) {
    let old = std::time::SystemTime::now() - Duration::from_secs(60);
    let f = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open");
    f.set_modified(old).expect("mtime");
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("dir");
    }
    std::fs::write(&p, bytes).expect("write");
    aged(&p);
    p
}

fn settings_from(text: &str, state: &Path) -> Settings {
    let file = File::parse(text).expect("memcp.toml");
    Settings::from_file(&file, state).expect("settings")
}

#[test]
fn recordings_with_no_text_are_read_here_and_the_rest_left_alone() {
    let w = world("voice", false);
    let dir = w.dir.join("tools");
    std::fs::create_dir_all(&dir).expect("dir");
    let settings = speech(&dir);
    let mut mem = w.memory();
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
        mem.file_put("sam-me", name, "dictée", &["inbox/audio"], &audio(&said))
            .expect("recording");
    }
    mem.call(
        "remember",
        json!({"space": "sam-me", "text": "Dictée du 2026-11-02, audio 2026-11-02-15h00-deja.wav : déjà transcrite.", "tags": ["inbox"]}),
    )
    .expect("note");
    let state = dir.join("state");
    let spaces = vec!["sam-me".to_string()];
    let r = voice::transcribe(&mut mem, &spaces, &settings, &state, 60).expect("pass");
    assert_eq!((r.transcribed, r.nothing_audible), (1, 1), "{r:?}");
    assert!(r.problems.iter().any(|p| p.contains("broken")), "{r:?}");
    let parts = w.texts("sam-me", "inbox", "réunion");
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
        w.texts("sam-me", "inbox", "A-NE-PAS-LIRE").is_empty(),
        "a recording already named is not read again"
    );
    assert!(
        !w.texts("sam-me", "inbox/audio", "vide")
            .iter()
            .any(|t| t.contains("vide.wav")),
        "silence left the inbox"
    );
    // A second pass reads nothing again; the broken one is tried until it is left alone.
    let again = voice::transcribe(&mut mem, &spaces, &settings, &state, 60).expect("pass");
    assert_eq!(
        (again.transcribed, again.nothing_audible),
        (0, 0),
        "{again:?}"
    );
    let _ = voice::transcribe(&mut mem, &spaces, &settings, &state, 60);
    let fourth = voice::transcribe(&mut mem, &spaces, &settings, &state, 60).expect("pass");
    assert!(
        fourth.problems.is_empty(),
        "left alone after three tries: {fourth:?}"
    );
}

#[test]
fn every_space_the_person_writes_to_is_read_once_a_space_made_later_included() {
    let w = world("voice-all", true);
    let dir = w.dir.join("tools");
    std::fs::create_dir_all(&dir).expect("dir");
    let settings = speech(&dir);
    let mut mem = w.memory();
    for (space, name, said) in [
        ("sam-me", "2026-11-02-13h34-un.wav", "memo alpha"),
        ("sam-work", "2026-11-02-14h00-deux.wav", "memo beta"),
        ("sam-work", "2026-11-02-15h00-casse.wav", "BROKEN"),
    ] {
        mem.file_put(space, name, "dictée", &["inbox/audio"], &audio(said))
            .expect("recording");
    }
    let waiting = voice::waiting(&mut mem).expect("waiting");
    assert!(
        waiting.iter().any(|(s, n, _)| s == "sam-work" && *n == 2),
        "{waiting:?}"
    );
    let state = dir.join("state");
    let all = vec!["all".to_string()];
    let r = voice::transcribe(&mut mem, &all, &settings, &state, 60).expect("pass");
    assert_eq!(r.transcribed, 2, "{r:?}");
    assert_eq!(w.texts("sam-work", "inbox", "memo").len(), 1);
    assert_eq!(w.texts("sam-me", "inbox", "memo").len(), 1);
    assert!(w.texts("sam-work", "inbox", "deux")[0].contains("deux.wav"));
    // Again: nothing is read twice. The broken one is given up after three tries and says so.
    for _ in 0..3 {
        let again = voice::transcribe(&mut mem, &all, &settings, &state, 60).expect("pass");
        assert_eq!(again.transcribed, 0, "{again:?}");
    }
    let quiet = voice::transcribe(&mut mem, &all, &settings, &state, 60).expect("pass");
    assert!(quiet.problems.is_empty(), "left alone: {quiet:?}");
}

#[test]
fn the_recordings_waiting_are_listed_per_space_for_the_status_command() {
    let w = world("waiting", false);
    let mut mem = w.memory();
    mem.file_put(
        "sam-me",
        "2026-11-02-13h34-un.wav",
        "dictée",
        &["inbox/audio"],
        &audio("hello"),
    )
    .expect("recording");
    let waiting = voice::waiting(&mut mem).expect("waiting");
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].0.as_str(), waiting[0].1), ("sam-me", 1));
}

#[test]
fn a_folder_is_absorbed_new_text_only_names_become_posts_and_proposals_come_back_as_files() {
    let w = world("absorb", false);
    let pro = w.dir.join("Notes");
    std::fs::create_dir_all(pro.join(".obsidian")).expect("dir");
    file(
        &pro,
        "Projects/Alpha.md",
        "# Décisions\nOn garde A.\n".as_bytes(),
    );
    file(&pro, "Raw/transcript.md", "Jamais lu.\n".as_bytes());
    file(
        &pro,
        "Projects/Confidentiel budget.md",
        "Jamais lu non plus.\n".as_bytes(),
    );
    std::fs::write(
        w.dir.join("roles.md"),
        "| Nom | Poste |\n|---|---|\n| Camille Martin | responsable des achats |\n",
    )
    .expect("roles");
    let conf = format!(
        "intake_person = \"sam\"\nabsorb = [\"{}  ->  sam-me\"]\nabsorb_exclude = [\"Raw\"]\nabsorb_protect = [\"confidentiel\"]\nabsorb_roles = \"{}\"\ndeliver = [\"sam-me proposals/pro -> {}\"]\n",
        pro.display(),
        w.dir.join("roles.md").display(),
        pro.join("Ideas").display()
    );
    let mut intake =
        Intake::new(&w.brain, settings_from(&conf, &w.dir.join("intake"))).expect("intake");
    let mut totals = |everything: bool| {
        let mut sent = 0;
        let mut read = 0;
        let mut noted = 0;
        let mut problems = vec![];
        for (_, r) in intake.absorb(everything) {
            let r = r.expect("pass");
            sent += r.sent;
            read += r.files_read;
            noted += r.noted;
            problems.extend(r.problems);
        }
        (sent, read, noted, problems)
    };
    // First pass: stock only, nothing sent (the folder was imported before).
    let (sent, _, noted, problems) = totals(false);
    assert_eq!((sent, noted), (0, 1), "{problems:?}");
    // A change: only the new text goes, with the name replaced by the post.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(
        pro.join("Projects/Alpha.md"),
        "# Décisions\nOn garde A.\n\n# Suite\nCamille Martin valide le budget le 2026-11-10.\n",
    )
    .expect("note");
    let (sent, _, _, problems) = totals(false);
    assert_eq!(sent, 1, "{problems:?}");
    let inbox = w.texts("sam-me", "inbox/absorb", "budget");
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert!(
        inbox[0].contains("responsable des achats valide le budget")
            && !inbox[0].contains("Camille"),
        "{inbox:?}"
    );
    assert!(
        inbox[0].starts_with("D'après la note Projects/Alpha.md"),
        "{inbox:?}"
    );
    // Nothing new: nothing sent.
    let (sent, read, _, _) = totals(false);
    assert_eq!((sent, read), (0, 0));
    // A proposal noted for the work assistant is put down as a file, once.
    w.memory()
        .call(
            "remember",
            json!({"space":"sam-me","text":"Idée : un tableau de bord des tickets par client.","tags":["proposals/pro"]}),
        )
        .expect("proposal");
    let (sent, _, _, problems) = totals(false);
    assert_eq!(sent, 1, "one file put down: {problems:?}");
    // Its guide waits for the person's approval on the page, as an app's guide.
    let waiting: i64 = w
        .brain
        .open_space(&memcp_core::Space::new("sam-me").expect("space"))
        .expect("space")
        .query_row(
            "SELECT count(*) FROM app_guides WHERE app = 'intake' AND text LIKE '%proposals/pro%'",
            [],
            |r| r.get(0),
        )
        .expect("guides");
    assert_eq!(waiting, 1);
    let files: Vec<_> = std::fs::read_dir(pro.join("Ideas"))
        .expect("ideas")
        .flatten()
        .collect();
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(files[0].path()).expect("idea");
    assert!(
        text.contains("source: mémoire") && text.contains("tableau de bord des tickets"),
        "{text}"
    );
    let (sent, _, _, _) = totals(false);
    assert_eq!(sent, 0, "the idea file is not sent back");
}

#[test]
fn a_watched_folder_turns_what_is_added_into_notes_recordings_and_files() {
    let w = world("watch", false);
    let drop = w.dir.join("Drop");
    let state = w.dir.join("intake");
    let conf = format!(
        "intake_person = \"sam\"\nwatch = [\"{} -> sam-me\"]\n",
        drop.display()
    );
    file(
        &drop,
        "idee.md",
        "# Idée\nUn bac à compost au fond du jardin.\n".as_bytes(),
    );
    file(&drop, "liste.txt", "acheter du lin\n".as_bytes());
    file(&drop, "sous/dictee.m4a", &audio("bonjour"));
    file(&drop, "plan.pdf", b"%PDF-1.4 fake");
    file(&drop, ".cache", b"hidden");
    file(&drop, "partiel.crdownload", b"unfinished");
    let mut intake = Intake::new(&w.brain, settings_from(&conf, &state)).expect("intake");
    let scans = intake.watch();
    let r = scans[0].report.as_ref().expect("scan");
    assert_eq!((r.notes, r.files), (2, 2), "{r:?}");
    assert!(r.problems.is_empty(), "{r:?}");
    assert_eq!(scans[0].space, "sam-me");
    // The notes are in the inbox of the space, in the person's words.
    let notes = w.texts("sam-me", "inbox", "compost");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].starts_with("Fichier idee.md déposé le "),
        "{notes:?}"
    );
    assert_eq!(w.texts("sam-me", "inbox", "lin").len(), 1);
    // The recording waits under inbox/audio for the node to read it, dated by the file.
    let waiting = voice::waiting(&mut w.memory()).expect("waiting");
    assert_eq!(waiting.iter().map(|x| x.1).sum::<u64>(), 1, "{waiting:?}");
    let files = w
        .memory()
        .call("file_list", json!({ "space": "sam-me", "limit": 50 }))
        .expect("files");
    let names: Vec<&str> = files["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert!(names.contains(&"plan.pdf"), "{names:?}");
    assert!(
        names
            .iter()
            .any(|n| n.ends_with("-dictee.m4a") && n.contains('h')),
        "{names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.contains("cache") || n.contains("crdownload"))
    );
    // Nothing is taken twice, not by the same node and not after a restart.
    let again = intake.watch();
    assert_eq!(again[0].report.as_ref().expect("scan").notes, 0);
    let mut restarted = Intake::new(&w.brain, settings_from(&conf, &state)).expect("intake");
    let r = restarted.watch();
    let r = r[0].report.as_ref().expect("scan");
    assert_eq!((r.notes, r.files), (0, 0), "{r:?}");
    // What is added later is taken; a file still being written waits.
    file(
        &drop,
        "nouveau.md",
        "Penser aux semis de mars.\n".as_bytes(),
    );
    std::fs::write(drop.join("en-cours.md"), "écrit à l'instant").expect("fresh");
    let r = restarted.watch();
    let r = r[0].report.as_ref().expect("scan");
    assert_eq!((r.notes, r.waiting), (1, 1), "{r:?}");
    assert_eq!(w.texts("sam-me", "inbox", "semis").len(), 1);
    // The files stay where they are.
    assert!(drop.join("idee.md").is_file() && drop.join("sous/dictee.m4a").is_file());
}

#[test]
fn a_folder_with_no_space_goes_to_the_to_sort_box_and_a_missing_folder_is_said() {
    let w = world("watch-triage", false);
    let drop = w.dir.join("Drop");
    let conf = format!(
        "intake_person = \"sam\"\nwatch = [\"{}\", \"{}\"]\n",
        drop.display(),
        w.dir.join("absent").display()
    );
    file(&drop, "note.txt", "rappeler le plombier\n".as_bytes());
    let mut intake =
        Intake::new(&w.brain, settings_from(&conf, &w.dir.join("intake"))).expect("intake");
    let triage = intake.memory().triage_space().expect("triage");
    assert!(triage.ends_with("a-trier"), "{triage}");
    let scans = intake.watch();
    assert_eq!(scans[0].space, triage);
    assert_eq!(scans[0].report.as_ref().expect("scan").notes, 1);
    assert_eq!(w.texts(&triage, "inbox", "plombier").len(), 1);
    assert!(
        scans[1].report.is_err(),
        "the folder that is not there is an error, not a crash"
    );
}

#[test]
fn the_settings_are_checked_and_say_which_line_cannot_work() {
    let state = Path::new("/tmp/none");
    let bad = |text: &str| -> String {
        let file = File::parse(text).expect("parses");
        Settings::from_file(&file, state).expect_err("refused")
    };
    assert!(bad("transcribe = true\n").contains("no speech service"));
    assert!(bad("absorb = [\"/tmp/x\"]\n").contains("names no space"));
    assert!(bad("deliver = [\"sam-me -> /tmp/x\"]\n").contains("SPACE TAG -> FOLDER"));
    assert!(bad("watch = [\" -> sam-me\"]\n").contains("names no folder"));
    let ok = settings_from(
        "transcribe = true\ntranscribe_api_url = \"http://127.0.0.1:1/v1/audio/transcriptions\"\ntranscribe_spaces = \"a, b\"\nwatch = [\"~/Drop -> sam-me\", \"/data/in\"]\nwatch_every = 5\n",
        state,
    );
    assert_eq!(ok.transcribe_spaces, ["a", "b"]);
    assert_eq!(ok.watch.len(), 2);
    assert_eq!(ok.watch[1].space, None);
    assert_eq!(ok.watch_every, 5);
    assert!(!ok.is_idle());
    assert!(settings_from("", state).is_idle());
}

#[test]
fn the_node_runs_the_tasks_by_itself_and_stops_them() {
    let w = world("background", false);
    let tools = w.dir.join("tools");
    std::fs::create_dir_all(&tools).expect("dir");
    let drop = w.dir.join("Drop");
    std::fs::create_dir_all(&drop).expect("dir");
    let conf = format!(
        "intake_person = \"sam\"\ntranscribe = true\ntranscribe_model = \"/nonexistent\"\ntranscribe_cmd = \"{}\"\ntranscribe_ffmpeg = \"{}\"\nwatch = [\"{} -> sam-me\"]\nwatch_every = 1\n",
        script(&tools, "whisper-cli", WHISPER).display(),
        script(&tools, "ffmpeg", FFMPEG).display(),
        drop.display()
    );
    let running = intake::start(&w.brain, settings_from(&conf, &w.dir.join("intake")))
        .expect("start")
        .expect("something to run");
    // A note dropped in the folder, and a recording dropped in the folder: the node takes the note, takes the recording,
    // reads it, and its text becomes a note of the same space.
    file(&drop, "vite.md", "Appeler la mairie lundi.\n".as_bytes());
    file(&drop, "voix.wav", &audio("penser a la reunion du jeudi"));
    let end = Instant::now() + Duration::from_secs(60);
    let mut ok = false;
    while Instant::now() < end {
        if !w.texts("sam-me", "inbox", "mairie").is_empty()
            && w.texts("sam-me", "inbox", "jeudi")
                .iter()
                .any(|t| t.starts_with("Dictée du "))
        {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    running.stop();
    assert!(ok, "the node did not do its work by itself");
}

#[test]
fn a_node_with_nothing_to_do_runs_nothing() {
    let w = world("idle", false);
    let running = intake::start(&w.brain, settings_from("", &w.dir.join("intake"))).expect("start");
    assert!(running.is_none());
}

/// A `whisper-cli` that keeps the words it was told to expect (`--prompt`) and reads the audio as the others do.
const WHISPER_PROMPT: &str = r#"#!/bin/sh
wav=""; base=""; prev=""
for a in "$@"; do
  [ "$prev" = "-f" ] && wav="$a"
  [ "$prev" = "-of" ] && base="$a"
  [ "$prev" = "--prompt" ] && printf '%s' "$a" > "$(dirname "$0")/prompt.txt"
  prev="$a"
done
printf '%s\n' "$@" > "$(dirname "$0")/args.txt"
tail -c +13 "$wav" > "$base.txt"
"#;

/// One pass over `sam-me` with a recording, the notes of the person naming `Wilhelmine Stark` and a `lexicon` when given:
/// the prompt the fake `whisper-cli` was given (`None` when it got no `--prompt`).
fn prompt_given(tag: &str, hints: bool, lexicon: Option<&str>) -> Option<String> {
    let w = world(tag, false);
    let dir = w.dir.join("tools");
    std::fs::create_dir_all(&dir).expect("dir");
    let mut settings = speech(&dir);
    settings.whisper_cmd = script(&dir, "whisper-cli", WHISPER_PROMPT)
        .display()
        .to_string();
    settings.hints = hints;
    if let Some(table) = lexicon {
        let f = dir.join("lexicon.md");
        std::fs::write(&f, table).expect("lexicon");
        settings.lexicon = Some(f);
    }
    let mut mem = w.memory();
    mem.call(
        "remember",
        json!({"space": "sam-me", "text": "Visite de Wilhelmine Stark, puis Wilhelmine Stark est repartie."}),
    )
    .expect("note");
    mem.file_put(
        "sam-me",
        "2026-11-02-13h34-visite.wav",
        "dictée",
        &["inbox/audio"],
        &audio("réunion avec Wilhelmine"),
    )
    .expect("recording");
    let spaces = vec!["sam-me".to_string()];
    let r = voice::transcribe(&mut mem, &spaces, &settings, &dir.join("state"), 60).expect("pass");
    assert_eq!(r.transcribed, 1, "{r:?}");
    assert!(
        std::fs::read_to_string(dir.join("args.txt")).is_ok_and(|a| a.contains("-nt")),
        "the fake whisper ran"
    );
    std::fs::read_to_string(dir.join("prompt.txt")).ok()
}

#[test]
fn the_words_of_the_memory_reach_the_whisper_command_line_as_its_prompt() {
    let p = prompt_given("hints-on", true, None).expect("a --prompt");
    assert!(p.contains("Wilhelmine Stark"), "{p}");
    // The terms the person wrote by hand come first, the words of the memory after, each once.
    let p = prompt_given(
        "hints-lexicon",
        true,
        Some("| Terme | Variantes | Note |\n|---|---|---|\n| Zendure | zen dur, zendur | batterie |\n| Wilhelmine Stark | wilmine | voisine |\n"),
    )
    .expect("a --prompt");
    assert!(p.starts_with("Zendure, Wilhelmine Stark"), "{p}");
    assert_eq!(p.matches("Wilhelmine Stark").count(), 1, "{p}");
}

#[test]
fn with_guided_transcription_off_the_command_line_gets_no_words_of_the_memory() {
    assert_eq!(prompt_given("hints-off", false, None), None);
    // The lexicon alone still goes as before.
    let p = prompt_given(
        "hints-off-lexicon",
        false,
        Some("| Terme | Variantes | Note |\n|---|---|---|\n| Zendure | zen dur | batterie |\n"),
    );
    assert_eq!(p.as_deref(), Some("Zendure"));
}

#[test]
fn transcribe_hints_is_on_unless_it_is_turned_off() {
    let state = Path::new("/tmp/none");
    assert!(settings_from("", state).voice.hints);
    assert!(
        settings_from("transcribe_hints = true\n", state)
            .voice
            .hints
    );
    assert!(
        !settings_from("transcribe_hints = false\n", state)
            .voice
            .hints
    );
    assert!(VoiceSettings::default().hints);
}
