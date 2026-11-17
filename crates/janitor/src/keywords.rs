//! The keywords of the spaces, kept by the janitor. Each space has a `rules` note (`{"version":1,"tags":[…],"keywords":[…]}`,
//! the one the person's page writes). The deterministic router reads it ([`crate::router`]); this module keeps it up to date:
//!
//! - **The frequency rule**: a word that comes back in many notes of one space (at least `keyword_min_notes`) and far less
//!   in the others becomes a keyword of that space, without asking a model.
//! - **The model proposes**: it reads a sample of the notes filed in each space and the notes the router could not place,
//!   and proposes keywords and tags. It writes nothing: the janitor checks the proposals and writes them.
//! - Every change is a new version of the `rules` note (`supersede`): the history keeps the old ones, nothing is erased.
//! - What is left in « À trier » after that is announced to the person by one note there, tagged `janitor/waiting`.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use memcp_client::Error as ClientError;
use serde_json::{Value, json};

use crate::models::{self, Models};
use crate::router::{Place, Rules, clean_keyword};
use crate::{Janitor, Report, learn, now, tags_of, time};

/// A note or a file of « À trier » that waits for the person, and why.
#[derive(Clone, Debug)]
pub struct Unplaced {
    /// The id of the note or of the file.
    pub id: String,
    /// What the router read: the text of the note, or the name and description of the file.
    pub text: String,
    /// Why it waits (`no keyword matched`, `two spaces tie: a, b`…).
    pub reason: String,
    pub file: bool,
}

impl Unplaced {
    pub fn new(id: &str, text: &str, reason: &str) -> Unplaced {
        Unplaced {
            id: id.into(),
            text: text.into(),
            reason: reason.into(),
            file: false,
        }
    }

    pub fn file(id: &str, name: &str, note: &str, reason: &str) -> Unplaced {
        Unplaced {
            id: id.into(),
            text: format!("{name} {note}").trim().to_string(),
            reason: reason.into(),
            file: true,
        }
    }
}

/// Reasons kept in the note of reasons, and how long each may be: the note is one fact of 4000 characters at most.
const MAX_REASONS: usize = 40;
const REASON_CHARS: usize = 60;

/// Notes read per main tag of a space, and tags read, for the frequency rule.
const SAMPLE_READ: usize = 100;
const SAMPLE_TAGS: usize = 8;
/// Notes of a space, and notes without a space, shown to the model.
const SHOWN_FILED: usize = 6;
const SHOWN_UNPLACED: usize = 10;
/// Keywords the frequency rule adds to one space in one pass.
const MAX_FREQUENT: usize = 10;
/// A word is frequent in a space when it is at least this many times more common there than in any other.
const DOMINANCE: usize = 3;

/// Tags of the notes that are not the person's own content.
const NOT_CONTENT: [&str; 7] = [
    "rules",
    "dashboard",
    "index",
    "guide",
    "janitor/keywords",
    "janitor/waiting",
    "janitor/reasons",
];

/// The words that, in at least `min` notes of one space, are far more common there than elsewhere, and are not yet a
/// keyword of any space. Per space, the most common first.
pub fn frequent(
    samples: &BTreeMap<String, Vec<String>>,
    rules: &BTreeMap<String, Rules>,
    min: usize,
) -> BTreeMap<String, Vec<String>> {
    let mut count: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
    for (space, notes) in samples {
        let c = count.entry(space.as_str()).or_default();
        for text in notes {
            for w in learn::words(text) {
                *c.entry(w).or_default() += 1;
            }
        }
    }
    let known: BTreeSet<&str> = rules
        .values()
        .flat_map(|r| r.keywords.iter().map(String::as_str))
        .collect();
    let mut out = BTreeMap::new();
    for (space, words) in &count {
        let mut found: Vec<(&String, usize)> = words
            .iter()
            .filter(|(w, n)| {
                **n >= min.max(1)
                    && !known.contains(w.as_str())
                    && count
                        .iter()
                        .filter(|(s, _)| *s != space)
                        .all(|(_, o)| o.get(*w).map_or(0, |x| *x) * DOMINANCE <= **n)
            })
            .map(|(w, n)| (w, *n))
            .collect();
        found.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let list: Vec<String> = found
            .into_iter()
            .take(MAX_FREQUENT)
            .map(|(w, _)| w.clone())
            .collect();
        if !list.is_empty() {
            out.insert((*space).to_string(), list);
        }
    }
    out
}

fn extract(text: &str, n: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(n).collect()
}

impl Janitor {
    /// Reads the `rules` note of each vault. The problems met.
    pub(crate) fn load_rules(&mut self, vaults: &[String]) -> Vec<String> {
        self.rules.clear();
        self.rule_ids.clear();
        self.vocab.clear();
        let mut problems = vec![];
        for v in vaults {
            // The vocabularies a person imported into this vault: an old node has no such tool, which is no problem.
            if let Ok(t) = self
                .client
                .call("vocabulary_terms", json!({ "space": v, "limit": 20000 }))
                && let Some(list) = t["terms"].as_array()
            {
                let terms: Vec<String> = list
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
                if !terms.is_empty() {
                    self.vocab.insert(v.clone(), terms);
                }
            }
            let found = match self.client.call(
                "recall",
                json!({ "space": v, "query": "", "tags": ["rules"], "limit": 5 }),
            ) {
                Ok(f) => f,
                Err(e) => {
                    problems.push(format!("the rules of {v} cannot be read: {e}"));
                    continue;
                }
            };
            let newest = found["results"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| f["status"] == "active" && f["space"].as_str().is_none_or(|s| s == v))
                .filter_map(|f| {
                    let rules = Rules::parse(f["text"].as_str()?)?;
                    Some((
                        f["created_at"].as_i64().unwrap_or(0),
                        f["id"].as_str()?,
                        rules,
                    ))
                })
                .max_by_key(|(at, _, _)| *at);
            if let Some((_, id, rules)) = newest {
                self.rules.insert(v.clone(), rules);
                self.rule_ids.insert(v.clone(), id.to_string());
            }
        }
        problems
    }

    /// Reads the labels and parents of the vaults (the `spaces` tool); with no answer, each vault stands at the top under
    /// its own name.
    pub(crate) fn load_layout(&mut self, vaults: &[String]) {
        let listed = self.client.call("spaces", json!({})).ok();
        self.layout = vaults
            .iter()
            .map(|v| {
                let row = listed
                    .as_ref()
                    .and_then(|l| l["spaces"].as_array())
                    .and_then(|a| a.iter().find(|s| s["name"].as_str() == Some(v)));
                let text = |k: &str| row.and_then(|r| r[k].as_str()).map(String::from);
                Place {
                    name: v.clone(),
                    label: text("label").unwrap_or_else(|| v.clone()),
                    parent: text("parent").filter(|p| vaults.contains(p)),
                }
            })
            .collect();
    }

    /// A sub-space with no `rules` note gets one, with its label and its name as keywords (the person's page can change
    /// them later). Returns the problems met.
    pub(crate) fn seed_sub_space_rules(&mut self) -> Vec<String> {
        let mut problems = vec![];
        for place in self.layout.clone() {
            if place.parent.is_none() || self.rule_ids.contains_key(&place.name) {
                continue;
            }
            let short = place
                .name
                .split_once('-')
                .map_or(place.name.as_str(), |x| x.1);
            let keywords: BTreeSet<String> = [place.label.as_str(), short]
                .into_iter()
                .filter_map(clean_keyword)
                .collect();
            if keywords.is_empty() {
                continue;
            }
            let rules = Rules {
                tags: vec![],
                keywords: keywords.into_iter().collect(),
            };
            if let Err(e) = self.write_rules(&place.name, &rules, "keywords of a sub-space") {
                problems.push(format!("the rules of {} cannot be seeded: {e}", place.name));
            }
        }
        problems
    }

    /// The rules the router reads: each vault's own keywords and tags, then the terms of its vocabularies. A term
    /// that is also a keyword counts once; the `rules` notes themselves are never changed by it.
    pub(crate) fn routing_rules(&self) -> BTreeMap<String, Rules> {
        let mut all = self.rules.clone();
        for (space, terms) in &self.vocab {
            let r = all.entry(space.clone()).or_default();
            let known: BTreeSet<String> = r.keywords.iter().cloned().collect();
            r.keywords
                .extend(terms.iter().filter(|t| !known.contains(*t)).cloned());
        }
        all
    }

    /// Writes the rules of a space: a new version of its `rules` note (the old one stays in the history), or the first note.
    fn write_rules(&mut self, space: &str, rules: &Rules, reason: &str) -> Result<(), ClientError> {
        let text = rules.render();
        let r = match self.rule_ids.get(space).cloned() {
            Some(old) => self.client.call(
                "supersede",
                json!({ "id": old, "text": text, "tags": ["rules"], "kind": "context", "reason": reason }),
            )?,
            None => {
                let args = json!({ "space": space, "kind": "context", "text": text, "tags": ["rules"] });
                let mut r = self.client.call("remember", args.clone())?;
                if r["result"] == "needs_decision" {
                    // The settings note is never a near duplicate of another note.
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
                r
            }
        };
        let id = r["fact"]["id"]
            .as_str()
            .ok_or_else(|| ClientError::Unreadable(format!("rules not written: {r}")))?;
        self.rule_ids.insert(space.to_string(), id.to_string());
        self.rules.insert(space.to_string(), rules.clone());
        Ok(())
    }

    /// Notes already filed in a space (not its settings, nor the janitor's own notes). A listing needs a tag, so it reads
    /// the space's main tags (from `stats`, the busiest first) one after the other.
    fn filed_sample(&mut self, space: &str, stats: &Value) -> Result<Vec<String>, ClientError> {
        let mut main: Vec<(&str, u64)> = stats[space]["tags"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(t, _)| {
                !t.contains('/')
                    && !NOT_CONTENT.contains(&t.as_str())
                    && *t != "inbox"
                    && *t != "janitor"
            })
            .map(|(t, n)| (t.as_str(), n.as_u64().unwrap_or(0)))
            .collect();
        main.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let mut seen = BTreeSet::new();
        let mut out = vec![];
        for (tag, _) in main.into_iter().take(SAMPLE_TAGS) {
            let found = self.client.call(
                "recall",
                json!({ "space": space, "query": "", "tags": [tag], "limit": SAMPLE_READ }),
            )?;
            for f in found["results"].as_array().into_iter().flatten() {
                let content = f["status"] == "active"
                    && f["type"].as_str() != Some("file")
                    && f["space"].as_str().is_none_or(|s| s == space)
                    && !tags_of(f).iter().any(|t| {
                        NOT_CONTENT.contains(t) || *t == "inbox" || t.starts_with("inbox/")
                    });
                if let (true, Some(id), Some(text)) =
                    (content, f["id"].as_str(), f["text"].as_str())
                    && seen.insert(id.to_string())
                {
                    out.push(text.to_string());
                }
            }
        }
        Ok(out)
    }

    /// Adds keywords to the spaces: those the frequency rule finds, and those the model proposes when notes wait (or at
    /// night) and the last time was long enough ago. Returns how many keywords and tags were added, and the problems.
    pub(crate) fn improve_keywords(
        &mut self,
        models: &Models<'_>,
        waiting: &BTreeMap<String, Vec<Unplaced>>,
        vaults: &[String],
        night: bool,
    ) -> (u32, Vec<String>) {
        let mut problems = vec![];
        let mut samples: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let stats = match self.client.call("stats", json!({})) {
            Ok(s) => s,
            Err(e) => return (0, vec![format!("the memory cannot be read: {e}")]),
        };
        for v in vaults {
            match self.filed_sample(v, &stats) {
                Ok(s) => {
                    samples.insert(v.clone(), s);
                }
                Err(e) => problems.push(format!("the notes of {v} cannot be read: {e}")),
            }
        }
        let mut add: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
        for (space, words) in frequent(&samples, &self.rules, self.config.keyword_min_notes) {
            add.entry(space).or_default().0.extend(words);
        }
        let unplaced: Vec<&Unplaced> = waiting.values().flatten().collect();
        let due = night
            || now()
                .unsigned_abs()
                .saturating_sub(self.state.stamp("proposed"))
                >= self.config.keywords_every_minutes * 60;
        if (!unplaced.is_empty() || night) && due && !samples.is_empty() {
            self.state.set_stamp("proposed", now().unsigned_abs());
            let mut shown = String::new();
            for (space, notes) in &samples {
                let rules = self.rules.get(space).cloned().unwrap_or_default();
                shown.push_str(&format!(
                    "## Coffre {space}\nMots-clés actuels : {}\nÉtiquettes actuelles : {}\nNotes déjà rangées :\n",
                    rules.keywords.join(", "),
                    rules.tags.join(", ")
                ));
                for n in notes.iter().take(SHOWN_FILED) {
                    shown.push_str(&format!("- {}\n", extract(n, 200)));
                }
                shown.push('\n');
            }
            if !unplaced.is_empty() {
                shown.push_str("## Notes qu'aucun coffre n'a pu prendre\n");
                for n in unplaced.iter().take(SHOWN_UNPLACED) {
                    shown.push_str(&format!("- {}\n", extract(&n.text, 200)));
                }
            }
            let mut log = vec![];
            match models.propose(&models::propose_prompt(), &shown, vaults, &mut log) {
                Some((proposals, _)) => {
                    // A keyword two spaces ask for is no use to either; one that another space already owns stays its.
                    let mut asked: BTreeMap<&str, usize> = BTreeMap::new();
                    for p in proposals.values() {
                        for k in p.keywords.iter().collect::<BTreeSet<_>>() {
                            *asked.entry(k.as_str()).or_default() += 1;
                        }
                    }
                    for (space, p) in &proposals {
                        let keywords: Vec<String> = p
                            .keywords
                            .iter()
                            .filter(|k| {
                                asked.get(k.as_str()) == Some(&1)
                                    && !self
                                        .rules
                                        .iter()
                                        .any(|(s, r)| s != space && r.keywords.contains(k))
                            })
                            .cloned()
                            .collect();
                        let entry = add.entry(space.clone()).or_default();
                        entry.0.extend(keywords);
                        entry.1.extend(p.tags.iter().cloned());
                    }
                }
                None => problems.extend(log.into_iter().map(|l| format!("keywords: {l}"))),
            }
        }
        let mut added = 0u32;
        for (space, (keywords, tags)) in add {
            let old = self.rules.get(&space).cloned().unwrap_or_default();
            let Some(next) = old.with(&keywords, &tags) else {
                continue;
            };
            let n = (next.keywords.len() + next.tags.len())
                .saturating_sub(old.keywords.len() + old.tags.len());
            match self.write_rules(&space, &next, "keywords added by the janitor") {
                Ok(()) => added += u32::try_from(n).unwrap_or(u32::MAX),
                Err(e) => problems.push(format!("the rules of {space} cannot be written: {e}")),
            }
        }
        (added, problems)
    }

    /// Keeps one note of the janitor in `space`, tagged `tag`, with `text`; with `None` the note retires (nothing waits).
    /// The note is replaced only when its text changed.
    fn keep_note(
        &mut self,
        key: &str,
        space: &str,
        tag: &str,
        text: Option<String>,
        report: &mut Report,
    ) {
        let old = self.state.note_id(key, space);
        let stamp = format!("{key}:{space}");
        let Some(text) = text else {
            if let Some(id) = old {
                // Nothing waits any more: the note retires (it stays in the history).
                let r = self.client.call(
                    "retire",
                    json!({ "id": id, "reason": "nothing waits in the inbox any more" }),
                );
                if r.is_ok() {
                    self.state.set_note_id(key, space, None);
                    self.state.set_stamp(&stamp, 0);
                }
            }
            return;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let digest = hasher.finish() >> 1;
        if old.is_some() && self.state.stamp(&stamp) == digest {
            return;
        }
        let written = match &old {
            Some(id) => self.client.call(
                "supersede",
                json!({ "id": id, "text": text, "tags": [tag], "kind": "context", "reason": "the janitor's note on what waits" }),
            ),
            None => Err(ClientError::Tool("no note yet".into())),
        }
        .or_else(|_| {
            self.client.call(
                "remember",
                json!({ "space": space, "kind": "context", "text": text, "tags": [tag] }),
            )
        });
        match written {
            Ok(r) => {
                if let Some(id) = r["fact"]["id"].as_str() {
                    self.state.set_note_id(key, space, Some(id));
                    self.state.set_stamp(&stamp, digest);
                }
            }
            Err(e) => report.problems.push(format!(
                "the note of what waits in {space} cannot be written: {e}"
            )),
        }
    }

    /// In each « À trier »: one note to tell the person what waits (notes and files), and one note of reasons (tag
    /// `janitor/reasons`) that the `triage_waiting` tool and the page read; none when nothing waits.
    pub(crate) fn leave_waiting_notes(
        &mut self,
        waiting: &BTreeMap<String, Vec<Unplaced>>,
        report: &mut Report,
    ) {
        let (y, m, d, _, _) = time::civil(now(), self.config.utc_offset_minutes);
        for (space, items) in waiting {
            if items.is_empty() {
                self.keep_note("waiting", space, "janitor/waiting", None, report);
                self.keep_note("reasons", space, crate::REASONS_TAG, None, report);
                continue;
            }
            let notes = items.iter().filter(|n| !n.file).count();
            let files = items.len() - notes;
            let first: Vec<String> = items.iter().take(3).map(|n| extract(&n.text, 80)).collect();
            let and_files = if files > 0 {
                format!(" et {files} fichier(s)")
            } else {
                String::new()
            };
            let text = format!(
                "{y:04}-{m:02}-{d:02}, {notes} note(s) attendent dans À trier{and_files} : aucun mot-clé ne les place (la raison de chacun est donnée par triage_waiting). Ajoute des mots-clés au coffre concerné, ou envoie-les depuis ta page. Premiers : {}.",
                first.join(" | ")
            );
            self.keep_note("waiting", space, "janitor/waiting", Some(text), report);
            let reasons: BTreeMap<&str, String> = items
                .iter()
                .take(MAX_REASONS)
                .map(|u| (u.id.as_str(), extract(&u.reason, REASON_CHARS)))
                .collect();
            let doc = json!({ "version": 1, "reasons": reasons }).to_string();
            self.keep_note("reasons", space, crate::REASONS_TAG, Some(doc), report);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::frequent;
    use crate::router::Rules;

    fn notes(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn a_word_common_in_one_space_only_is_frequent_there() {
        let samples = BTreeMap::from([
            (
                "work".to_string(),
                notes(&[
                    "Orionstack deploy planned, tension high",
                    "Orionstack review done, tension low",
                    "Orionstack release notes, tension again",
                ]),
            ),
            (
                "health".to_string(),
                notes(&[
                    "Morning tension reading",
                    "Evening tension reading",
                    "tension",
                ]),
            ),
        ]);
        let found = frequent(&samples, &BTreeMap::new(), 3);
        assert_eq!(found.get("work"), Some(&vec!["orionstack".to_string()]));
        assert!(!found.contains_key("health") || !found["health"].contains(&"tension".to_string()));
        // Below the threshold, or already a keyword: nothing.
        assert!(frequent(&samples, &BTreeMap::new(), 4).is_empty());
        let known = BTreeMap::from([(
            "work".to_string(),
            Rules {
                keywords: vec!["orionstack".into()],
                tags: vec![],
            },
        )]);
        assert!(!frequent(&samples, &known, 3).contains_key("work"));
    }
}
