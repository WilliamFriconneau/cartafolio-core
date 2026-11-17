//! One pass of synchronisation with a peer, from this node: for each space the peer's key reaches, pull the peer's
//! events after this node's cursor and apply them here, then push this node's events after the peer's cursor. The
//! cursors move only past what was dealt with, so a pass cut short resumes where it stopped; the same event twice is
//! one row (see `memcp_core::sync`). Over HTTP to the peer's `/sync/` routes, at the address of the connection.

use std::time::Duration;

use memcp_core::sync::{self, Cursor, Event};
use memcp_core::{Brain, Space};
use serde_json::{Value, json};

const PAGE: usize = 500;
const PAGE_BYTES: usize = 4 << 20;

/// What a pass did, per space.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
    pub spaces: usize,
    pub pulled: usize,
    pub pushed: usize,
    pub disputed: usize,
    /// Notes this replica removed to keep to its window (see `memcp_core::sync::prune_window`).
    pub pruned: usize,
    pub problems: Vec<String>,
}

pub struct Peer {
    /// The address of this node's connection on the peer: `https://…/mcp/<person>/<connection>`.
    address: String,
    key: String,
    /// The name this node keeps the peer under (its cursors and its status).
    name: String,
    agent: ureq::Agent,
}

impl Peer {
    pub fn new(address: &str, key: &str, name: &str) -> Peer {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();
        Peer {
            address: address.trim_end_matches('/').to_string(),
            key: key.to_string(),
            name: name.to_string(),
            agent,
        }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let mut res = self
            .agent
            .get(&format!("{}/sync/{path}", self.address))
            .header("Authorization", &format!("Bearer {}", self.key))
            .call()
            .map_err(|e| format!("unreachable: {e}"))?;
        read(&mut res)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let mut res = self
            .agent
            .post(&format!("{}/sync/{path}", self.address))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(|e| format!("unreachable: {e}"))?;
        read(&mut res)
    }

    /// One pass over every space the peer's key reaches.
    pub fn sync_once(&self, brain: &Brain, now: i64) -> Result<Pass, String> {
        // The person paused this machine: nothing is exchanged until they resume it.
        if brain
            .open_control()
            .is_ok_and(|c| memcp_core::peers::is_paused(&c, &self.name))
        {
            return Ok(Pass {
                problems: vec!["paused".into()],
                ..Pass::default()
            });
        }
        let listed = self.get("spaces")?;
        let theirs = listed["protocol"].as_i64().unwrap_or(1);
        if theirs > sync::PROTOCOL {
            return Err(format!(
                "the peer runs a newer exchange (protocol {theirs}, version {}): update this machine",
                listed["version"].as_str().unwrap_or("?")
            ));
        }
        let mut pass = Pass::default();
        for s in listed["spaces"].as_array().cloned().unwrap_or_default() {
            let Some(name) = s["space"].as_str() else {
                continue;
            };
            let Ok(space) = Space::new(name) else {
                continue;
            };
            sync::ensure_space(brain, &space, s["kind"].as_str().unwrap_or("custom"))
                .map_err(|e| e.to_string())?;
            pass.spaces += 1;
            if let Err(e) = self.pull(brain, &space, now, &mut pass) {
                pass.problems.push(format!("{name}: pull: {e}"));
            }
            if s["writable"].as_bool() == Some(true)
                && let Err(e) = self.push(brain, &space, now, &mut pass)
            {
                pass.problems.push(format!("{name}: push: {e}"));
            }
        }
        // A replica that holds a window drops what is outside it and acknowledged: local, nothing is sent.
        match sync::prune_all(brain, now) {
            Ok(Some(done)) => pass.pruned = done.iter().map(|p| p.facts).sum(),
            Ok(None) => {}
            Err(e) => pass.problems.push(format!("window: {e}")),
        }
        Ok(pass)
    }

    fn pull(&self, brain: &Brain, space: &Space, now: i64, pass: &mut Pass) -> Result<(), String> {
        // A replica that holds a window leaves out what is older than it (pinned notes excepted).
        let cutoff = sync::window_cutoff(brain, now).map_err(|e| e.to_string())?;
        loop {
            let (pulled, _) = sync::cursors(brain, &self.name, space).map_err(|e| e.to_string())?;
            let page = self.get(&format!(
                "events?space={}&after={pulled}&limit={PAGE}",
                space.as_str()
            ))?;
            let raw = page["events"].as_array().cloned().unwrap_or_default();
            let events: Vec<Event> = raw
                .iter()
                .filter_map(|v| Event::from_json(v).ok())
                .collect();
            let done = sync::apply_window(brain, space, &events, &self.name, cutoff)
                .map_err(|e| e.to_string())?;
            pass.pulled += done.applied;
            pass.disputed += done.disputed;
            let next = page["next"].as_i64().unwrap_or(pulled);
            let head = page["head"].as_i64().unwrap_or(next);
            // Past what was dealt with only: an event waiting for another stops the cursor there.
            let to = if done.stopped.is_some() {
                events
                    .get(done.consumed.saturating_sub(1))
                    .map_or(pulled, |e| e.seq.max(pulled))
            } else {
                next
            };
            sync::set_cursor(brain, &self.name, space, Cursor::Pulled, to)
                .map_err(|e| e.to_string())?;
            let _ = sync::note_transfer(brain, &self.name, space, "receiving", to, head, now);
            if done.stopped.is_some() || page["more"].as_bool() != Some(true) || to <= pulled {
                return Ok(());
            }
        }
    }

    fn push(&self, brain: &Brain, space: &Space, now: i64, pass: &mut Pass) -> Result<(), String> {
        loop {
            let (_, pushed) = sync::cursors(brain, &self.name, space).map_err(|e| e.to_string())?;
            let page = sync::events_after(brain, space, pushed, PAGE, PAGE_BYTES, Some(&self.name))
                .map_err(|e| e.to_string())?;
            if page.events.is_empty() {
                if page.next > pushed {
                    sync::set_cursor(brain, &self.name, space, Cursor::Pushed, page.next)
                        .map_err(|e| e.to_string())?;
                }
                let _ = sync::note_transfer(
                    brain, &self.name, space, "sending", page.head, page.head, now,
                );
                return Ok(());
            }
            let body = json!({
                "events": page.events.iter().map(Event::to_json).collect::<Vec<_>>(),
                "next": page.next,
                "head": page.head,
            });
            let answer = self.post(&format!("events?space={}", space.as_str()), &body)?;
            if let Some(e) = answer["error"].as_str() {
                return Err(e.to_string());
            }
            pass.pushed += answer["applied"].as_u64().unwrap_or(0) as usize;
            let consumed = answer["consumed"]
                .as_u64()
                .map_or(page.events.len(), |c| c as usize);
            let stopped = !answer["stopped"].is_null();
            let to = if stopped || consumed < page.events.len() {
                page.events
                    .get(consumed.saturating_sub(1))
                    .map_or(pushed, |e| e.seq.max(pushed))
            } else {
                page.next
            };
            sync::set_cursor(brain, &self.name, space, Cursor::Pushed, to)
                .map_err(|e| e.to_string())?;
            let _ = sync::note_transfer(brain, &self.name, space, "sending", to, page.head, now);
            if stopped || !page.more || to <= pushed {
                return Ok(());
            }
        }
    }
}

fn read(res: &mut ureq::http::Response<ureq::Body>) -> Result<Value, String> {
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| format!("unreadable: {e}"))?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(v)
    } else {
        Err(v["error"]
            .as_str()
            .map_or_else(|| format!("HTTP {status}"), String::from))
    }
}

/// A request to join another memory, as the asking machine holds it.
#[derive(Debug, Clone)]
pub struct JoinAsk {
    pub id: String,
    pub secret: String,
    /// The three words the person compares on both machines.
    pub check: String,
}

fn join_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// Asks the node at `server` to let this machine join (`POST /join`): a person there compares the three words and
/// answers. Nothing is joined until they do.
pub fn join_request(server: &str, device: &str) -> Result<JoinAsk, String> {
    let server = server.trim().trim_end_matches('/');
    let mut res = join_agent()
        .post(&format!("{server}/join"))
        .header("Content-Type", "application/json")
        .send(json!({ "device": device }).to_string().as_bytes())
        .map_err(|e| format!("unreachable: {e}"))?;
    let v = read(&mut res)?;
    let field = |k: &str| {
        v[k].as_str()
            .map(String::from)
            .ok_or_else(|| format!("no {k} in the answer"))
    };
    Ok(JoinAsk {
        id: field("id")?,
        secret: field("secret")?,
        check: field("check")?,
    })
}

/// Where the request stands: `{state: waiting | denied | expired}` or `{state: approved, join: {agent, key, mcp_url}}`.
pub fn join_poll(server: &str, ask: &JoinAsk) -> Result<Value, String> {
    let server = server.trim().trim_end_matches('/');
    let mut res = join_agent()
        .get(&format!("{server}/join/{}", ask.id))
        .header("X-Join-Secret", &ask.secret)
        .call()
        .map_err(|e| format!("unreachable: {e}"))?;
    read(&mut res)
}
