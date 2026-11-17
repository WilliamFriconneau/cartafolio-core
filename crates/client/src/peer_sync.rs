//! One pass of synchronisation with a peer, from this node: for each space the peer's key reaches, pull the peer's
//! events after this node's cursor and apply them here, then push this node's events after the peer's cursor. The
//! cursors move only past what was dealt with, so a pass cut short resumes where it stopped; the same event twice is
//! one row (see `memcp_core::sync`). Over HTTP to the peer's `/sync/` routes, at the address of the connection.
//!
//! Both ways for the spaces themselves (see `memcp_core::swarm`): a space of the person that the peer lacks is offered to
//! it (`POST /sync/spaces`) and then exchanged like the others; the person's Inbox here is paired with theirs
//! there, whatever its name; an archive on either side archives the other's copy. A peer of an older version, which
//! takes no offers, is kept in step as before, and the pass says what it could not give it.

use std::time::Duration;

use memcp_core::catalogue::TRIAGE;
use memcp_core::swarm::{self, Answer};
use memcp_core::sync::{self, Cursor, Event};
use memcp_core::{Brain, Space, directory, triage, users};
use serde_json::{Value, json};

const PAGE: usize = 500;
const PAGE_BYTES: usize = 4 << 20;

/// What a pass did, per space.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
    /// Spaces exchanged in this pass.
    pub spaces: usize,
    /// Events received from the peer and applied here.
    pub pulled: usize,
    /// Events of this node the peer took.
    pub pushed: usize,
    /// Events that touched a fact changed on both sides (kept, flagged for the person).
    pub disputed: usize,
    /// Notes this replica removed to keep to its window (see `memcp_core::sync::prune_window`).
    pub pruned: usize,
    /// Spaces of this node the peer did not have and made in this pass.
    pub offered: usize,
    /// Spaces archived in this pass because the other side archived them (here or there).
    pub archived: usize,
    /// What went wrong, in words, one line each; the pass goes on past them.
    pub problems: Vec<String>,
}

/// The peer this node keeps in step with, reached at the address of this node's own connection there.
pub struct Peer {
    /// The address of this node's connection on the peer: `https://…/mcp/<person>/<connection>`.
    address: String,
    key: String,
    /// The name this node keeps the peer under (its cursors and its status).
    name: String,
    /// The person of this node whose spaces are kept in step with the peer: the one named, else the person whose memory
    /// this node holds (`users::node_person`), found at each pass.
    person: Option<String>,
    agent: ureq::Agent,
}

impl Peer {
    /// `address`: this node's connection on the peer; `key`: its key; `name`: how this node keeps the peer.
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
            person: None,
            agent,
        }
    }

    /// The person of this node (a user id) whose spaces are kept in step with the peer: the spaces made here from the
    /// peer's are theirs, and theirs are the spaces offered to the peer. By default the person whose memory this node
    /// holds: the peer connection says whose memory it is on the peer, and that is the same human here, whatever their
    /// id or name on each node (see `users::node_person`).
    pub fn with_person(mut self, user_id: &str) -> Peer {
        self.person = Some(user_id.to_string());
        self
    }

    /// The person of this node the pass is for.
    fn person(&self, brain: &Brain) -> Result<String, String> {
        if let Some(p) = &self.person {
            return Ok(p.clone());
        }
        let ctl = brain.open_control().map_err(|e| e.to_string())?;
        users::node_person(&ctl)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "nobody on this node: no person to keep in step".to_string())
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

    /// One pass over every space the peer's key reaches. What it could not do is kept for the page and the apps
    /// (`sync::note_pass`), as well as returned.
    pub fn sync_once(&self, brain: &Brain, now: i64) -> Result<Pass, String> {
        let done = self.sync_inner(brain, now);
        let problems = match &done {
            Ok(p) => p.problems.clone(),
            Err(e) => vec![e.clone()],
        };
        let _ = sync::note_pass(brain, &self.name, now, &problems);
        done
    }

    fn sync_inner(&self, brain: &Brain, now: i64) -> Result<Pass, String> {
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
        // This node is active now: its own entry in the peer directory says so, at every pass.
        let _ = directory::touch_self(brain, now);
        let listed = self.get("spaces")?;
        let theirs = listed["protocol"].as_i64().unwrap_or(1);
        if theirs > sync::PROTOCOL {
            return Err(format!(
                "the peer runs a newer exchange (protocol {theirs}, version {}): update this machine",
                listed["version"].as_str().unwrap_or("?")
            ));
        }
        let mut pass = Pass::default();
        let person = self.person(brain)?;
        let pairs = self.spaces(brain, &person, &listed, now, &mut pass)?;
        // Every space is made before any is filled: a space's parent is then here when its layout arrives.
        for (local, remote, writable) in pairs {
            pass.spaces += 1;
            if let Err(e) = self.pull(brain, &local, &remote, now, &mut pass) {
                pass.problems.push(format!("{remote}: pull: {e}"));
            }
            if writable && let Err(e) = self.push(brain, &local, &remote, now, &mut pass) {
                pass.problems.push(format!("{remote}: push: {e}"));
            }
        }
        self.gossip(brain, now);
        // A replica that holds a window drops what is outside it and acknowledged: local, nothing is sent.
        match sync::prune_all(brain, now) {
            Ok(Some(done)) => pass.pruned = done.iter().map(|p| p.facts).sum(),
            Ok(None) => {}
            Err(e) => pass.problems.push(format!("window: {e}")),
        }
        Ok(pass)
    }

    /// Which spaces this pass exchanges: `(here, there, writable there)`. The peer's archives are applied here, the
    /// peer's spaces are made here, the person's Inbox is paired with the peer's, and the spaces of the person the
    /// peer lacks are offered to it.
    fn spaces(
        &self,
        brain: &Brain,
        person: &str,
        listed: &Value,
        now: i64,
        pass: &mut Pass,
    ) -> Result<Vec<(Space, String, bool)>, String> {
        let fail = |e: memcp_core::Error| e.to_string();
        let offers_ok = listed["offers"].as_bool() == Some(true);
        let entries = listed["spaces"].as_array().cloned().unwrap_or_default();
        // 1. What the peer archived is archived here (an archive, never a deletion), unless the copy here is newer.
        for a in listed["archived"].as_array().into_iter().flatten() {
            let (Some(Ok(space)), Some(at)) = (
                a["space"].as_str().map(Space::new),
                a["archived_at"].as_i64(),
            ) else {
                continue;
            };
            match swarm::archive_here(brain, person, &space, at) {
                Ok(true) => pass.archived += 1,
                Ok(false) => {}
                Err(e) => pass.problems.push(format!("{space}: archive: {e}")),
            }
        }
        // 2. The peer's spaces. Its Inbox (the one it names as the person's, else the first) goes with the box of
        // the person here, whatever the names; its other boxes are its own janitor's.
        let mine = triage::of(brain, person).map_err(fail)?;
        let theirs = entries
            .iter()
            .find(|s| s["triage"].as_bool() == Some(true))
            .or_else(|| entries.iter().find(|s| s["kind"].as_str() == Some(TRIAGE)))
            .and_then(|s| s["space"].as_str())
            .map(str::to_string);
        let mut pairs = vec![];
        let mut listed_names = std::collections::BTreeSet::new();
        for s in &entries {
            let Some(name) = s["space"].as_str() else {
                continue;
            };
            listed_names.insert(name.to_string());
            let Ok(space) = Space::new(name) else {
                continue;
            };
            let kind = s["kind"].as_str().unwrap_or("custom");
            let writable = s["writable"].as_bool() == Some(true);
            // An older peer does not say when its copy began: it counts as made now.
            let since = s["since"].as_i64();
            if kind == TRIAGE {
                if theirs.as_deref() != Some(name) {
                    continue;
                }
                let here = match &mine {
                    Some(t) => t.clone(),
                    None => {
                        if let Err(e) =
                            swarm::adopt(brain, person, &space, TRIAGE, since.unwrap_or(now))
                        {
                            pass.problems.push(format!("{name}: made here: {e}"));
                            continue;
                        }
                        space
                    }
                };
                pairs.push((here, name.to_string(), writable));
                continue;
            }
            // Archived here after the peer's copy began: the peer archives it too; it is not made again here.
            if !brain.has_space(&space).map_err(fail)?
                && let Some((_, at)) = swarm::archived_at(brain, &space).map_err(fail)?
                && since.unwrap_or(0) < at
            {
                if offers_ok {
                    let said = self.post("spaces", &json!({ "space": name, "archived_at": at }));
                    if let Err(e) = said {
                        pass.problems.push(format!("{name}: archive there: {e}"));
                    }
                } else {
                    pass.problems.push(format!(
                        "{name}: archived here; the peer runs an older version and keeps it"
                    ));
                }
                continue;
            }
            // One space that cannot be made here is said, and the others still go.
            if let Err(e) = swarm::adopt(brain, person, &space, kind, since.unwrap_or(now)) {
                pass.problems.push(format!("{name}: made here: {e}"));
                continue;
            }
            pairs.push((space, name.to_string(), writable));
        }
        // 3. The spaces of the person the peer lacks are offered to it. Inbox only when the peer has none.
        let offers: Vec<swarm::Offer> = swarm::offers(brain, person)
            .map_err(fail)?
            .into_iter()
            .filter(|o| !listed_names.contains(o.space.as_str()))
            .filter(|o| {
                if o.kind == TRIAGE {
                    theirs.is_none() && mine.as_ref() == Some(&o.space)
                } else {
                    true
                }
            })
            .collect();
        if offers.is_empty() {
            return Ok(pairs);
        }
        if !offers_ok {
            // A node of this version always lists its archives: then it is the key, not the version, that takes no offer.
            let why = if listed.get("archived").is_some() {
                "the key of this node there is not a peer of the person that reaches all their spaces, so it offers none"
            } else {
                "the peer runs an older version and takes no new space"
            };
            pass.problems.push(format!(
                "{why}: {} not offered ({})",
                offers.len(),
                offers
                    .iter()
                    .map(|o| o.space.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return Ok(pairs);
        }
        for o in offers {
            let answer = self
                .post("spaces", &o.to_json())
                .and_then(|v| Answer::from_json(&v).map_err(|e| e.to_string()));
            match answer {
                Ok(Answer::Created(there)) => {
                    pass.offered += 1;
                    pairs.push((o.space, there.as_str().to_string(), true));
                }
                Ok(Answer::Held(there) | Answer::Triage(there)) => {
                    pairs.push((o.space, there.as_str().to_string(), true));
                }
                Ok(Answer::Archived(at)) => {
                    match swarm::archive_here(brain, person, &o.space, at) {
                        Ok(true) => pass.archived += 1,
                        Ok(false) => {}
                        Err(e) => pass.problems.push(format!("{}: archive: {e}", o.space)),
                    }
                }
                Err(e) => pass
                    .problems
                    .push(format!("{}: offer refused: {e}", o.space)),
            }
        }
        Ok(pairs)
    }

    /// The peer directories, both ways: takes the peer's (`GET /sync/peers`), then gives this node's
    /// (`POST /sync/peers`). A peer without the endpoint (an older node) or a failure changes nothing: the
    /// directory is a courtesy, never a reason for a pass to fail.
    fn gossip(&self, brain: &Brain, now: i64) {
        // The name the peer goes by: the one it tells, else the one this node keeps it under.
        let mut asked = self.name.clone();
        if let Ok(theirs) = self.get("peers") {
            if let Some(n) = theirs["node"]["name"].as_str() {
                asked = n.to_string();
            }
            let list = theirs["peers"].as_array().cloned().unwrap_or_default();
            let _ = directory::merge(brain, &asked, &list, now);
            // The devices asking to join that node: the person may answer them from here too.
            let asking = theirs["node"]["pending"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let _ = directory::store_remote_joins(brain, &asked, &asking, now);
            // The peer's own entry is the one this connection reaches.
            if theirs["node"]["name"].is_string() {
                let _ = directory::merge(brain, &asked, std::slice::from_ref(&theirs["node"]), now);
                let _ = directory::set_link(brain, &asked, &self.name);
            }
        } else {
            return;
        }
        if let (Ok(node), Ok(peers)) = (directory::node_json(brain, now), directory::to_wire(brain))
        {
            // The answers the person gave here to requests of that node go back with this exchange.
            let answers = directory::answers_for(brain, &asked).unwrap_or_default();
            let sent = self.post(
                "peers",
                &json!({ "node": node, "peers": peers, "answers": answers }),
            );
            if sent.is_ok() && !answers.is_empty() {
                let _ = directory::answers_delivered(brain, &asked);
            }
        }
    }

    /// The peer's events of `remote` after this node's cursor, applied to `space` here.
    fn pull(
        &self,
        brain: &Brain,
        space: &Space,
        remote: &str,
        now: i64,
        pass: &mut Pass,
    ) -> Result<(), String> {
        // A replica that holds a window leaves out what is older than it (pinned notes excepted).
        let cutoff = sync::window_cutoff(brain, now).map_err(|e| e.to_string())?;
        loop {
            let (pulled, _) = sync::cursors(brain, &self.name, space).map_err(|e| e.to_string())?;
            let page = self.get(&format!(
                "events?space={remote}&after={pulled}&limit={PAGE}"
            ))?;
            let raw = page["events"].as_array().cloned().unwrap_or_default();
            let events: Vec<Event> = raw
                .iter()
                .filter_map(|v| Event::from_json(v).ok())
                .collect();
            // An event this node cannot read is passed over like one it refuses, never in silence.
            if events.len() < raw.len() {
                pass.problems.push(format!(
                    "{}: pull: {} unreadable event(s) passed over",
                    space.as_str(),
                    raw.len() - events.len()
                ));
            }
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

    /// This node's events of `space` after the peer's cursor, given to the peer's `remote`.
    fn push(
        &self,
        brain: &Brain,
        space: &Space,
        remote: &str,
        now: i64,
        pass: &mut Pass,
    ) -> Result<(), String> {
        loop {
            let (_, pushed) = sync::cursors(brain, &self.name, space).map_err(|e| e.to_string())?;
            let page = sync::events_after(brain, space, pushed, PAGE, PAGE_BYTES, Some(&self.name))
                .map_err(|e| e.to_string())?;
            if page.events.is_empty() {
                if page.next > pushed {
                    sync::set_cursor(brain, &self.name, space, Cursor::Pushed, page.next)
                        .map_err(|e| e.to_string())?;
                }
                // A page of events that came from this peer (none to send back) is not the end: after a large copy
                // from it, this node's own notes wait further on.
                if page.more && page.next > pushed {
                    continue;
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
            let answer = self.post(&format!("events?space={remote}"), &body)?;
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
    /// The request's id on the other node (`GET /join/<id>`).
    pub id: String,
    /// The secret that proves this machine asked (`X-Join-Secret`).
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
    join_request_with(server, device, None, None)
}

/// [`join_request`] with a one-time `ticket` from a node of the same person (no words to compare then) and the id of
/// the asking node, which the person sees beside the three words.
pub fn join_request_with(
    server: &str,
    device: &str,
    ticket: Option<&str>,
    node_id: Option<&str>,
) -> Result<JoinAsk, String> {
    let server = server.trim().trim_end_matches('/');
    let mut body = json!({ "device": device });
    if let Some(t) = ticket {
        body["ticket"] = json!(t);
    }
    if let Some(i) = node_id {
        body["node_id"] = json!(i);
    }
    let mut res = join_agent()
        .post(&format!("{server}/join"))
        .header("Content-Type", "application/json")
        .send(body.to_string().as_bytes())
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

/// Who answers at `server` (`GET /.well-known/memcp`): its name and, for a node of this version, its id.
pub fn who_is_there(server: &str) -> Result<Value, String> {
    let server = server.trim().trim_end_matches('/');
    let mut res = join_agent()
        .get(&format!("{server}/.well-known/memcp"))
        .call()
        .map_err(|e| format!("unreachable: {e}"))?;
    let v = read(&mut res)?;
    if v["service"] != "memcp" {
        return Err("this is not a MemCP node".into());
    }
    Ok(v)
}
