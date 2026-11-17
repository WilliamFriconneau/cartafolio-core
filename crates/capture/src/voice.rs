//! The voice part of the intake, on the machine that has the speech model: a recording that reached an inbox with no
//! text is read by Whisper here, its text becomes a note of the same inbox (filed later like any note), and the recording
//! is erased from the memory as soon as its text is filed — the one erasure the memory has.
//!
//! - A recording already named by a note (transcribed on the capturing side) is left alone: a note received means the work
//!   is done.
//! - A long text is cut into parts; a pass that stops halfway resumes from the missing part.
//! - Silence, and a text that is only what Whisper makes up on silence, give nothing: that recording leaves the inbox.
//! - A recording that cannot be read is tried three times, then left alone, said once.
//!
//! Only one pass runs at a time on a state folder (a lock); the work files live in a private folder under it.

use std::path::{Path, PathBuf};

use memcp_client::{Client, Error as ClientError};
use serde_json::{Value, json};

use crate::Error;
use crate::whisper::{Heard, VoiceSettings, Whisper};

/// The longest text of one note (the memory takes 4000 characters).
const NOTE_MAX: usize = 3800;
/// Tries before a recording that cannot be read is left alone.
const MAX_FAILS: u64 = 3;
/// A recording is erased as soon as its text is filed (the memory allows it from then on).
const ERASE_AFTER: i64 = 0;
/// Passes refused by the memory before an erasure is given up (the text was retired by someone, say).
const MAX_ERASE_FAILS: u64 = 5;

/// How a voice pass went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VoiceReport {
    pub transcribed: u32,
    pub nothing_audible: u32,
    pub erased: u32,
    pub problems: Vec<String>,
    pub key_refused: bool,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// What the passes remember: `{files: {file_id: {space, name, done_at, parts, of, fails, erase_fails, erased}}}`.
struct State {
    file: PathBuf,
    data: Value,
}

impl State {
    fn open(dir: &Path) -> Result<State, Error> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
        let file = dir.join("voice.json");
        let data = match std::fs::read_to_string(&file) {
            Ok(t) => match serde_json::from_str::<Value>(&t) {
                Ok(v) if v["files"].is_object() => v,
                _ => {
                    let _ =
                        std::fs::rename(&file, dir.join(format!("voice.corrupt-{}.json", now())));
                    json!({ "files": {} })
                }
            },
            Err(_) => json!({ "files": {} }),
        };
        Ok(State { file, data })
    }

    fn save(&self) -> Result<(), Error> {
        let tmp = self.file.with_extension("tmp");
        std::fs::write(&tmp, self.data.to_string()).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.file).map_err(|e| Error::Io(e.to_string()))
    }

    fn entry(&mut self, id: &str) -> &mut Value {
        if self.data["files"].get(id).is_none() {
            self.data["files"][id] =
                json!({ "parts": [], "fails": 0, "erase_fails": 0, "erased": false });
        }
        &mut self.data["files"][id]
    }

    fn get(&self, id: &str) -> Option<&Value> {
        self.data["files"].get(id)
    }
}

/// Whether `text` names the recording `name` as a whole word (not `1.m4a` inside `11.m4a`).
fn names(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric() && c != '-' && c != '_')
            && after.is_none_or(|c| !c.is_alphanumeric() && c != '-' && c != '_')
    })
}

/// `text` in pieces of at most `size` characters, at a word, else anywhere.
fn split(text: &str, size: usize) -> Vec<String> {
    let mut parts = vec![];
    let mut cur = String::new();
    for word in text.split_inclusive(char::is_whitespace) {
        if cur.chars().count() + word.chars().count() > size && !cur.trim().is_empty() {
            parts.push(cur.trim().to_string());
            cur.clear();
        }
        cur.push_str(word);
        while cur.chars().count() > size {
            let head: String = cur.chars().take(size).collect();
            cur = cur.chars().skip(size).collect();
            parts.push(head.trim().to_string());
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts.retain(|p| !p.is_empty());
    parts
}

fn tags_of(fact: &Value) -> Vec<&str> {
    fact["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// When a recording was made: from its name (`2026-11-02-13h34-...`, the local time the capturing side wrote), else when
/// the memory received it. Unix time.
fn made_at(f: &Value, name: &str, offset_minutes: i32) -> i64 {
    let digits: Vec<i64> = name
        .get(..16)
        .map(|p| p.split(['-', 'h']).filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default();
    if let [y, mo, d, h, mi] = digits[..]
        && (1..=12).contains(&mo)
        && (1..=31).contains(&d)
        && (0..24).contains(&h)
        && (0..60).contains(&mi)
    {
        return crate::time::unix(y, mo, d, h, mi) - i64::from(offset_minutes) * 60;
    }
    f["created_at"]
        .as_str()
        .and_then(crate::time::parse_iso)
        .unwrap_or_else(now)
}

fn fail(report: &mut VoiceReport, what: &str, e: &ClientError) {
    if matches!(e, ClientError::Refused) {
        report.key_refused = true;
    }
    report.problems.push(format!("{what}: {e}"));
}

/// The spaces to read: the list as given, or, when it holds `all`, every space the key can write to (a space made
/// since is read too, nothing is named twice).
pub fn resolve_spaces(client: &mut Client, wanted: &[String]) -> Result<Vec<String>, ClientError> {
    if !wanted.iter().any(|w| w.eq_ignore_ascii_case("all")) {
        return Ok(wanted.to_vec());
    }
    let listed = client.call("spaces", json!({}))?;
    let mut out: Vec<String> = listed["spaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["writable"] == true)
        .filter_map(|s| s["name"].as_str().map(String::from))
        .collect();
    for w in wanted {
        if !w.eq_ignore_ascii_case("all") && !out.contains(w) {
            out.push(w.clone());
        }
    }
    Ok(out)
}

/// Recordings that wait for a text, per space: `(space, how many, age in seconds of the oldest)`.
pub fn waiting(client: &mut Client) -> Result<Vec<(String, u64, i64)>, ClientError> {
    let listed = client.call("spaces", json!({}))?;
    Ok(listed["spaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let n = s["recordings_waiting"].as_u64().filter(|n| *n > 0)?;
            let age = now() - s["oldest_recording_at"].as_i64().unwrap_or_else(now);
            Some((s["name"].as_str()?.to_string(), n, age.max(0)))
        })
        .collect())
}

/// One pass over the inboxes of `spaces`: recordings transcribed, old ones erased.
pub fn transcribe(
    client: &mut Client,
    vaults: &[String],
    settings: &VoiceSettings,
    state_dir: &Path,
    offset_minutes: i32,
) -> Result<VoiceReport, Error> {
    let mut report = VoiceReport::default();
    std::fs::create_dir_all(state_dir).map_err(|e| Error::Io(e.to_string()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir.join("voice.lock"))
        .map_err(|e| Error::Io(e.to_string()))?;
    if lock.try_lock().is_err() {
        report.problems.push("another pass is running".into());
        return Ok(report);
    }
    let mut state = State::open(state_dir)?;
    let whisper = Whisper::new(settings);
    let vaults = &match resolve_spaces(client, vaults) {
        Ok(v) => v,
        Err(e) => {
            fail(&mut report, "the spaces cannot be listed", &e);
            return Ok(report);
        }
    };
    for vault in vaults {
        // The recordings waiting are the descriptions under inbox/audio, newest first.
        let listed = match client.call(
            "recall",
            json!({ "space": vault, "query": "", "tags": ["inbox/audio"], "limit": 50 }),
        ) {
            Ok(l) => l,
            Err(e) => {
                fail(
                    &mut report,
                    &format!("the recordings of {vault} cannot be listed"),
                    &e,
                );
                if report.key_refused {
                    break;
                }
                continue;
            }
        };
        let files = match client.call("file_list", json!({ "space": vault, "limit": 50 })) {
            Ok(f) => f,
            Err(e) => {
                fail(
                    &mut report,
                    &format!("the files of {vault} cannot be listed"),
                    &e,
                );
                continue;
            }
        };
        for desc in listed["results"].as_array().into_iter().flatten() {
            let Some(desc_id) = desc["id"].as_str() else {
                continue;
            };
            let Some(f) = files["files"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| f["fact_id"].as_str() == Some(desc_id))
            else {
                continue;
            };
            let (Some(id), Some(name)) = (f["id"].as_str(), f["name"].as_str()) else {
                continue;
            };
            if !f["mime"].as_str().is_some_and(|m| m.starts_with("audio/"))
                || f["status"] != "active"
            {
                continue;
            }
            let seen = state.get(id);
            if seen.is_some_and(|e| e["done_at"].is_i64()) {
                continue;
            }
            if seen.and_then(|e| e["fails"].as_u64()).unwrap_or(0) >= MAX_FAILS {
                let marked = seen.is_some_and(|e| e["marked"] == true);
                let tagged = tags_of(desc).contains(&FAILED_TAG);
                if marked && !tagged {
                    // The person took the tag off: another round of tries.
                    let e = state.entry(id);
                    e["fails"] = json!(0);
                    e["marked"] = json!(false);
                    state.save()?;
                } else {
                    if !marked {
                        mark_failed(client, &mut state, id, &mut report);
                        state.save()?;
                    }
                    continue;
                }
            }
            let seen = state.get(id);
            let started = seen
                .and_then(|e| e["parts"].as_array())
                .is_some_and(|p| !p.is_empty());
            if !started {
                match named_by_a_note(client, name, desc_id) {
                    Ok(Some(note)) => {
                        let e = state.entry(id);
                        e["vault"] = json!(vault);
                        e["name"] = json!(name);
                        e["done_at"] = json!(now());
                        e["text"] = json!(note);
                        state.save()?;
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        fail(&mut report, "the notes cannot be searched", &e);
                        continue;
                    }
                }
            }
            one(
                client,
                &whisper,
                &mut state,
                vault,
                f,
                id,
                name,
                offset_minutes,
                state_dir,
                &mut report,
            )?;
            if report.key_refused {
                state.save()?;
                return Ok(report);
            }
        }
    }
    erase_old(client, &mut state, &mut report);
    state.save()?;
    Ok(report)
}

/// A note other than the recording's own description that names it: the id of the first.
fn named_by_a_note(
    client: &mut Client,
    name: &str,
    own: &str,
) -> Result<Option<String>, ClientError> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let found = client.call(
        "recall",
        json!({ "query": stem.replace(['-', '_'], " "), "tags": ["inbox"], "limit": 20 }),
    )?;
    let plain = client.call(
        "recall",
        json!({ "query": stem.replace(['-', '_'], " "), "limit": 20 }),
    )?;
    Ok(found["results"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(plain["results"].as_array().into_iter().flatten())
        .find(|h| {
            h["id"].as_str() != Some(own)
                && h["type"].as_str() != Some("file")
                && h["status"] == "active"
                && !tags_of(h).iter().any(|t| t.starts_with("inbox/audio"))
                && h["text"].as_str().is_some_and(|t| names(t, name))
        })
        .and_then(|h| h["id"].as_str())
        .map(String::from))
}

#[allow(clippy::too_many_arguments)]
fn one(
    client: &mut Client,
    whisper: &Whisper,
    state: &mut State,
    vault: &str,
    f: &Value,
    id: &str,
    name: &str,
    offset_minutes: i32,
    state_dir: &Path,
    report: &mut VoiceReport,
) -> Result<(), Error> {
    {
        let e = state.entry(id);
        e["vault"] = json!(vault);
        e["name"] = json!(name);
    }
    let bytes = match client.file_get(id) {
        Ok((_, b)) => b,
        Err(e) => {
            fail(report, &format!("{name} cannot be read"), &e);
            return Ok(());
        }
    };
    // A private folder for its files, under the state folder.
    let base = state_dir.join("work");
    std::fs::create_dir_all(&base).map_err(|e| Error::Io(e.to_string()))?;
    crate::private::set_private_dir(&base).map_err(|e| Error::Io(e.to_string()))?;
    let work = base.join(id.replace(['/', '.'], "_"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir(&work).map_err(|e| Error::Io(e.to_string()))?;
    let heard = (|| -> Result<Heard, String> {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("m4a");
        let audio = work.join(format!("recording.{ext}"));
        std::fs::write(&audio, &bytes).map_err(|e| e.to_string())?;
        whisper.transcribe(&audio, &work)
    })();
    let _ = std::fs::remove_dir_all(&work);
    match heard {
        Ok(Heard::Nothing) => {
            state.entry(id)["done_at"] = json!(now());
            // Nothing to wait for: the recording leaves the inbox (its bytes stay: `file_retire` erases nothing).
            let _ = client.call(
                "file_retire",
                json!({ "id": id, "reason": "nothing audible in it (silence or noise), said the bridge" }),
            );
            report.nothing_audible += 1;
        }
        Ok(Heard::Text { text, seconds }) => {
            let at = made_at(f, name, offset_minutes);
            let (y, m, d, hh, mm) = crate::time::civil(at, offset_minutes);
            let head = format!(
                "Dictée du {y:04}-{m:02}-{d:02} à {hh:02}h{mm:02}, {seconds} s, audio {name} ({id})"
            );
            let parts = split(
                &text,
                NOTE_MAX.saturating_sub(head.chars().count() + 30).max(200),
            );
            let done = state.entry(id)["parts"].as_array().map_or(0, Vec::len);
            for (i, part) in parts.iter().enumerate().skip(done) {
                let of = if parts.len() > 1 {
                    format!(" (partie {}/{})", i + 1, parts.len())
                } else {
                    String::new()
                };
                let args = json!({ "space": vault, "kind": "context", "tags": ["inbox", "dictee"], "text": format!("{head}{of} : {part}") });
                match client.call("remember", args) {
                    Ok(r) => {
                        let Some(fact) = r["fact"]["id"].as_str() else {
                            report
                                .problems
                                .push(format!("{name}: part {} was not kept", i + 1));
                            return Ok(());
                        };
                        let e = state.entry(id);
                        e["of"] = json!(parts.len());
                        if let Some(list) = e["parts"].as_array_mut() {
                            list.push(json!(fact));
                        }
                        state.save()?;
                    }
                    Err(e) => {
                        fail(
                            report,
                            &format!("the text of {name} cannot be kept (it resumes next pass)"),
                            &e,
                        );
                        return Ok(());
                    }
                }
            }
            state.entry(id)["done_at"] = json!(now());
            report.transcribed += 1;
        }
        Err(why) => {
            let e = state.entry(id);
            let n = e["fails"].as_u64().unwrap_or(0) + 1;
            e["fails"] = json!(n);
            let last = if n >= MAX_FAILS {
                " (left alone from now on)"
            } else {
                ""
            };
            report.problems.push(format!("{name}: {why}{last}"));
            if n >= MAX_FAILS {
                mark_failed(client, state, id, report);
            }
        }
    }
    state.save()
}

/// The tag a recording given up on carries on its description (see `transcription_failed`).
const FAILED_TAG: &str = "failed/transcription";

/// Tags the recording `id` as failed, so that it is visible and counted apart from what waits for the person.
fn mark_failed(client: &mut Client, state: &mut State, id: &str, report: &mut VoiceReport) {
    match client.call("transcription_failed", json!({ "id": id })) {
        Ok(_) => state.entry(id)["marked"] = json!(true),
        Err(e) => fail(report, "the failed recording cannot be tagged", &e),
    }
}

/// The recordings whose text is old enough, wherever it was filed: erased. The text is found by the recording's id, which
/// it carries (« audio <name> (<id>) »), so a note filed in another space, or corrected since, is found too.
fn erase_old(client: &mut Client, state: &mut State, report: &mut VoiceReport) {
    let due: Vec<(String, i64)> = state.data["files"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(_, e)| e["erased"] != true)
                .filter_map(|(id, e)| Some((id.clone(), e["done_at"].as_i64()?)))
                .filter(|(_, t)| now() - t >= ERASE_AFTER)
                .collect()
        })
        .unwrap_or_default();
    for (id, _) in due {
        let name = state
            .get(&id)
            .and_then(|e| e["name"].as_str())
            .unwrap_or("")
            .to_string();
        let known = state
            .get(&id)
            .and_then(|e| e["text"].as_str())
            .map(String::from);
        // By its id, and by its name: a text written on the capturing side before 12/11 names the file only.
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        let mut queries = vec![id.clone()];
        if !stem.is_empty() {
            queries.push(stem.replace(['-', '_'], " "));
        }
        let hits: Vec<Value> = queries
            .iter()
            .filter_map(|q| {
                client
                    .call("recall", json!({ "query": q, "limit": 20 }))
                    .ok()
            })
            .flat_map(|r| r["results"].as_array().cloned().unwrap_or_default())
            .collect();
        let filed_texts: Vec<&Value> = hits
            .iter()
            .filter(|h| {
                h["status"] == "active"
                    && h["type"].as_str() != Some("file")
                    && !tags_of(h).iter().any(|t| t.starts_with("inbox"))
                    && h["text"]
                        .as_str()
                        .is_some_and(|t| t.contains(&id) || (!name.is_empty() && names(t, &name)))
            })
            .collect();
        // The memory counts the week from the filed copy, written when it was sorted, not from the transcription: a
        // copy younger than that is waited for, not tried (five refusals « too recent » gave it up for good).
        let found: Vec<String> = filed_texts
            .iter()
            .filter(|h| {
                h["created_at"]
                    .as_i64()
                    .is_none_or(|t| now() - t >= ERASE_AFTER)
            })
            .filter_map(|h| h["id"].as_str().map(String::from))
            .collect();
        if found.is_empty() && !filed_texts.is_empty() {
            continue;
        }
        // Only a filed text found now counts a refusal: the note kept at transcription is retired as soon as it is
        // sorted, so a refusal on it alone means « not filed yet », not « never » (it gave up after five passes).
        let filed = !found.is_empty();
        let mut candidates = found;
        candidates.sort();
        candidates.dedup();
        if let Some(k) = known
            && !candidates.contains(&k)
        {
            candidates.push(k);
        }
        if candidates.is_empty() {
            // Not filed yet (still in an inbox): waited for.
            continue;
        }
        let mut last = None;
        let mut done = false;
        for t in &candidates {
            match client.call("file_erase", json!({ "id": id, "transcript": t })) {
                Ok(_) => {
                    done = true;
                    report.erased += 1;
                    break;
                }
                Err(e @ ClientError::Tool(_)) => last = Some(e),
                Err(e) => {
                    fail(report, &format!("{name} cannot be erased now"), &e);
                    last = None;
                    break;
                }
            }
        }
        let e = state.entry(&id);
        if done {
            e["erased"] = json!(true);
        } else if let Some(err) = last.filter(|_| filed) {
            let n = e["erase_fails"].as_u64().unwrap_or(0) + 1;
            e["erase_fails"] = json!(n);
            if n >= MAX_ERASE_FAILS {
                e["erased"] = json!(true);
                report.problems.push(format!(
                    "{name} is kept: the memory refuses to erase it ({err})"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_recording_is_named_as_a_whole_word_and_a_long_text_is_cut_whole() {
        assert!(super::names("audio 1.m4a (file_x) : t", "1.m4a"));
        assert!(!super::names("audio 11.m4a : t", "1.m4a"));
        let text = format!("{} {}", "mot ".repeat(500), "x".repeat(250));
        let parts = super::split(&text, 100);
        assert!(parts.iter().all(|p| p.chars().count() <= 100));
        let squeeze = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(squeeze(&parts.concat()), squeeze(&text), "nothing lost");
    }
}
