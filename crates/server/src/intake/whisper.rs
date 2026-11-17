//! A recording to its text, on this machine: `ffmpeg` makes it a 16 kHz wav, a silence is told from a speech, and
//! `whisper-cli` (whisper.cpp) reads it. Nothing leaves the machine.
//!
//! Whisper invents text on a silence or a noise (« Sous-titrage Société Radio-Canada »): an audio with no sound at all,
//! and a text that is only one of those phrases, are told apart from speech and give nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The function a [`Decoder`] holds.
pub type DecodeFn = dyn Fn(&Path) -> Result<Vec<f32>, String> + Send + Sync;

/// Turns a recording into 16 kHz, mono, 32-bit float samples. A host that has its own audio system (an app: AVFoundation
/// on a Mac, where `ffmpeg` cannot be run) provides one, and the speech model then runs inside the program
/// (`local-whisper`, on the GPU with `metal`). With none, `ffmpeg` and `whisper-cli` are run, or the speech service is asked.
#[derive(Clone)]
pub struct Decoder(pub std::sync::Arc<DecodeFn>);

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decoder")
    }
}

/// How recordings are read on this machine.
#[derive(Clone, Debug)]
pub struct VoiceSettings {
    pub whisper_cmd: String,
    pub whisper_model: PathBuf,
    pub whisper_lang: String,
    pub whisper_threads: u32,
    pub ffmpeg_cmd: String,
    /// A speech service of the OpenAI kind (`…/v1/audio/transcriptions`): when set, a recording is sent there and no
    /// speech model or `ffmpeg` is needed on this machine. The recording leaves it: say so to the person.
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub api_model: String,
    /// A table of terms and their wrong spellings (a Markdown table), to help and to correct Whisper.
    pub lexicon: Option<PathBuf>,
    /// The host's way to decode a recording: when set, the speech model (`whisper_model`) runs inside the program.
    pub decoder: Option<Decoder>,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            whisper_cmd: "whisper-cli".into(),
            whisper_model: PathBuf::new(),
            whisper_lang: "fr".into(),
            whisper_threads: 8,
            ffmpeg_cmd: "ffmpeg".into(),
            api_url: None,
            api_key: None,
            api_model: "whisper-1".into(),
            lexicon: None,
            decoder: None,
        }
    }
}

/// What a recording said.
#[derive(Debug, PartialEq, Eq)]
pub enum Heard {
    Text {
        text: String,
        seconds: u32,
    },
    /// Silence, a noise, or only what Whisper makes up on them.
    Nothing,
}

/// What Whisper makes up on a silence, learned from the subtitles it was trained on.
const PARASITES: [&str; 8] = [
    "sous-titr",
    "amara.org",
    "merci d'avoir regard",
    "merci d’avoir regard",
    "abonnez-vous",
    "thanks for watching",
    "sous titres réalisé",
    "sous titres par",
];

/// Whether the text is only made up phrases: once each known phrase (and what follows it on its line) is taken out,
/// nothing but punctuation is left. « Penser à vérifier les sous-titres du film » is speech, and stays.
pub fn parasite(text: &str) -> bool {
    let low = text.to_lowercase();
    if !PARASITES.iter().any(|p| low.contains(p)) {
        return false;
    }
    low.lines().all(|line| {
        let mut rest = line.to_string();
        for p in PARASITES {
            if let Some(at) = rest.find(p) {
                rest.truncate(at);
            }
        }
        // What stands before a phrase on its line: a few words at most (« Sous-titres réalisés par… »).
        rest.chars().filter(|c| c.is_alphanumeric()).count() < 3
    })
}

/// The text without what Whisper makes up inside a long recording: a line that is only a made up phrase, a line with
/// no word (« . . . . »), and the same line or sentence said again and again (a loop of the model) kept once.
pub fn clean(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.chars().filter(|c| c.is_alphanumeric()).count() == 0 || parasite(line) {
            continue;
        }
        // A sentence said again right after itself, in the same line.
        let mut kept: Vec<&str> = Vec::new();
        for s in line.split_inclusive(['.', '!', '?']) {
            let same = kept.last().is_some_and(|l| {
                l.trim().to_lowercase() == s.trim().to_lowercase()
                    && s.chars().filter(|c| c.is_alphanumeric()).count() > 0
            });
            if !same {
                kept.push(s);
            }
        }
        let line = kept.concat().trim().to_string();
        if out
            .last()
            .is_some_and(|l| l.to_lowercase() == line.to_lowercase())
        {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// The terms the person says that a speech model gets wrong, and how it spells them.
#[derive(Default)]
pub struct Lexicon {
    /// (the right term, its wrong spellings)
    entries: Vec<(String, Vec<String>)>,
}

impl Lexicon {
    /// Reads a Markdown table: `| term | wrong spellings, separated by commas | ... |`.
    pub fn load(file: Option<&Path>) -> Lexicon {
        let text = file
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        let mut entries = vec![];
        for line in text.lines().filter(|l| l.starts_with('|')) {
            let cells: Vec<&str> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cells.len() < 3
                || cells[0].is_empty()
                || cells[0] == "Terme"
                || cells[0].chars().all(|c| "-: ".contains(c))
            {
                continue;
            }
            let variants = cells[1]
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            entries.push((cells[0].to_string(), variants));
        }
        Lexicon { entries }
    }

    /// The terms, to tell Whisper what to expect (it reads about 500 characters of it).
    pub fn prompt(&self) -> String {
        let mut out = String::new();
        for (term, _) in &self.entries {
            if out.chars().count() + term.chars().count() + 2 > 520 {
                break;
            }
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(term);
        }
        out
    }

    /// The text with each wrong spelling put right, whole words only, whatever the case.
    pub fn correct(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (term, variants) in &self.entries {
            for v in variants {
                out = replace_word(&out, v, term);
            }
        }
        out
    }
}

/// `from` as a whole word (case folded) replaced by `to`.
fn replace_word(text: &str, from: &str, to: &str) -> String {
    let (low, from_low) = (text.to_lowercase(), from.to_lowercase());
    // Lowercasing can change the length of a letter: only work where no letter changed length (comparing the totals
    // is not enough, two changes can cancel out).
    if from_low.is_empty()
        || text
            .chars()
            .any(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() != c.len_utf8())
    {
        return text.to_string();
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::new();
    let mut at = 0;
    while let Some(i) = low[at..].find(&from_low) {
        let (start, end) = (at + i, at + i + from_low.len());
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        out.push_str(&text[at..start]);
        if word(before) || word(after) {
            out.push_str(&text[start..end]);
        } else {
            out.push_str(to);
        }
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// Runs `cmd` with no input, its error output kept in `err_file`, and stops it after `secs` seconds: a tool that
/// hangs on a broken file must not stop every pass after it. The status and what it wrote on its error output.
fn run(
    cmd: &mut Command,
    err_file: &Path,
    secs: u64,
) -> Result<(std::process::ExitStatus, String), String> {
    let err = std::fs::File::create(err_file).map_err(|e| e.to_string())?;
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(err)
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("stopped after {secs} s"));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok((
        status,
        std::fs::read_to_string(err_file).unwrap_or_default(),
    ))
}

pub struct Whisper {
    cmd: String,
    model: PathBuf,
    lang: String,
    threads: u32,
    ffmpeg: String,
    api_url: Option<String>,
    api_key: Option<String>,
    api_model: String,
    lexicon: Lexicon,
    lexicon_file: Option<PathBuf>,
    decoder: Option<Decoder>,
}

impl Whisper {
    pub fn new(c: &VoiceSettings) -> Whisper {
        Whisper {
            cmd: c.whisper_cmd.clone(),
            model: c.whisper_model.clone(),
            lang: c.whisper_lang.clone(),
            threads: c.whisper_threads,
            ffmpeg: c.ffmpeg_cmd.clone(),
            api_url: c.api_url.clone(),
            api_key: c.api_key.clone(),
            api_model: c.api_model.clone(),
            lexicon: Lexicon::load(c.lexicon.as_deref()),
            lexicon_file: c.lexicon.clone(),
            decoder: c.decoder.clone(),
        }
    }

    /// What `audio` says. `work` is a folder for the temporary files.
    pub fn transcribe(&self, audio: &Path, work: &Path) -> Result<Heard, String> {
        if let Some(url) = &self.api_url {
            return self.via_api(url, audio);
        }
        if let Some(decoder) = &self.decoder {
            return self.in_process(decoder, audio);
        }
        let wav = work.join("audio.wav");
        let (made, why) = run(
            Command::new(&self.ffmpeg)
                .args(["-nostdin", "-loglevel", "error", "-y", "-i"])
                .arg(audio)
                .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
                .arg(&wav),
            &work.join("ffmpeg.err"),
            600,
        )
        .map_err(|e| format!("{}: {e}", self.ffmpeg))?;
        if !made.success() {
            return Err(format!("the recording cannot be read: {}", why.trim()));
        }
        // 16 kHz, mono, 16 bits: 32 000 bytes a second after a 44-byte header.
        let bytes = std::fs::metadata(&wav).map_err(|e| e.to_string())?.len();
        let seconds = u32::try_from(bytes.saturating_sub(44) / 32_000).unwrap_or(u32::MAX);
        if self.silent(&wav, work) {
            return Ok(Heard::Nothing);
        }
        let base = work.join("heard");
        let mut cmd = Command::new(&self.cmd);
        cmd.arg("-m")
            .arg(&self.model)
            .arg("-f")
            .arg(&wav)
            .args(["-l", &self.lang, "-nt", "-np", "-t"])
            .arg(self.threads.to_string());
        let vocab = self.lexicon.prompt();
        if !vocab.is_empty() {
            cmd.args(["--prompt", &vocab]);
        }
        cmd.args(["-otxt", "-of"]).arg(&base);
        // A recording is an hour at most; whisper reads it many times faster than real time.
        let (ran, why) = run(&mut cmd, &work.join("whisper.err"), 3600)
            .map_err(|e| format!("{}: {e}", self.cmd))?;
        if !ran.success() {
            return Err(format!(
                "whisper failed: {}",
                why.lines().last().unwrap_or("").trim()
            ));
        }
        let raw = std::fs::read_to_string(base.with_extension("txt"))
            .map_err(|e| format!("no text from whisper: {e}"))?;
        let text = clean(&raw);
        if text.is_empty() || parasite(&text) {
            return Ok(Heard::Nothing);
        }
        Ok(Heard::Text {
            text: self.lexicon.correct(&text),
            seconds,
        })
    }

    /// What `audio` says, by the speech model inside the program, from the samples the host decoded.
    #[cfg(feature = "local-whisper")]
    fn in_process(&self, decoder: &Decoder, audio: &Path) -> Result<Heard, String> {
        let samples = (decoder.0)(audio)?;
        let seconds = u32::try_from(samples.len() / 16_000).unwrap_or(u32::MAX);
        let heard = super::local::transcribe_samples(
            &self.model,
            &samples,
            &self.lang,
            self.threads,
            self.lexicon_file.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        Ok(match heard {
            Some(text) => Heard::Text { text, seconds },
            None => Heard::Nothing,
        })
    }

    /// Without `local-whisper` the program has no speech model of its own.
    #[cfg(not(feature = "local-whisper"))]
    fn in_process(&self, decoder: &Decoder, audio: &Path) -> Result<Heard, String> {
        let _ = (decoder, audio, &self.lexicon_file);
        Err("this build has no built-in speech model (local-whisper)".into())
    }

    /// What `audio` says, by a speech service of the OpenAI kind: one multipart request, a JSON answer with `text`.
    fn via_api(&self, url: &str, audio: &Path) -> Result<Heard, String> {
        use std::time::Duration;
        const MAX: usize = 24 * 1024 * 1024;
        let bytes = std::fs::read(audio).map_err(|e| format!("{}: {e}", audio.display()))?;
        if bytes.is_empty() {
            return Ok(Heard::Nothing);
        }
        if bytes.len() > MAX {
            return Err("the recording is over 24 MB: too long for a speech service".into());
        }
        let name = audio
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio")
            .replace(['"', '\\', '\r', '\n'], "_");
        let boundary = format!(
            "memcp{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 1024);
        let mut field = |k: &str, v: &str| {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
                )
                .as_bytes(),
            );
        };
        field("model", &self.api_model);
        field("response_format", "json");
        if !self.lang.is_empty() {
            field("language", &self.lang);
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = agent.post(url).header(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        );
        if let Some(k) = &self.api_key {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        let mut resp = req
            .send(&body[..])
            .map_err(|e| format!("the speech service cannot be reached: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("the speech service answered badly: {e}"))?;
        if !(200..300).contains(&status) {
            return Err(format!(
                "the speech service refused ({status}): {}",
                text.chars().take(200).collect::<String>()
            ));
        }
        let said = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["text"].as_str().map(str::to_string))
            .unwrap_or(text);
        let said = clean(&said);
        if said.is_empty() || parasite(&said) {
            return Ok(Heard::Nothing);
        }
        Ok(Heard::Text {
            text: self.lexicon.correct(&said),
            seconds: 0,
        })
    }

    /// No sound at all (a file with the microphone off): the loudest point is below -50 dB.
    fn silent(&self, wav: &Path, work: &Path) -> bool {
        let out = run(
            Command::new(&self.ffmpeg)
                .args(["-nostdin", "-i"])
                .arg(wav)
                .args(["-af", "volumedetect", "-f", "null", "-"]),
            &work.join("volume.err"),
            300,
        );
        let Ok((_, text)) = out else { return false };
        text.lines()
            .find_map(|l| l.split("max_volume:").nth(1))
            .and_then(|v| v.trim().trim_end_matches("dB").trim().parse::<f64>().ok())
            .is_some_and(|db| db < -50.0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{Lexicon, clean, parasite, replace_word};

    #[test]
    fn what_whisper_makes_up_inside_a_long_text_goes_and_a_loop_is_kept_once() {
        let raw = "Bonjour à tous.\nSous-titrage ST' 501\nSous-titrage ST' 501\n. . . . . .\nOn commence.\n\
                   On commence.\npanique. Je ne suis pas un poste de panique. Je ne suis pas un poste de panique.\n\
                   Merci d'avoir regardé cette vidéo !\nOui.";
        assert_eq!(
            clean(raw),
            "Bonjour à tous.\nOn commence.\npanique. Je ne suis pas un poste de panique.\nOui."
        );
        assert_eq!(clean("Sous-titrage Société Radio-Canada\n. . ."), "");
        assert_eq!(
            clean("Penser à vérifier les sous-titres du film"),
            "Penser à vérifier les sous-titres du film"
        );
    }

    #[test]
    fn what_whisper_makes_up_on_a_silence_is_told_from_speech() {
        for t in [
            "Sous-titrage Société Radio-Canada",
            "Sous-titres réalisés par la communauté d'Amara.org",
            "Merci d'avoir regardé cette vidéo !",
        ] {
            assert!(parasite(t), "{t}");
        }
        assert!(!parasite("Acheter du pain."));
        assert!(!parasite("Penser à vérifier les sous-titres du film"));
        assert!(!parasite(&format!(
            "Il faut vérifier le sous-titrage de la vidéo avant de l'envoyer. {}",
            "Et rappeler tout le monde pour jeudi. ".repeat(4)
        )));
    }

    #[test]
    fn a_wrong_spelling_is_put_right_as_a_whole_word_only() {
        assert_eq!(
            replace_word("le sandure et sandurex", "sandure", "ZENDURE"),
            "le ZENDURE et sandurex"
        );
        assert_eq!(
            replace_word("Zendur et zendure", "zendur", "Zendure"),
            "Zendure et zendure"
        );
        assert_eq!(replace_word("rien à changer", "x", "y"), "rien à changer");
    }

    #[test]
    fn a_markdown_table_becomes_a_lexicon() {
        let dir = std::env::temp_dir().join(format!("lex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let f = dir.join("lexique.md");
        std::fs::write(&f, "# Lexique\n\n| Terme | Variantes | Note |\n|---|---|---|\n| Zendure | zendur, sandure | batterie |\n| Tydom | tidom | alarme |\n").expect("write");
        let l = Lexicon::load(Some(&f));
        assert_eq!(l.prompt(), "Zendure, Tydom");
        assert_eq!(
            l.correct("la sandure et le tidom"),
            "la Zendure et le Tydom"
        );
        assert_eq!(Lexicon::load(None).prompt(), "");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod api_tests {
    #![allow(clippy::expect_used)]

    use super::{Heard, VoiceSettings, Whisper};

    /// A speech service that answers once with `status` and `body`, and hands back the request it got.
    fn service(
        status: u16,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<(String, String)>) {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("listen");
        let url = format!("http://{}/v1/audio/transcriptions", server.server_addr());
        let h = std::thread::spawn(move || {
            let mut rq = server.recv().expect("request");
            let auth = rq
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.to_string())
                .unwrap_or_default();
            let mut raw = Vec::new();
            rq.as_reader().read_to_end(&mut raw).expect("body");
            let text = String::from_utf8_lossy(&raw).to_string();
            rq.respond(tiny_http::Response::from_string(body).with_status_code(status))
                .expect("respond");
            (auth, text)
        });
        (url, h)
    }

    fn settings(url: &str) -> VoiceSettings {
        VoiceSettings {
            api_url: Some(url.into()),
            api_key: Some("sk-test".into()),
            api_model: "whisper-test".into(),
            whisper_lang: "fr".into(),
            ..VoiceSettings::default()
        }
    }

    fn recording() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let p =
            std::env::temp_dir().join(format!("memcp-voice-api-{}-{n}.webm", std::process::id()));
        std::fs::write(&p, [0x1A, 0x45, 0xDF, 0xA3, 1, 2, 3, 4]).expect("write");
        p
    }

    #[test]
    fn a_recording_is_sent_to_the_speech_service_and_its_text_comes_back() {
        let (url, seen) = service(200, r#"{"text":" Il faut appeler le plombier vendredi. "}"#);
        let audio = recording();
        let heard = Whisper::new(&settings(&url))
            .transcribe(&audio, &std::env::temp_dir())
            .expect("transcribed");
        assert_eq!(
            heard,
            Heard::Text {
                text: "Il faut appeler le plombier vendredi.".into(),
                seconds: 0
            }
        );
        let (auth, request) = seen.join().expect("service");
        assert_eq!(auth, "Bearer sk-test");
        assert!(
            request.contains("name=\"model\"\r\n\r\nwhisper-test"),
            "{request}"
        );
        assert!(request.contains("name=\"language\"\r\n\r\nfr"), "{request}");
        assert!(request.contains("filename=\"memcp-voice-api-"), "{request}");
        let _ = std::fs::remove_file(audio);
    }

    #[test]
    fn what_a_speech_service_makes_up_on_a_silence_is_nothing() {
        let (url, seen) = service(200, r#"{"text":"Sous-titrage Société Radio-Canada"}"#);
        let audio = recording();
        let heard = Whisper::new(&settings(&url))
            .transcribe(&audio, &std::env::temp_dir())
            .expect("answered");
        assert_eq!(heard, Heard::Nothing);
        let _ = seen.join();
        let _ = std::fs::remove_file(audio);
    }

    #[test]
    fn a_refusal_of_the_service_is_an_error_that_says_why() {
        let (url, seen) = service(401, r#"{"error":"bad key"}"#);
        let audio = recording();
        let err = Whisper::new(&settings(&url))
            .transcribe(&audio, &std::env::temp_dir())
            .expect_err("refused");
        assert!(err.contains("401") && err.contains("bad key"), "{err}");
        let _ = seen.join();
        let _ = std::fs::remove_file(audio);
    }
}
