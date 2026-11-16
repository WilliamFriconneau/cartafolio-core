//! The browser extension's link: in a call in the browser (Google Meet, Teams on the web), the extension reads who is
//! there and who speaks, and tells the app on this machine (`POST http://127.0.0.1:8790/extension`). The core keeps
//! what it heard for a day; when a meeting ends, [`roster`] says who was there and when each one spoke, as text put
//! before the transcript.
//!
//! Only the extension is heard: the `Origin` must be its own (`chrome-extension://` and the id fixed by the key in its
//! manifest) and the `Host` this machine. A web page cannot write here, nor read anything: the answer says nothing but
//! that the app is there.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::Error;

/// The extension's id, fixed by the public key in its manifest (`apps/chrome-extension/manifest.json`).
pub const EXTENSION_ID: &str = "ikhhhehjfmhdobidogcbommdklfmnnfg";
/// Where the app listens for it.
pub const EXTENSION_PORT: u16 = 8790;

const BODY_MAX: u64 = 256 * 1024;
const NAME_MAX: usize = 120;
const EVENTS_PER_REPORT: usize = 2000;
/// What is kept: a day, and at most this many events.
const KEEP_MS: i64 = 24 * 3600 * 1000;
const EVENTS_MAX: usize = 100_000;
/// A report this recent: the extension is there, and a call it reports is on.
pub const ALIVE_MS: i64 = 90 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Joined,
    Left,
    Speaking,
    Silent,
}

/// Something the extension saw, at `at` (Unix time in milliseconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub at: i64,
    pub name: String,
    pub kind: Kind,
}

/// The call the extension last reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// `meet` or `teams`.
    pub platform: String,
    pub title: String,
    /// Where the call is: the Meet code (`/abc-defg-hij`) or, for Teams, its title. Another room in the same browser
    /// is another meeting, even when the browser never lets the microphone go (13/11).
    pub room: String,
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    last_seen: i64,
    call: Option<(Call, i64)>,
    present: BTreeSet<String>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A name as it may be kept: no control characters, spaces made single, not too long; `None` when nothing is left.
fn clean_name(raw: &str) -> Option<String> {
    let flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(NAME_MAX).collect();
    (!name.is_empty() && !icon_word(&name)).then_some(name)
}

/// The name of one of Meet's icons, read where a name was looked for (« frame_person », « keep_off », « devices »):
/// lower case letters and underscores only, as no person's name is written there.
fn icon_word(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

impl State {
    /// Takes one report of the extension, received at `now`. Its events are dated by the browser, on this same machine;
    /// one dated in the future is dated now.
    fn take(&mut self, report: &Value, now: i64) {
        self.last_seen = now;
        let in_call = report["in_call"].as_bool().unwrap_or(false);
        let mut events: Vec<Event> = report["events"]
            .as_array()
            .map(|a| a.iter().take(EVENTS_PER_REPORT).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| {
                let kind = match e["kind"].as_str()? {
                    "joined" => Kind::Joined,
                    "left" => Kind::Left,
                    "speaking" => Kind::Speaking,
                    "silent" => Kind::Silent,
                    _ => return None,
                };
                let at = e["at"].as_i64().unwrap_or(now).min(now);
                Some(Event {
                    at,
                    name: clean_name(e["name"].as_str()?)?,
                    kind,
                })
            })
            .collect();
        // The list of those there, sent with each report: anyone missed (a report lost) joins now, anyone gone leaves.
        if let Some(list) = report["participants"].as_array() {
            let seen: BTreeSet<String> = list
                .iter()
                .filter_map(|n| n.as_str().and_then(clean_name))
                .collect();
            for name in seen.difference(&self.present) {
                if !events
                    .iter()
                    .any(|e| &e.name == name && e.kind == Kind::Joined)
                {
                    events.push(Event {
                        at: now,
                        name: name.clone(),
                        kind: Kind::Joined,
                    });
                }
            }
            for name in self.present.difference(&seen) {
                if !events
                    .iter()
                    .any(|e| &e.name == name && e.kind == Kind::Left)
                {
                    events.push(Event {
                        at: now,
                        name: name.clone(),
                        kind: Kind::Left,
                    });
                }
            }
        }
        if !in_call {
            for name in &self.present {
                events.push(Event {
                    at: now,
                    name: name.clone(),
                    kind: Kind::Left,
                });
            }
        }
        for e in &events {
            match e.kind {
                Kind::Joined | Kind::Speaking => {
                    self.present.insert(e.name.clone());
                }
                Kind::Left => {
                    self.present.remove(&e.name);
                }
                Kind::Silent => {}
            }
        }
        if !in_call {
            self.present.clear();
        }
        self.events.extend(events);
        self.events.sort_by_key(|e| e.at);
        self.events.retain(|e| now - e.at < KEEP_MS);
        if self.events.len() > EVENTS_MAX {
            let over = self.events.len() - EVENTS_MAX;
            self.events.drain(..over);
        }
        self.call = in_call.then(|| {
            (
                Call {
                    platform: report["platform"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(20)
                        .collect(),
                    title: report["title"]
                        .as_str()
                        .and_then(clean_name)
                        .unwrap_or_default(),
                    room: report["room"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(200)
                        .collect(),
                },
                now,
            )
        });
    }
}

/// Who was there between `from` and `to` (Unix milliseconds), and when each one spoke, as text in French:
/// empty when the extension saw nothing then.
pub fn roster(events: &[Event], from: i64, to: i64) -> String {
    fn add<'a>(list: &mut Vec<&'a str>, name: &'a str) {
        if !list.contains(&name) {
            list.push(name);
        }
    }
    // Those there, in the order they came; those of the meeting; who speaks since when; the turns.
    let mut present: Vec<&str> = Vec::new();
    let mut there: Vec<&str> = Vec::new();
    let mut speaking: BTreeMap<&str, i64> = BTreeMap::new();
    let mut turns: Vec<(i64, i64, &str)> = Vec::new();
    let mut started = false;
    for e in events.iter().filter(|e| e.at <= to) {
        let name = e.name.as_str();
        if !started && e.at >= from {
            started = true;
            for p in &present {
                add(&mut there, p);
            }
        }
        match e.kind {
            Kind::Joined | Kind::Speaking => {
                add(&mut present, name);
                if started {
                    add(&mut there, name);
                }
                if e.kind == Kind::Speaking {
                    speaking.entry(name).or_insert(e.at);
                }
            }
            Kind::Left | Kind::Silent => {
                if e.kind == Kind::Left {
                    present.retain(|p| *p != name);
                }
                if let Some(start) = speaking.remove(name).filter(|_| e.at > from) {
                    turns.push((start.max(from), e.at, name));
                }
            }
        }
    }
    if !started {
        there = present;
    }
    for (name, start) in speaking {
        turns.push((start.max(from), to, name));
    }
    if there.is_empty() && turns.is_empty() {
        return String::new();
    }
    turns.sort_by_key(|t| t.0);
    // The same voice again within three seconds is the same turn; under a second is noise.
    let mut merged: Vec<(i64, i64, &str)> = Vec::new();
    for t in turns {
        match merged.last_mut() {
            Some(last) if last.2 == t.2 && t.0 - last.1 <= 3000 => last.1 = last.1.max(t.1),
            _ => merged.push(t),
        }
    }
    merged.retain(|t| t.1 - t.0 >= 1000);
    let clock = |ms: i64| {
        let s = (ms - from).max(0) / 1000;
        if to - from >= 3_600_000 {
            format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        } else {
            format!("{:02}:{:02}", s / 60, s % 60)
        }
    };
    let mut out = format!("Participants : {}\n", there.join(", "));
    if !merged.is_empty() {
        out.push_str("\nPrises de parole :\n");
        for (start, end, name) in merged.iter().take(600) {
            out.push_str(&format!("{}–{} {name}\n", clock(*start), clock(*end)));
        }
    }
    out
}

fn header(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn reply(request: tiny_http::Request, status: u16, body: &Value) {
    let mut r = tiny_http::Response::from_string(body.to_string()).with_status_code(status);
    if let Ok(h) = tiny_http::Header::from_bytes("Content-Type", "application/json") {
        r = r.with_header(h);
    }
    let _ = request.respond(r);
}

/// The link with the extension, on a thread of its own. Stops when dropped.
pub struct BrowserLink {
    server: Option<Arc<tiny_http::Server>>,
    thread: Option<JoinHandle<()>>,
    state: Arc<Mutex<State>>,
}

impl BrowserLink {
    /// Listens on `127.0.0.1:<port>` before returning: a port already taken is said at once.
    pub fn start(port: u16) -> Result<BrowserLink, Error> {
        let server = Arc::new(
            tiny_http::Server::http(("127.0.0.1", port))
                .map_err(|e| Error::Io(format!("127.0.0.1:{port}: {e}")))?,
        );
        let state = Arc::new(Mutex::new(State::default()));
        let (serving, shared) = (Arc::clone(&server), Arc::clone(&state));
        let thread = std::thread::Builder::new()
            .name("memcp-bridge-browser".into())
            .spawn(move || serve(&serving, &shared))
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(BrowserLink {
            server: Some(server),
            thread: Some(thread),
            state,
        })
    }

    /// When the extension last said something (Unix milliseconds), 0 if never since the app started.
    pub fn last_seen(&self) -> i64 {
        self.state.lock().map_or(0, |s| s.last_seen)
    }

    /// The call in the browser, if one is on now.
    pub fn call(&self) -> Option<Call> {
        let s = self.state.lock().ok()?;
        s.call
            .as_ref()
            .filter(|(_, at)| now_ms() - at < ALIVE_MS)
            .map(|(c, _)| c.clone())
    }

    /// [`roster`] of what the extension saw between `from` and `to` (Unix milliseconds).
    pub fn roster(&self, from: i64, to: i64) -> String {
        self.state
            .lock()
            .map(|s| roster(&s.events, from, to))
            .unwrap_or_default()
    }

    pub fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.unblock();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for BrowserLink {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(server: &tiny_http::Server, state: &Mutex<State>) {
    let ours = format!("chrome-extension://{EXTENSION_ID}");
    for mut request in server.incoming_requests() {
        let host_ok = header(&request, "host").is_some_and(|h| {
            let name = h.rsplit_once(':').map_or(h.as_str(), |(n, _)| n);
            matches!(name, "127.0.0.1" | "localhost")
        });
        let origin = header(&request, "origin");
        let path = request.url().split('?').next().unwrap_or("").to_string();
        if !host_ok || path != "/extension" {
            reply(request, 403, &json!({ "error": "refused" }));
            continue;
        }
        match request.method() {
            // Is the app there: the extension's popup asks, and gets no more than that. Asked by the extension, it
            // also says the extension is there, so the app and the popup never disagree.
            tiny_http::Method::Get if origin.as_deref().is_none_or(|o| o == ours) => {
                if origin.is_some()
                    && let Ok(mut s) = state.lock()
                {
                    s.last_seen = now_ms();
                }
                reply(request, 200, &json!({ "app": "memcp-bridge" }));
            }
            tiny_http::Method::Post if origin.as_deref() == Some(ours.as_str()) => {
                let mut body = String::new();
                if request
                    .as_reader()
                    .take(BODY_MAX + 1)
                    .read_to_string(&mut body)
                    .is_err()
                    || body.len() as u64 > BODY_MAX
                {
                    reply(request, 413, &json!({ "error": "too big" }));
                    continue;
                }
                let Ok(report) = serde_json::from_str::<Value>(&body) else {
                    reply(request, 400, &json!({ "error": "not JSON" }));
                    continue;
                };
                if let Ok(mut s) = state.lock() {
                    s.take(&report, now_ms());
                }
                reply(request, 200, &json!({ "ok": true }));
            }
            _ => reply(request, 403, &json!({ "error": "refused" })),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn ev(at: i64, name: &str, kind: Kind) -> Event {
        Event {
            at,
            name: name.into(),
            kind,
        }
    }

    /// Who was there and the turns, from the meeting's start, merged and cleaned of noise.
    #[test]
    fn what_meet_names_its_icons_is_no_one() {
        for icon in [
            "frame_person",
            "devices",
            "keep_off",
            "smart_toy",
            "sticker",
        ] {
            assert_eq!(super::clean_name(icon), None, "{icon}");
        }
        for name in [
            "Léa",
            "Marie Dupont",
            "f.picard@example.org",
            "Anne Marie DURAND",
        ] {
            assert_eq!(super::clean_name(name).as_deref(), Some(name));
        }
    }

    #[test]
    fn roster_says_who_was_there_and_when_each_spoke() {
        let events = vec![
            ev(0, "Marie Dupont", Kind::Joined),
            ev(500, "Paul Martin", Kind::Joined),
            ev(10_000, "Marie Dupont", Kind::Speaking),
            ev(20_000, "Marie Dupont", Kind::Silent),
            ev(21_000, "Marie Dupont", Kind::Speaking),
            ev(30_000, "Marie Dupont", Kind::Silent),
            ev(31_000, "Paul Martin", Kind::Speaking),
            ev(31_500, "Paul Martin", Kind::Silent),
            ev(65_000, "Paul Martin", Kind::Speaking),
            ev(80_000, "Léa", Kind::Joined),
        ];
        let text = roster(&events, 5_000, 90_000);
        assert_eq!(
            text,
            "Participants : Marie Dupont, Paul Martin, Léa\n\nPrises de parole :\n00:05–00:25 Marie Dupont\n01:00–01:25 Paul Martin\n"
        );
    }

    #[test]
    fn nothing_seen_gives_nothing() {
        assert_eq!(roster(&[], 0, 1000), "");
        let before = vec![ev(0, "Marie", Kind::Joined), ev(10, "Marie", Kind::Left)];
        assert_eq!(roster(&before, 1000, 2000), "");
    }

    /// The list sent with each report makes up for a lost one; a report out of a call ends everyone's presence.
    #[test]
    fn the_list_of_participants_corrects_what_was_missed() {
        let mut s = State::default();
        s.take(
            &json!({"in_call": true, "platform": "meet", "participants": ["Marie", "Paul"], "events": []}),
            1000,
        );
        s.take(
            &json!({"in_call": true, "platform": "meet", "participants": ["Paul"], "events": [
                {"at": 1500, "name": "Paul", "kind": "speaking"}
            ]}),
            2000,
        );
        assert_eq!(s.present, BTreeSet::from(["Paul".to_string()]));
        assert!(s.events.contains(&ev(2000, "Marie", Kind::Left)));
        s.take(&json!({"in_call": false, "events": []}), 3000);
        assert!(s.present.is_empty());
        assert!(s.call.is_none());
        assert_eq!(
            roster(&s.events, 0, 4000),
            "Participants : Marie, Paul\n\nPrises de parole :\n00:01–00:03 Paul\n"
        );
    }

    #[test]
    fn names_are_cleaned_and_bounded() {
        assert_eq!(
            clean_name("  Marie\n\tDupont "),
            Some("Marie Dupont".into())
        );
        assert_eq!(clean_name(" \u{7} "), None);
        assert_eq!(
            clean_name(&"A".repeat(500)).map(|n| n.len()),
            Some(NAME_MAX)
        );
        let mut s = State::default();
        s.take(
            &json!({"in_call": true, "events": [{"at": 99_999, "name": "Futur", "kind": "joined"}]}),
            1000,
        );
        assert_eq!(s.events[0].at, 1000);
    }

    fn send(port: u16, method: &str, origin: Option<&str>, body: &str) -> String {
        use std::io::Write;
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
        write!(
            c,
            "{method} /extension HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write");
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    /// The extension is heard; a web page, or the extension's id faked by another, is not.
    #[test]
    fn only_the_extension_is_heard() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture");
        let port = probe.local_addr().expect("fixture").port();
        drop(probe);
        let mut link = BrowserLink::start(port).expect("start");
        let ours = format!("chrome-extension://{EXTENSION_ID}");
        let report = r#"{"in_call":true,"platform":"meet","title":"Point","room":"/abc-defg-hij","participants":["Marie"],"events":[]}"#;
        assert!(
            send(port, "POST", Some("https://evil.example"), report).starts_with("HTTP/1.1 403")
        );
        assert!(
            send(
                port,
                "POST",
                Some("chrome-extension://abcdefghijklmnopabcdefghijklmnop"),
                report
            )
            .starts_with("HTTP/1.1 403")
        );
        assert!(send(port, "POST", None, report).starts_with("HTTP/1.1 403"));
        assert!(send(port, "GET", Some("https://evil.example"), "").starts_with("HTTP/1.1 403"));
        assert_eq!(link.last_seen(), 0);
        assert!(send(port, "GET", None, "").starts_with("HTTP/1.1 200"));
        assert_eq!(link.last_seen(), 0);
        assert!(send(port, "GET", Some(&ours), "").starts_with("HTTP/1.1 200"));
        assert!(link.last_seen() > 0);
        assert!(send(port, "POST", Some(&ours), report).starts_with("HTTP/1.1 200"));
        assert_eq!(
            link.call(),
            Some(Call {
                platform: "meet".into(),
                title: "Point".into(),
                room: "/abc-defg-hij".into()
            })
        );
        assert!(
            link.roster(0, now_ms() + 1)
                .starts_with("Participants : Marie")
        );
        link.stop();
        // The listener closes as the thread ends: give it a moment, never assume it is instant.
        let closed = (0..60).any(|_| {
            let refused = std::net::TcpStream::connect(("127.0.0.1", port)).is_err();
            if !refused {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            refused
        });
        assert!(closed, "the port is still listening after stop");
    }
}
