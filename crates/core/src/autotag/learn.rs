//! What the router learns from the person's corrections: which words of a note lead to which tags (per space), and
//! which words lead to which space. Same idea as the janitor's learned words, with one difference: here the teacher is the
//! person, who is right by definition, so two corrections make a word sure.
//!
//! - A person adds a tag to a note, or removes one: each word of the note counts for the added tags (`t`), against the
//!   removed ones (`x`).
//! - A person sends a note of Inbox to a space: each word counts for that space (`routes`).
//! - Per space, and bounded: [`MAX_WORDS`] words per space, [`MAX_TAGS_PER_WORD`] tags per word. The least seen are
//!   forgotten first.
//! - The memory's own plumbing (inbox, sources, janitor…) is never learned, and neither is what the janitor did.
//!
//! The store is one small JSON file beside the databases (`autotag-learned.json`); losing it only makes the node forget.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};

/// Words kept per space (and for the routes), the most seen first.
pub const MAX_WORDS: usize = 2000;
/// Tags kept per word.
pub const MAX_TAGS_PER_WORD: usize = 8;
/// Words of one note that teach.
pub const WORDS_PER_NOTE: usize = 30;
/// Times a word must have been corrected before it decides with another word.
pub const MIN_SEEN: u64 = 2;
/// Times a single word must have been corrected before it decides alone.
pub const ALONE: u64 = 4;

static LOCK: Mutex<()> = Mutex::new(());

const STOP: [&str; 24] = [
    "avec", "pour", "dans", "plus", "mais", "cette", "fait", "sont", "elle", "nous", "vous",
    "leur", "aussi", "comme", "tout", "tous", "with", "that", "this", "from", "have", "were",
    "will", "your",
];

/// What the learning says of a note, for one space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// Tags the person gave to notes like this one.
    pub add: Vec<String>,
    /// Tags the person took off notes like this one.
    pub deny: BTreeSet<String>,
}

/// The words of a note that may say its subject: folded, four letters or more, no stop word, each once.
pub fn learn_words(text: &str) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for w in super::words(text) {
        if w.chars().count() >= 4
            && !w.chars().all(|c| c.is_ascii_digit())
            && !STOP.contains(&w.as_str())
            && !crate::text::fold(&w).is_empty()
        {
            seen.insert(w);
        }
    }
    let mut v: Vec<String> = seen.into_iter().collect();
    v.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
    v.truncate(WORDS_PER_NOTE);
    v.sort();
    v
}

/// The learned words of a node.
#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// The store of the node whose data directory is `dir`.
    pub fn open(dir: &Path) -> Store {
        Store {
            path: dir.join("autotag-learned.json"),
        }
    }

    fn load(&self) -> Value {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "version": 1, "spaces": {}, "routes": {} }))
    }

    fn save(&self, data: &Value) {
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, data.to_string()).is_ok() {
            let _ = crate::fsx::set_private_file(&tmp);
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// The person put `added` on, and took `removed` off, a note of `space` whose text is `text`. Structural tags are
    /// left out. Returns whether something was learned.
    pub fn learn_tags(
        &self,
        space: &str,
        text: &str,
        added: &[String],
        removed: &[String],
    ) -> bool {
        let added: Vec<&String> = added.iter().filter(|t| !super::is_structural(t)).collect();
        let removed: Vec<&String> = removed
            .iter()
            .filter(|t| !super::is_structural(t))
            .collect();
        let words = learn_words(text);
        if (added.is_empty() && removed.is_empty()) || words.is_empty() {
            return false;
        }
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = self.load();
        let bag = &mut data["spaces"][space];
        if !bag.is_object() {
            *bag = json!({});
        }
        for w in &words {
            let e = &mut bag[w];
            for (key, tags) in [("t", &added), ("x", &removed)] {
                for t in tags {
                    e[key][t.as_str()] = json!(e[key][t.as_str()].as_u64().unwrap_or(0) + 1);
                }
            }
            // A tag put on after it was taken off counts against the earlier refusal, and the other way round.
            for t in &added {
                if let Some(x) = e["x"][t.as_str()].as_u64() {
                    e["x"][t.as_str()] = json!(x.saturating_sub(1));
                }
            }
            for t in &removed {
                if let Some(c) = e["t"][t.as_str()].as_u64() {
                    e["t"][t.as_str()] = json!(c.saturating_sub(1));
                }
            }
            trim_tags(e);
        }
        bound(bag);
        self.save(&data);
        true
    }

    /// The person sent a note whose text is `text` to the space `to`.
    pub fn learn_route(&self, text: &str, to: &str) -> bool {
        let words = learn_words(text);
        if words.is_empty() {
            return false;
        }
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = self.load();
        let bag = &mut data["routes"];
        if !bag.is_object() {
            *bag = json!({});
        }
        for w in &words {
            let e = &mut bag[w];
            e[to] = json!(e[to].as_u64().unwrap_or(0) + 1);
            if let Some(o) = e.as_object_mut()
                && o.len() > MAX_TAGS_PER_WORD
                && let Some(least) = o
                    .iter()
                    .min_by_key(|(k, v)| (v.as_u64().unwrap_or(0), (*k).clone()))
                    .map(|(k, _)| k.clone())
            {
                o.remove(&least);
            }
        }
        bound(bag);
        self.save(&data);
        true
    }

    /// What the person taught for a note of `space`.
    pub fn hints(&self, space: &str, text: &str) -> Hints {
        let data = self.load();
        let bag = &data["spaces"][space];
        let mut votes: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        let mut deny = BTreeSet::new();
        for w in learn_words(text) {
            let e = &bag[&w];
            for (t, c) in e["t"].as_object().into_iter().flatten() {
                let c = c.as_u64().unwrap_or(0);
                let x = e["x"][t].as_u64().unwrap_or(0);
                if c >= MIN_SEEN && x * 3 <= c {
                    let v = votes.entry(t.clone()).or_insert((0, 0));
                    v.0 += 1;
                    v.1 = v.1.max(c);
                }
            }
            for (t, x) in e["x"].as_object().into_iter().flatten() {
                let x = x.as_u64().unwrap_or(0);
                let c = e["t"][t].as_u64().unwrap_or(0);
                if x >= MIN_SEEN && x > c {
                    deny.insert(t.clone());
                }
            }
        }
        let add = votes
            .into_iter()
            .filter(|(t, (words, best))| !deny.contains(t) && (*words >= 2 || *best >= ALONE))
            .map(|(t, _)| t)
            .collect();
        Hints { add, deny }
    }

    /// The spaces the person sent notes like this one to, best first, with their score (the corrections of the words).
    /// Empty when no space is sure: at least two words each corrected twice, and the first space twice the second.
    pub fn route(&self, text: &str) -> Vec<(String, u64)> {
        let data = self.load();
        let bag = &data["routes"];
        let mut by_space: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for w in learn_words(text) {
            for (s, c) in bag[&w].as_object().into_iter().flatten() {
                let c = c.as_u64().unwrap_or(0);
                if c >= MIN_SEEN {
                    let v = by_space.entry(s.clone()).or_insert((0, 0));
                    v.0 += 1;
                    v.1 += c;
                }
            }
        }
        let mut ranked: Vec<(String, (usize, u64))> =
            by_space.into_iter().filter(|(_, (n, _))| *n >= 2).collect();
        ranked.sort_by(|a, b| b.1.1.cmp(&a.1.1).then_with(|| a.0.cmp(&b.0)));
        if ranked.len() > 1 && ranked[0].1.1 < 2 * ranked[1].1.1 {
            return vec![];
        }
        ranked.into_iter().map(|(s, (_, c))| (s, c)).collect()
    }
}

fn trim_tags(e: &mut Value) {
    for key in ["t", "x"] {
        let Some(o) = e[key].as_object_mut() else {
            continue;
        };
        o.retain(|_, v| v.as_u64().unwrap_or(0) > 0);
        while o.len() > MAX_TAGS_PER_WORD {
            let Some(least) = o
                .iter()
                .min_by_key(|(k, v)| (v.as_u64().unwrap_or(0), (*k).clone()))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            o.remove(&least);
        }
        if o.is_empty()
            && let Some(parent) = e.as_object_mut()
        {
            parent.remove(key);
        }
    }
}

fn weight(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::Object(o) => o.values().map(weight).sum(),
        _ => 0,
    }
}

/// Past [`MAX_WORDS`] words, the least seen are forgotten.
fn bound(bag: &mut Value) {
    let Some(o) = bag.as_object_mut() else {
        return;
    };
    if o.len() <= MAX_WORDS {
        return;
    }
    let mut seen: Vec<(String, u64)> = o.iter().map(|(w, e)| (w.clone(), weight(e))).collect();
    seen.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (w, _) in seen.into_iter().skip(MAX_WORDS) {
        o.remove(&w);
    }
}
