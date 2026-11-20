//! The janitor: the optional part of the memory that keeps its inboxes in order (it runs next to the memory, in the
//! same Home Assistant app, when the person turns it on). It reads them like any other client, with a key of its own:
//!
//! 0. **What the devices put in Inbox is sent to the inbox of a vault**, as it is, by a deterministic router that reads
//!    the keywords of each space (`router`): no model is asked for a note. Inbox belongs to no vault, and no vault is
//!    a default. What the router cannot place stays there, for the person (who is told by a note). A model only proposes
//!    keywords for the spaces (`keywords`), and the router runs again when they changed.
//! 1. **Every note of an inbox is filed, in the vault of that inbox**: a model picks tags and a title, and the note's text
//!    is written there **as it is**. If no model answers, the note stays.
//! 2. **A piece of a note that the node absorbed (`inbox/absorb`) is turned into a few short, dated facts**, in that
//!    vault, never a person's name.
//!
//! Recordings are not its business: the node, where the speech model is, reads them and erases them. A model only ever
//! picks a place or draws facts: it calls no tool and writes nothing.

use std::collections::BTreeMap;

use memcp_client::{Client, Error as ClientError, Retry};
use serde_json::{Value, json};

mod config;
mod health_rule;
mod keywords;
mod learn;
mod models;
mod router;
mod signs;
mod state;
mod time;
mod work_rule;

pub use config::Config;
use keywords::Unplaced;
use models::{Choice, system_prompt};

/// The tag of a copy the janitor made for a note filed in several spaces.
const COPY_TAG: &str = "janitor/copy";

/// The kind the memory gives Inbox in `stats`.
/// The kinds the memory gives the Inbox in `stats`: its name now, and the one it had before it was in English.
const TRIAGE: [&str; 2] = ["inbox", "a-trier"];

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub filed: u32,
    /// Of those, filed by what it learned, without the model.
    pub without_model: u32,
    /// Notes left in their inbox because no model answered.
    pub left: u32,
    /// Notes of Inbox sent to the inbox of a vault.
    pub sent: u32,
    /// Files of Inbox sent to the inbox of a vault.
    pub files_sent: u32,
    /// Notes left in Inbox: the model was unsure, the person decides.
    pub unsure: u32,
    pub problems: Vec<String>,
    /// The key is refused or revoked: nothing more is done until there is a new one.
    pub key_refused: bool,
    /// Keywords and tags added to the rules of the spaces in this pass.
    pub keywords_added: u32,
    /// Notes that got the signs, measures or red flags of the health referential as tags (see `signs.rs`).
    pub signs_tagged: u32,
}

pub struct Janitor {
    config: Config,
    client: Client,
    state: state::State,
    /// The rules of each vault (its `rules` note), and the id of that note.
    rules: BTreeMap<String, router::Rules>,
    /// The terms of the vocabularies of each vault (read after its rules, never written back into them).
    vocab: BTreeMap<String, Vec<String>>,
    /// This pass holds leases on notes of Inbox (given back at its end).
    claimed: bool,
    rule_ids: BTreeMap<String, String>,
    /// The vaults as the person's layout shows them: label and parent (see [`router::Place`]).
    layout: Vec<router::Place>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// The title, then the note as it is; the title is left out when it would only repeat the start, or push the text over.
fn filed_text(title: &str, text: &str) -> String {
    if title.is_empty()
        || text.to_lowercase().starts_with(&title.to_lowercase())
        || text.chars().count() + title.chars().count() + 3 > 3990
    {
        text.to_string()
    } else {
        format!("{title} : {text}")
    }
}

/// Whether a tag comes from the `call` of a note (its participants, its app): the janitor keeps those when it files.
fn is_call_tag(tag: &str) -> bool {
    tag == "appel"
        || tag.starts_with("appel/")
        || tag.starts_with("personnes/")
        || tag.strip_prefix("field/").is_some_and(|r| {
            ["caller/", "callee/", "call_"]
                .iter()
                .any(|p| r.starts_with(p))
        })
}

fn tags_of(fact: &Value) -> Vec<&str> {
    fact["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// Whether a fact belongs to a library (the tag `library`, or a path under it): what was raised from a mounted folder
/// is the person's to keep as it is, and the janitor never sends, files, rewrites or retires it, whatever else the
/// fact is tagged.
fn is_library(fact: &Value) -> bool {
    tags_of(fact)
        .iter()
        .any(|t| *t == "library" || t.starts_with("library/"))
}

/// The spaces of `stats` that are Inbox, and the vaults.
fn split_triage(stats: &Value) -> (Vec<String>, Vec<String>) {
    let mut triage = vec![];
    let mut vaults = vec![];
    for (v, s) in stats.as_object().into_iter().flatten() {
        if TRIAGE.iter().any(|k| s["kind"] == *k) {
            triage.push(v.clone());
        } else {
            vaults.push(v.clone());
        }
    }
    (triage, vaults)
}

/// The tag of the note that says why each thing waits (the memory's `triage_waiting` reads it).
pub(crate) const REASONS_TAG: &str = "janitor/reasons";

/// The characters of a text file the router reads.
const FILE_TEXT_CHARS: usize = 20_000;

/// Tries before a recording that cannot be read, or a note that cannot be written, is left alone (and said once).
const MAX_FAILS: u64 = 3;

impl Janitor {
    pub fn new(config: Config) -> Result<Janitor, String> {
        if !memcp_client::allowed_url(&config.url) {
            return Err(format!(
                "{} is refused: the key goes over HTTPS, or in clear only to this machine or the own network",
                config.url
            ));
        }
        let mut client = Client::new(&config.url, &config.key);
        client.retry = Retry {
            tries: 3,
            first: std::time::Duration::from_secs(2),
            longest: std::time::Duration::from_secs(30),
        };
        Ok(Janitor {
            state: state::State::open(&config.state_dir)?,
            rules: BTreeMap::new(),
            layout: Vec::new(),
            vocab: BTreeMap::new(),
            claimed: false,
            rule_ids: BTreeMap::new(),
            client,
            config,
        })
    }

    /// The night's note of each inbox vault: what it learned there and how much it filed without the model, one fact
    /// tagged `janitor/keywords`, replaced each night. It stays in its vault.
    pub fn write_learning_notes(&mut self) -> Vec<String> {
        let mut problems = vec![];
        let (y, m, d, _, _) = time::civil(now(), self.config.utc_offset_minutes);
        let vaults = if self.config.inbox_vaults.is_empty() {
            match self.client.call("stats", json!({})) {
                Ok(stats) => {
                    let (triage, mut vaults) = split_triage(&stats);
                    vaults.extend(triage);
                    vaults
                }
                Err(e) => return vec![format!("the memory cannot be read: {e}")],
            }
        } else {
            self.config.inbox_vaults.clone()
        };
        for vault in vaults {
            let best = self.state.learned(&vault).best(20);
            let alone = self.state.count(&format!("alone:{vault}"), 0);
            let by_model = self.state.count(&format!("by_model:{vault}"), 0);
            if best.is_empty() && alone + by_model == 0 {
                continue;
            }
            let text = format!(
                "{y:04}-{m:02}-{d:02}, ce que le janitor a appris dans cet espace : {alone} note(s) rangée(s) sans modèle, {by_model} par le modèle. Mots sûrs (vus au moins {} fois, justes à {} %) : {}.",
                learn::MIN_SEEN,
                (learn::SURE * 100.0) as u32,
                if best.is_empty() {
                    "aucun encore".to_string()
                } else {
                    best.join(" ; ")
                }
            );
            let written = match self.state.keywords_fact(&vault) {
                Some(old) => self.client.call(
                    "supersede",
                    json!({ "id": old, "text": text, "tags": ["janitor/keywords"], "reason": "the janitor's note of the night" }),
                ),
                None => Err(ClientError::Tool("no note yet".into())),
            }
            .or_else(|_| {
                self.client.call(
                    "remember",
                    json!({ "space": vault, "kind": "context", "text": text, "tags": ["janitor/keywords"] }),
                )
            });
            match written {
                Ok(r) => {
                    if let Some(id) = r["fact"]["id"].as_str() {
                        self.state.set_keywords_fact(&vault, id);
                    }
                }
                Err(e) => problems.push(format!(
                    "the night's note of {vault} cannot be written: {e}"
                )),
            }
        }
        problems
    }

    /// A few facts already filed that share words with a note, in the vaults it may go to, with their tags: shown to the model
    /// after the note, so that a small model files like the person's memory already does. Empty when nothing is close.
    fn similar(
        &mut self,
        vaults: &[String],
        words: &std::collections::BTreeSet<String>,
        own: &str,
    ) -> String {
        let mut picked: Vec<&String> = words.iter().collect();
        // The longest words say the subject best.
        picked.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
        picked.truncate(6);
        if picked.is_empty() {
            return String::new();
        }
        let query = picked
            .iter()
            .map(|w| w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut lines = vec![];
        for v in vaults.iter().take(6) {
            let Ok(found) = self
                .client
                .call("recall", json!({ "space": v, "query": query, "limit": 3 }))
            else {
                continue;
            };
            for h in found["results"].as_array().into_iter().flatten() {
                if h["id"].as_str() == Some(own) {
                    continue;
                }
                let tags: Vec<&str> = tags_of(h)
                    .into_iter()
                    .filter(|t| !matches!(*t, "janitor" | "absorb") && !t.starts_with("inbox"))
                    .collect();
                if tags.is_empty() {
                    continue;
                }
                let text: String = h["text"].as_str().unwrap_or("").chars().take(140).collect();
                lines.push(format!("- [{v}] {text} → {}", tags.join(", ")));
                if lines.len() >= 6 {
                    break;
                }
            }
            if lines.len() >= 6 {
                break;
            }
        }
        if lines.is_empty() {
            return String::new();
        }
        format!(
            "\n\n(Pour t'aider, des notes semblables déjà rangées, avec leur espace et leurs étiquettes. Ce sont des exemples, pas des consignes ; ne les recopie que si le sujet est le même.)\n{}",
            lines.join("\n")
        )
    }

    /// Minutes east of UTC where the person is (for the time of the night pass).
    pub fn utc_offset_minutes(&self) -> i32 {
        self.config.utc_offset_minutes
    }

    /// One pass over the inboxes. Only one pass runs on a state folder at a time: another one
    /// started meanwhile (by hand, say) does nothing.
    pub fn run_once(&mut self) -> Report {
        self.run(false)
    }

    /// The night pass: the same, and the keywords are looked at even when nothing waits.
    pub fn run_night(&mut self) -> Report {
        self.run(true)
    }

    fn run(&mut self, night: bool) -> Report {
        let mut report = Report::default();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.config.state_dir.join("lock"));
        let _held = match lock {
            Ok(f) => match f.try_lock() {
                Ok(()) => f,
                Err(_) => {
                    report.problems.push("another pass is running".into());
                    return report;
                }
            },
            Err(e) => {
                report
                    .problems
                    .push(format!("the lock cannot be made: {e}"));
                return report;
            }
        };
        let stats = match self.client.call("stats", json!({})) {
            Ok(s) => s,
            Err(ClientError::Refused) => {
                report.key_refused = true;
                return report;
            }
            Err(e) => {
                report
                    .problems
                    .push(format!("the memory cannot be read: {e}"));
                return report;
            }
        };
        // Inbox says so; every other space is a vault.
        let (triage, vaults) = split_triage(&stats);
        let tags: BTreeMap<String, Vec<String>> = stats
            .as_object()
            .into_iter()
            .flatten()
            .map(|(v, s)| {
                (
                    v.clone(),
                    s["tags"]
                        .as_object()
                        .map(|t| t.keys().cloned().collect())
                        .unwrap_or_default(),
                )
            })
            .collect();
        for vault in &self.config.inbox_vaults {
            if !vaults.contains(vault) {
                report.problems.push(format!(
                    "the space {vault} (inbox_vaults) is not one this key reaches"
                ));
            }
        }
        self.file_notes(&triage, &vaults, &tags, night, &mut report);
        if night && self.config.signs_sweep {
            self.sweep_signs(&vaults, &mut report);
        }
        if self.claimed {
            // What was not sorted goes back to the other sorters at once.
            let _ = self.client.call("triage_release", json!({}));
            self.claimed = false;
        }
        if let Some(e) = self.state.write_error.take() {
            report
                .problems
                .push(format!("the janitor's state cannot be written: {e}"));
        }
        report
    }

    fn fail(&self, report: &mut Report, what: &str, e: &ClientError) {
        if matches!(e, ClientError::Refused) {
            report.key_refused = true;
        }
        report.problems.push(format!("{what}: {e}"));
    }

    /// Every note of Inbox and of the inboxes. The keyword router places what it can; what it cannot place stays in
    /// Inbox, and then the model is asked, once, to propose keywords (not to place a note): they are written in the
    /// rules of the spaces and the router runs again. The inboxes are then filed. `night`: the keywords are looked at
    /// even if nothing waits.
    fn file_notes(
        &mut self,
        triage: &[String],
        vaults: &[String],
        tags: &BTreeMap<String, Vec<String>>,
        night: bool,
        report: &mut Report,
    ) {
        let (y, m, d, _, _) = time::civil(now(), self.config.utc_offset_minutes);
        let today = format!("{y:04}-{m:02}-{d:02}");
        let mut budget = self.config.max_notes;
        let config = self.config.clone();
        let models = models::Models::new(&config);
        let problems = self.load_rules(vaults);
        report.problems.extend(problems);
        self.load_layout(vaults);
        let problems = self.seed_sub_space_rules();
        report.problems.extend(problems);
        // Inbox first: what it sends on is filed in its vault in the same pass.
        let before = budget;
        let mut waiting = BTreeMap::new();
        for t in triage {
            let left = self.sort_triage(t, vaults, &mut budget, report);
            waiting.insert(t.clone(), left);
            if report.key_refused {
                return;
            }
        }
        if waiting.values().any(|l| !l.is_empty()) || night {
            let (added, problems) = self.improve_keywords(&models, &waiting, vaults, night);
            report.keywords_added += added;
            report.problems.extend(problems);
            if added > 0 {
                // The keywords changed: the router runs again on what was left.
                report.unsure = 0;
                budget = before.saturating_sub(report.sent as usize);
                waiting.clear();
                for t in triage {
                    let left = self.sort_triage(t, vaults, &mut budget, report);
                    waiting.insert(t.clone(), left);
                    if report.key_refused {
                        return;
                    }
                }
            }
        }
        self.leave_waiting_notes(&waiting, report);
        let inboxes = if self.config.inbox_vaults.is_empty() {
            vaults.to_vec()
        } else {
            self.config.inbox_vaults.clone()
        };
        for vault in inboxes {
            if !vaults.contains(&vault) {
                continue;
            }
            // A note stays in the vault whose inbox holds it: the model only picks tags and a title.
            let allowed: Vec<String> = vec![vault.clone()];
            let system = system_prompt(&allowed, tags, &vault, &today);
            let listed = match self.client.call(
                "recall",
                json!({ "space": vault, "query": "", "tags": ["inbox"], "limit": 50 }),
            ) {
                Ok(l) => l,
                Err(e) => {
                    self.fail(report, "the inbox cannot be read", &e);
                    continue;
                }
            };
            for note in listed["results"].as_array().into_iter().flatten() {
                let (Some(id), Some(text)) = (note["id"].as_str(), note["text"].as_str()) else {
                    continue;
                };
                // Only a note of this vault (not another vault's, shown here by `@`), still waiting, and a note:
                // not a recording's or a document's own description.
                if budget == 0
                    || note["space"].as_str().is_some_and(|s| s != vault)
                    || note["status"] != "active"
                    || note["type"].as_str() == Some("file")
                    || tags_of(note).iter().any(|g| g.starts_with("inbox/audio"))
                    || is_library(note)
                    || self.state.note_fails(id) >= MAX_FAILS
                {
                    continue;
                }
                budget -= 1;
                // A piece of a note the node absorbed: its facts are drawn out, in this vault, and the piece leaves.
                if tags_of(note).contains(&"inbox/absorb") {
                    let vault_tags = tags.get(&vault).cloned().unwrap_or_default();
                    self.extract_one(&models, note, id, text, &vault, &vault_tags, &today, report);
                    if report.key_refused {
                        return;
                    }
                    continue;
                }
                // The person's rule first: a note marked #santé goes to the journal of health, whatever a model says.
                if self.health_note(note, id, text, &vault, vaults, report) {
                    if report.key_refused {
                        return;
                    }
                    continue;
                }
                // The keywords: a note that clearly belongs to another vault goes to the inbox of that vault; one the person
                // asks to file in several spaces is copied to each. A copy made by the janitor is never copied again.
                let copy = tags_of(note).contains(&COPY_TAG);
                let routed: Vec<String> = if copy {
                    vec![]
                } else {
                    router::route(text, &tags_of(note), &self.routing_rules(), &self.layout)
                        .into_iter()
                        .filter(|t| vaults.contains(t))
                        .collect()
                };
                let others: Vec<String> = routed.iter().filter(|t| **t != vault).cloned().collect();
                if !others.is_empty() {
                    let stays = routed.contains(&vault);
                    match self.send_many(note, id, text, &vault, &others, stays, "the keywords") {
                        Ok(()) => report.sent += 1,
                        Err(e) => {
                            self.state.note_failed(id);
                            self.fail(
                                report,
                                &format!("{id} cannot be sent to {}", others.join(", ")),
                                &e,
                            );
                        }
                    }
                    if report.key_refused {
                        return;
                    }
                    if !stays {
                        continue;
                    }
                }
                // What it has learned first: a note it is sure of is filed without the model (one time in ten the model is
                // asked anyway, and a disagreement lowers the score of the words that misled it).
                let note_words = learn::words(text);
                let guess = if learn::is_loop(text) {
                    None
                } else {
                    self.state.learned(&vault).guess(&note_words, false)
                };
                let check = guess.is_some()
                    && self
                        .state
                        .count("checks", 1)
                        .is_multiple_of(learn::CHECK_EVERY);
                let alone = guess.as_ref().filter(|_| !check).map(|g| Choice {
                    vault: g.vault.clone().unwrap_or_else(|| vault.clone()),
                    tags: g.tags.iter().map(|(t, _)| t.clone()).collect(),
                    title: String::new(),
                });
                let (choice, by) = if let Some(c) = alone {
                    report.without_model += 1;
                    self.state.count(&format!("alone:{vault}"), 1);
                    (c, "what it learned")
                } else {
                    let mut log = vec![];
                    // Notes like this one already filed, with their tags: examples the model sees (data, not orders).
                    let shown = format!("{text}{}", self.similar(&allowed, &note_words, id));
                    let Some((choice, by)) =
                        models.route(&system, &shown, &allowed, &vault, &mut log)
                    else {
                        report.left += 1;
                        report
                            .problems
                            .extend(log.into_iter().map(|l| format!("{id}: {l}")));
                        continue;
                    };
                    self.state.count(&format!("by_model:{vault}"), 1);
                    // Learned from the model only, never from itself; a checked guess that the model contradicts loses.
                    if let Some(g) = &guess {
                        let agreed = g.tags.iter().all(|(t, _)| choice.tags.contains(t))
                            && g.vault.as_ref().is_none_or(|v| *v == choice.vault);
                        if !agreed {
                            self.state.learned(&vault).wrong(&g.led_by);
                        }
                    }
                    if learn::worth_learning(text) {
                        self.state
                            .learned(&vault)
                            .learn(&note_words, &choice.tags, None);
                    }
                    self.state.saved();
                    (choice, by)
                };
                match self.file_one(note, id, text, &vault, &choice, by) {
                    Ok(_) => report.filed += 1,
                    Err(e) => {
                        self.state.note_failed(id);
                        self.fail(
                            report,
                            &format!("{id} cannot be filed in {}", choice.vault),
                            &e,
                        );
                    }
                }
                if report.key_refused {
                    return;
                }
            }
        }
    }

    /// The notes of one Inbox: the health rule, the work rule, then the keyword router (see [`router`]) decide; each
    /// note goes, as it is and still in the inbox, to the vault they name. A note they cannot place stays, for the keywords
    /// to learn from and for the person. No model is asked here. Files and recordings are not sent.
    fn sort_triage(
        &mut self,
        triage: &str,
        vaults: &[String],
        budget: &mut usize,
        report: &mut Report,
    ) -> Vec<Unplaced> {
        let mut left = self.sort_notes(triage, vaults, budget, report);
        if !report.key_refused {
            left.extend(self.sort_files(triage, vaults, budget, report));
        }
        left
    }

    fn sort_notes(
        &mut self,
        triage: &str,
        vaults: &[String],
        budget: &mut usize,
        report: &mut Report,
    ) -> Vec<Unplaced> {
        let mut unplaced = vec![];
        if vaults.is_empty() {
            return unplaced;
        }
        // The notes are claimed (a lease of ten minutes), so that another sorter, on a device, does not take the same
        // ones. A node without the tool is read the old way.
        let listed = match self
            .client
            .call("triage_claim", json!({ "limit": 50, "minutes": 10 }))
        {
            Ok(c) => {
                let notes: Vec<Value> = c["claimed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|n| n["space"].as_str().is_none_or(|s| s == triage))
                    .cloned()
                    .map(|mut n| {
                        n["status"] = json!("active");
                        n
                    })
                    .collect();
                self.claimed = true;
                json!({ "results": notes })
            }
            Err(_) => match self.client.call(
                "recall",
                json!({ "space": triage, "query": "", "tags": ["inbox"], "limit": 50 }),
            ) {
                Ok(l) => l,
                Err(e) => {
                    self.fail(report, "the Inbox cannot be read", &e);
                    return unplaced;
                }
            },
        };
        for note in listed["results"].as_array().into_iter().flatten() {
            let (Some(id), Some(text)) = (note["id"].as_str(), note["text"].as_str()) else {
                continue;
            };
            if *budget == 0
                || note["space"].as_str().is_some_and(|s| s != triage)
                || note["status"] != "active"
                || note["type"].as_str() == Some("file")
                || tags_of(note).iter().any(|g| g.starts_with("inbox/audio"))
                || is_library(note)
            {
                continue;
            }
            if self.state.note_fails(id) >= MAX_FAILS {
                unplaced.push(Unplaced::new(
                    id,
                    text,
                    "it could not be sent three times; the person decides",
                ));
                continue;
            }
            *budget -= 1;
            if self.health_note(note, id, text, triage, vaults, report) {
                if report.key_refused {
                    return unplaced;
                }
                continue;
            }
            // The person's rule for work: a meeting, or a note about their work, goes to `work`.
            let mut why = String::new();
            let (mut to, mut by): (Vec<String>, &str) =
                if vaults.iter().any(|v| v == work_rule::WORK)
                    && work_rule::applies(text, &self.config.work_words)
                {
                    (vec![work_rule::WORK.to_string()], "the person's work rule")
                } else if let Some(v) = self.people_route(&tags_of(note), triage, vaults) {
                    (vec![v], "the people of the note")
                } else {
                    let routed =
                        router::decide(text, &tags_of(note), &self.routing_rules(), &self.layout)
                            .unwrap_or_else(|reason| {
                                why = reason;
                                vec![]
                            });
                    (
                        routed
                            .into_iter()
                            .filter(|t| t != triage && vaults.contains(t))
                            .collect(),
                        "the keywords",
                    )
                };
            // No keyword matched: where the person has sent notes like this one (the node learned it), when one space is sure.
            if to.is_empty()
                && (why.is_empty() || why == "no keyword matched")
                && let Some(v) = self.taught_route(text, triage, vaults)
            {
                to = vec![v];
                by = "what the person taught";
            }
            if to.is_empty() {
                report.unsure += 1;
                if why.is_empty() {
                    why = "no keyword matched".into();
                }
                unplaced.push(Unplaced::new(id, text, &why));
                continue;
            }
            match self.send_many(note, id, text, triage, &to, false, by) {
                Ok(()) => report.sent += 1,
                Err(e) => {
                    self.state.note_failed(id);
                    self.fail(
                        report,
                        &format!("{id} cannot be sent to {}", to.join(", ")),
                        &e,
                    );
                }
            }
            if report.key_refused {
                return unplaced;
            }
        }
        unplaced
    }

    /// Where the person sent earlier notes with the same people (the `personnes/` tags of the note), as the node learned it:
    /// a strong hint, ahead of the keywords. A note with no person, or a node that learned nothing, says nothing.
    fn people_route(&mut self, tags: &[&str], triage: &str, vaults: &[String]) -> Option<String> {
        let people: Vec<&str> = tags
            .iter()
            .copied()
            .filter(|t| t.starts_with("personnes/"))
            .collect();
        if people.is_empty() {
            return None;
        }
        let answer = self
            .client
            .call("tags_suggest", json!({ "text": "-", "tags": people }))
            .ok()?;
        let first = answer["route"].as_array()?.first()?;
        if first.get("by")?.as_str() != Some("people") {
            return None;
        }
        first
            .get("space")?
            .as_str()
            .filter(|s| *s != triage && vaults.iter().any(|v| v == s))
            .map(String::from)
    }

    /// Where the person has sent notes like this one, as the node learned it (`tags_suggest`, `route`): the first space,
    /// when it is one of the vaults that can be filled. A node without the tool, or that learned nothing, says nothing.
    fn taught_route(&mut self, text: &str, triage: &str, vaults: &[String]) -> Option<String> {
        let answer = self
            .client
            .call("tags_suggest", json!({ "text": text }))
            .ok()?;
        answer["route"]
            .as_array()?
            .first()?
            .get("space")?
            .as_str()
            .filter(|s| *s != triage && vaults.iter().any(|v| v == s))
            .map(String::from)
    }

    /// The files of one Inbox (documents, images; never a recording, which the node reads): the same router as for
    /// notes, on the words of the file's name, its readable text (a text or markdown file), its description and its tags.
    /// A file the router places goes, as it is, to the inbox of that vault (`triage_send`); the others stay, with the reason.
    /// A file goes to the first space named when the person names several: nothing is copied.
    fn sort_files(
        &mut self,
        triage: &str,
        vaults: &[String],
        budget: &mut usize,
        report: &mut Report,
    ) -> Vec<Unplaced> {
        let mut unplaced = vec![];
        if vaults.is_empty() {
            return unplaced;
        }
        // A node without the tool has no files to sort this way.
        let Ok(listed) = self.client.call("triage_waiting", json!({ "limit": 200 })) else {
            return unplaced;
        };
        let files: Vec<Value> = listed["triage"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["space"].as_str() == Some(triage))
            .flat_map(|t| t["files"].as_array().cloned().unwrap_or_default())
            .collect();
        for file in &files {
            let (Some(id), Some(name)) = (file["id"].as_str(), file["name"].as_str()) else {
                continue;
            };
            let mime = file["mime"].as_str().unwrap_or("");
            // A recording is the node's: its text is made there, and when that fails it is tagged and left.
            if mime.starts_with("audio/") || file["status"].as_str().is_some_and(|s| s != "active")
            {
                continue;
            }
            let note = file["note"].as_str().unwrap_or("");
            if self.state.note_fails(id) >= MAX_FAILS {
                unplaced.push(Unplaced::file(
                    id,
                    name,
                    note,
                    "it could not be sent three times; the person decides",
                ));
                continue;
            }
            if *budget == 0 {
                unplaced.push(Unplaced::file(id, name, note, "not looked at yet"));
                continue;
            }
            *budget -= 1;
            // What is readable: the text of a text or markdown file (the first characters only).
            let body =
                if mime.starts_with("text/") && file["size"].as_i64().unwrap_or(0) <= 1_000_000 {
                    self.client
                        .file_get(id)
                        .map(|(_, bytes)| {
                            String::from_utf8_lossy(&bytes)
                                .chars()
                                .take(FILE_TEXT_CHARS)
                                .collect::<String>()
                        })
                        .unwrap_or_default()
                } else {
                    String::new()
                };
            let text = format!("{name}\n{note}\n{body}");
            let tags: Vec<&str> = tags_of(file);
            let mut why = String::new();
            let mut to: Vec<String> = if vaults.iter().any(|v| v == work_rule::WORK)
                && work_rule::applies(&text, &self.config.work_words)
            {
                vec![work_rule::WORK.to_string()]
            } else {
                router::decide(&text, &tags, &self.routing_rules(), &self.layout)
                    .unwrap_or_else(|reason| {
                        why = reason;
                        vec![]
                    })
                    .into_iter()
                    .filter(|t| t != triage && vaults.contains(t))
                    .collect()
            };
            if to.is_empty()
                && (why.is_empty() || why == "no keyword matched")
                && let Some(v) = self.taught_route(&text, triage, vaults)
            {
                to = vec![v];
            }
            let Some(target) = to.first() else {
                report.unsure += 1;
                if why.is_empty() {
                    why = "no keyword matched".into();
                }
                unplaced.push(Unplaced::file(id, name, note, &why));
                continue;
            };
            match self
                .client
                .call("triage_send", json!({ "id": id, "to": target }))
            {
                Ok(_) => report.files_sent += 1,
                Err(e) => {
                    self.state.note_failed(id);
                    self.fail(report, &format!("{name} cannot be sent to {target}"), &e);
                    if report.key_refused {
                        return unplaced;
                    }
                    unplaced.push(Unplaced::file(id, name, note, "it could not be sent"));
                }
            }
        }
        unplaced
    }

    /// The person's rule for a note marked `#santé` (see [`health_rule`]): filed in `health` under `journal/<date>` and
    /// `sante`, its words untouched and where it comes from kept. Never a note of `work`. Says whether it took the note.
    fn health_note(
        &mut self,
        note: &Value,
        id: &str,
        text: &str,
        from: &str,
        vaults: &[String],
        report: &mut Report,
    ) -> bool {
        if from == health_rule::NEVER_FROM
            || !vaults.iter().any(|v| v == health_rule::HEALTH)
            || !health_rule::applies(text)
        {
            return false;
        }
        let offset = self.config.utc_offset_minutes;
        let written = note["created_at"].as_i64().unwrap_or_else(now);
        let local = time::civil(written, offset);
        let date = health_rule::written_date(text, local.0).unwrap_or_else(|| {
            let today = format!("{:04}-{:02}-{:02}", local.0, local.1, local.2);
            let wake = if local.3 < 12 {
                self.wake_on(&today)
            } else {
                None
            };
            health_rule::day_of(local, wake)
        });
        // The day and the rule's tag, then what the health referential finds in the words (alerts, signs, measures).
        let mut tags = vec![format!("journal/{date}"), health_rule::TAG.into()];
        tags.extend(health_rule::detected_tags(text));
        let choice = Choice {
            vault: health_rule::HEALTH.into(),
            tags,
            title: String::new(),
        };
        let body = health_rule::with_origin(text, &health_rule::origin(note));
        match self.file_one(note, id, &body, from, &choice, "the person's #santé rule") {
            Ok(_) => report.filed += 1,
            Err(e) => {
                self.state.note_failed(id);
                self.fail(report, &format!("{id} cannot be filed in health"), &e);
            }
        }
        true
    }

    /// The hour the person woke up on `date`, from the day an app wrote in `health` (minutes since midnight).
    fn wake_on(&mut self, date: &str) -> Option<u32> {
        let found = self
            .client
            .call(
                "recall",
                json!({ "space": health_rule::HEALTH, "query": "", "tags": [format!("journal/{date}")], "limit": 20 }),
            )
            .ok()?;
        found["results"]
            .as_array()?
            .iter()
            .filter_map(|f| f["text"].as_str())
            .filter(|t| health_rule::is_day_note(t))
            .find_map(health_rule::wake_from_day)
    }

    /// Writes a note of Inbox in the inbox of each space of `to`, its text and tags unchanged, then retires it from
    /// Inbox once every write is certain. With several spaces the first gets the note and the others a copy (all tagged
    /// `janitor/copy`, so that it is not copied again); the reason of the retirement says where the copies went.
    /// `keep`: the note is not retired (it already sits in an inbox that keeps it); only copies are made.
    #[allow(clippy::too_many_arguments)]
    fn send_many(
        &mut self,
        note: &Value,
        id: &str,
        text: &str,
        triage: &str,
        to: &[String],
        keep: bool,
        by: &str,
    ) -> Result<(), ClientError> {
        let mut tags_out: Vec<String> = tags_of(note).into_iter().map(String::from).collect();
        if !tags_out
            .iter()
            .any(|t| t == "inbox" || t.starts_with("inbox/"))
        {
            tags_out.push("inbox".into());
        }
        let kind = note["kind"].as_str().unwrap_or("fact");
        let several = to.len() > 1 || keep;
        let mut written = vec![];
        for space in to {
            let mut tags = tags_out.clone();
            if several {
                tags.push(COPY_TAG.into());
            }
            let r = self.client.call(
                "remember",
                json!({ "space": space, "kind": kind, "text": text, "tags": tags }),
            )?;
            let new = r["fact"]["id"]
                .as_str()
                .filter(|n| {
                    *n != id && matches!(r["result"].as_str(), Some("created" | "duplicate"))
                })
                .map(String::from)
                .ok_or_else(|| {
                    ClientError::Unreadable(format!("nothing was written: {}", r["result"]))
                })?;
            written.push((space.clone(), new));
        }
        if !keep {
            let places: Vec<String> = written
                .iter()
                .map(|(s, n)| format!("the inbox of {s} as {n}"))
                .collect();
            let reason = if written.len() > 1 {
                format!(
                    "sent from {triage} by the janitor ({by}) to {}; the person asked for it in each of these spaces, so the note is copied, nothing is erased",
                    places.join(" and ")
                )
            } else {
                format!(
                    "sent from {triage} by the janitor ({by}) to {}",
                    places.join("")
                )
            };
            self.client
                .call("retire", json!({ "id": id, "reason": reason }))?;
        }
        if let Some((space, new)) = written.first() {
            self.voice_command(text, space, new);
        }
        Ok(())
    }

    /// A note said as a command also becomes a card of a living board, its source the note just filed: « j'ai une idée… »
    /// → the space's ideas, « il faut… » / « je dois… » → what's next, « problème… » / « risque… » → targets, « acheter… » /
    /// « il manque… » → the shopping list of the home space, wherever it was said. Best effort: a card that cannot be
    /// written leaves the note filed as it is.
    fn voice_command(&mut self, text: &str, space: &str, source: &str) {
        let Some((page, home_list)) = command_board(text) else {
            return;
        };
        let target = if home_list {
            let Ok(listed) = self.client.call("spaces", json!({})) else {
                return;
            };
            let home = listed["spaces"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|s| s["kind"] == "home")
                .and_then(|s| s["name"].as_str())
                .map(String::from);
            match home {
                Some(h) => h,
                None => return,
            }
        } else {
            space.to_string()
        };
        let line: String = text.trim().chars().take(140).collect();
        let _ = self.client.call(
            "living_set",
            json!({ "space": target, "page": page, "add": [ format!("{line} ({source})") ] }),
        );
    }

    /// Draws the facts out of one absorbed piece and writes them in its own vault; then the piece leaves the inbox.
    #[allow(clippy::too_many_arguments)]
    fn extract_one(
        &mut self,
        models: &models::Models<'_>,
        note: &Value,
        id: &str,
        text: &str,
        vault: &str,
        vault_tags: &[String],
        today: &str,
        report: &mut Report,
    ) {
        let _ = note;
        let mut log = vec![];
        let system = models::extract_prompt(vault_tags, today);
        let Some((facts, by)) = models.extract(&system, text, &mut log) else {
            report.left += 1;
            report
                .problems
                .extend(log.into_iter().map(|l| format!("{id}: {l}")));
            return;
        };
        // Where it came from, said once at the end of each fact: « (d'après Projets/Alpha.md) ».
        let source = text
            .strip_prefix("D'après la note ")
            .and_then(|r| r.split(", modifiée").next())
            .map(|p| format!(" (d'après {p})"))
            .unwrap_or_default();
        let mut written = vec![];
        for f in &facts {
            let mut tags_out = f.tags.clone();
            tags_out.extend(["absorb".to_string(), "janitor".to_string()]);
            let args = json!({ "space": vault, "kind": "fact", "text": format!("{}{source}", f.text), "tags": tags_out });
            let mut r = match self.client.call("remember", args.clone()) {
                Ok(r) => r,
                Err(e) => {
                    self.state.note_failed(id);
                    self.fail(report, &format!("{id}: a fact cannot be kept"), &e);
                    return;
                }
            };
            if r["result"] == "needs_decision" {
                let mut again = args;
                again["distinct_from"] = json!(
                    r["candidates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["id"].as_str())
                        .collect::<Vec<_>>()
                );
                r = match self.client.call("remember", again) {
                    Ok(r) => r,
                    Err(e) => {
                        self.state.note_failed(id);
                        self.fail(report, &format!("{id}: a fact cannot be kept"), &e);
                        return;
                    }
                };
            }
            if let Some(f) = r["fact"]["id"].as_str() {
                written.push(f.to_string());
            }
        }
        let reason = if written.is_empty() {
            format!("nothing durable in it, said the janitor ({by})")
        } else {
            format!("facts drawn by the janitor ({by}): {}", written.join(", "))
        };
        match self
            .client
            .call("retire", json!({ "id": id, "reason": reason }))
        {
            Ok(_) => report.filed += 1,
            Err(e) => self.fail(
                report,
                &format!("{id}: its facts are kept but it could not leave the inbox"),
                &e,
            ),
        }
    }

    /// Files one note where `choice` says, its text unchanged: the id of the fact that holds it now.
    ///
    /// In its own vault the note is **replaced** by its filed version (`supersede`: one step, the history keeps the
    /// raw note, and a crash between two calls cannot leave it twice). In another vault it is written there, then
    /// retired from the inbox, and only once the write is certain: a fact id that is not the note's own.
    fn file_one(
        &mut self,
        note: &Value,
        id: &str,
        text: &str,
        vault: &str,
        choice: &Choice,
        by: &str,
    ) -> Result<String, ClientError> {
        let mut tags_out = choice.tags.clone();
        tags_out.push("janitor".into());
        // The participants of a call stay on the note, whatever the model chose.
        for t in tags_of(note) {
            if is_call_tag(t) && !tags_out.iter().any(|x| x == t) {
                tags_out.push(t.to_string());
            }
        }
        let kind = note["kind"].as_str().unwrap_or("fact");
        let body = filed_text(&choice.title, text);
        if choice.vault == vault {
            let r = self.client.call(
                "supersede",
                json!({ "id": id, "text": body, "tags": tags_out, "kind": kind, "reason": format!("filed by the janitor ({by})") }),
            )?;
            return r["fact"]["id"]
                .as_str()
                .map(String::from)
                .ok_or_else(|| ClientError::Unreadable(format!("supersede: {r}")));
        }
        let args = json!({ "space": choice.vault, "kind": kind, "text": body, "tags": tags_out });
        let mut r = self.client.call("remember", args.clone())?;
        if r["result"] == "needs_decision" {
            // A note is not a duplicate of what it resembles: both are kept.
            let mut again = args;
            again["distinct_from"] = json!(
                r["candidates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["id"].as_str())
                    .collect::<Vec<_>>()
            );
            r = self.client.call("remember", again)?;
        }
        let new = r["fact"]["id"]
            .as_str()
            .filter(|n| *n != id && matches!(r["result"].as_str(), Some("created" | "duplicate")))
            .map(String::from)
            .ok_or_else(|| {
                ClientError::Unreadable(format!("nothing was written: {}", r["result"]))
            })?;
        self.client.call(
            "retire",
            json!({ "id": id, "reason": format!("filed by the janitor ({by}) in {} as {new}", choice.vault) }),
        )?;
        Ok(new)
    }
}

/// The board a note said as a command goes to, and whether it is the home space's shopping list. The words are the
/// person's own way of speaking, in French and English; a note that starts otherwise is no command.
pub fn command_board(text: &str) -> Option<(&'static str, bool)> {
    let t = text.trim().to_lowercase().replace('’', "'");
    let starts = |words: &[&str]| words.iter().any(|w| t.starts_with(w));
    if starts(&[
        "acheter",
        "il manque",
        "on n'a plus",
        "plus de ",
        "buy ",
        "we need to buy",
        "out of ",
    ]) {
        return Some(("list-shopping", true));
    }
    if starts(&[
        "j'ai une idée",
        "j'ai une idee",
        "idée",
        "idee",
        "une idée",
        "i have an idea",
        "idea",
    ]) {
        return Some(("ideas", false));
    }
    if starts(&["problème", "probleme", "risque", "souci", "problem", "risk"]) {
        return Some(("targets", false));
    }
    if starts(&[
        "il faut",
        "je dois",
        "à faire",
        "a faire",
        "penser à",
        "penser a",
        "rappeler",
        "todo",
        "i must",
        "i have to",
        "remember to",
    ]) {
        return Some(("next", false));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::filed_text;

    #[test]
    fn the_title_goes_before_the_note_unless_it_would_only_repeat_it() {
        assert_eq!(
            filed_text("Mal dormi", "2026-11-02, cinq heures"),
            "Mal dormi : 2026-11-02, cinq heures"
        );
        assert_eq!(filed_text("", "texte"), "texte");
        assert_eq!(filed_text("Idée", "Idée de projet"), "Idée de projet");
        assert_eq!(
            filed_text("test janitor", "TEST janitor : une note"),
            "TEST janitor : une note"
        );
        let long = "x".repeat(3985);
        assert_eq!(filed_text("Titre", &long), long);
    }
}
