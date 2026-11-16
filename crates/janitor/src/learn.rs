//! What the janitor learns from the model, so that it needs it less each time: which words of a note lead to which tags
//! (and, in À trier, to which vault), vault by vault, each association with a score — how often it was seen and
//! how often it was right.
//!
//! - It learns only from the model's choices, never from its own: a mistake of its own cannot feed itself.
//! - It files a note without the model when every tag it would give, and the vault, is sure: seen at least [`MIN_SEEN`]
//!   times and right [`SURE`] of the time.
//! - One time in [`CHECK_EVERY`] it asks the model anyway, and a disagreement lowers the score of the words that misled it.
//! - Nothing of a vault is learned into another: each vault has its own words.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

/// Times a word must have been seen before it decides anything.
pub const MIN_SEEN: u64 = 5;
/// The share of times a word must have been right.
pub const SURE: f64 = 0.9;
/// One time in this many, a note the janitor could file alone is checked by the model.
pub const CHECK_EVERY: u64 = 10;
/// Words kept per vault, the most seen first, past which the rarest are forgotten.
const MAX_WORDS: usize = 5000;

/// Words that say nothing of a subject.
const STOP: [&str; 76] = [
    "avec", "pour", "dans", "plus", "mais", "cette", "être", "avoir", "fait", "faire", "sont",
    "était", "elle", "nous", "vous", "leur", "leurs", "aussi", "comme", "tout", "tous", "toute",
    "très", "bien", "encore", "après", "avant", "depuis", "entre", "sans", "sous", "chez", "quand",
    "alors", "donc", "parce", "peut", "faut", "suis", "ceci", "cela", "dont", "moins", "même",
    "autre", "autres", "note", "notes", "dictée", "audio", "partie", "matin", "soir", "hier",
    "demain", "aujourd", "with", "that", "this", "from", "have", "were", "will", "your", "merci",
    "chercher", "prendre", "plutôt", "instant", "sauf", "voilà", "enfin", "chose", "choses",
    "juste", "vraiment",
];

/// Sure words a note needs before it is filed without the model, and the share of its known words they must be.
pub const MIN_SURE_WORDS: usize = 2;
pub const SURE_SHARE: f64 = 0.5;

/// A note that is only a loop of the speech model (« Merci. Merci. Merci. » for minutes): few distinct words for many.
pub fn is_loop(text: &str) -> bool {
    let all: Vec<String> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if all.len() < 12 {
        return false;
    }
    let distinct: BTreeSet<&String> = all.iter().collect();
    (distinct.len() as f64) / (all.len() as f64) < 0.3
}

/// A dictation is cut into overlapping pieces (« (partie 3/8) »): the same speech is said several times, so only its first
/// piece is learned from, and a loop of the speech model is never learned from.
pub fn worth_learning(text: &str) -> bool {
    if is_loop(text) {
        return false;
    }
    match text
        .split("(partie ")
        .nth(1)
        .and_then(|r| r.split('/').next())
    {
        Some(k) => k.trim() == "1",
        None => true,
    }
}

/// The words of a note that may say its subject: lowercase, letters only, four letters or more, each once.
pub fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphabetic())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 4 && !STOP.contains(&w.as_str()))
        .collect()
}

/// What a vault has learned: `{word: {n, tags: {tag: c}, vaults: {vault: c}}}`.
pub struct Learned<'a> {
    pub data: &'a mut Value,
}

impl Learned<'_> {
    /// The model chose `tags` (and `vault`, in À trier) for a note made of `words`.
    pub fn learn(&mut self, words: &BTreeSet<String>, tags: &[String], vault: Option<&str>) {
        if !self.data.is_object() {
            *self.data = json!({});
        }
        for w in words {
            let e = &mut self.data[w];
            e["n"] = json!(e["n"].as_u64().unwrap_or(0) + 1);
            for t in tags {
                e["tags"][t] = json!(e["tags"][t].as_u64().unwrap_or(0) + 1);
            }
            if let Some(v) = vault {
                e["vaults"][v] = json!(e["vaults"][v].as_u64().unwrap_or(0) + 1);
            }
        }
        self.forget_rare();
    }

    fn forget_rare(&mut self) {
        let Some(o) = self.data.as_object_mut() else {
            return;
        };
        if o.len() <= MAX_WORDS {
            return;
        }
        let mut seen: Vec<(String, u64)> = o
            .iter()
            .map(|(w, e)| (w.clone(), e["n"].as_u64().unwrap_or(0)))
            .collect();
        seen.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (w, _) in seen.into_iter().skip(MAX_WORDS) {
            o.remove(&w);
        }
    }

    /// What its sure words say of a note: the tags (at most three), the vault if `with_vault`, each with its score.
    /// `None` when nothing sure is known, or when the vault was asked for and none is sure.
    pub fn guess(&self, words: &BTreeSet<String>, with_vault: bool) -> Option<Guess> {
        let mut tags: BTreeMap<String, f64> = BTreeMap::new();
        let mut vaults: BTreeMap<String, f64> = BTreeMap::new();
        let mut led_by: BTreeSet<String> = BTreeSet::new();
        let mut known = 0_usize;
        for w in words {
            let e = &self.data[w];
            let n = e["n"].as_u64().unwrap_or(0);
            if n < MIN_SEEN {
                continue;
            }
            known += 1;
            for (key, out) in [("tags", &mut tags), ("vaults", &mut vaults)] {
                for (t, c) in e[key].as_object().into_iter().flatten() {
                    let score = c.as_u64().unwrap_or(0) as f64 / n as f64;
                    if score >= SURE {
                        let best = out.entry(t.clone()).or_insert(0.0);
                        if score > *best {
                            *best = score;
                        }
                        led_by.insert(w.clone());
                    }
                }
            }
        }
        // One sure word is not enough, and the sure words must be most of the words it knows: a word seen everywhere
        // (« travail ») must not decide alone.
        if tags.is_empty()
            || led_by.len() < MIN_SURE_WORDS
            || (led_by.len() as f64) < SURE_SHARE * known as f64
        {
            return None;
        }
        let vault = if with_vault {
            // One sure vault, or no guess at all.
            if vaults.len() != 1 {
                return None;
            }
            vaults.keys().next().cloned()
        } else {
            None
        };
        let mut ranked: Vec<(String, f64)> = tags.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(3);
        Some(Guess {
            tags: ranked,
            vault,
            led_by: led_by.into_iter().collect(),
        })
    }

    /// The model disagreed with a guess: the words that led to it are counted once more, with no right answer, so their
    /// score falls.
    pub fn wrong(&mut self, led_by: &[String]) {
        for w in led_by {
            let e = &mut self.data[w];
            e["n"] = json!(e["n"].as_u64().unwrap_or(0) + 1);
        }
    }

    /// The surest associations, for the night's note: `word → tag (right c of n times)`.
    pub fn best(&self, limit: usize) -> Vec<String> {
        let mut out: Vec<(f64, u64, String)> = vec![];
        for (w, e) in self.data.as_object().into_iter().flatten() {
            let n = e["n"].as_u64().unwrap_or(0);
            if n < MIN_SEEN {
                continue;
            }
            for (t, c) in e["tags"].as_object().into_iter().flatten() {
                let c = c.as_u64().unwrap_or(0);
                let score = c as f64 / n as f64;
                if score >= SURE {
                    out.push((score, n, format!("{w} → {t} ({c} of {n})")));
                }
            }
        }
        out.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        out.into_iter().take(limit).map(|(_, _, s)| s).collect()
    }
}

/// What the learned words say of a note.
#[derive(Debug, Clone, PartialEq)]
pub struct Guess {
    pub tags: Vec<(String, f64)>,
    pub vault: Option<String>,
    /// The words that led to it (lowered if the model disagrees).
    pub led_by: Vec<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{Learned, MIN_SEEN, is_loop, words, worth_learning};
    use serde_json::json;

    #[test]
    fn it_files_alone_only_what_it_is_sure_of_and_a_disagreement_lowers_the_score() {
        let mut data = json!({});
        let mut l = Learned { data: &mut data };
        for i in 0..MIN_SEEN {
            let w = words(&format!("La batterie Zendure n'a pas chargé, essai {i}"));
            l.learn(&w, &["home/energie".into()], None);
        }
        let note = words("Batterie Zendure encore en défaut ce soir");
        let g = l.guess(&note, false).expect("sure");
        assert_eq!(g.tags[0].0, "home/energie");
        assert!(g.led_by.contains(&"zendure".to_string()));
        // A word seen once decides nothing.
        assert!(l.guess(&words("Une recette de crêpes"), false).is_none());
        // In À trier a vault is needed too, and none was learned: no guess.
        assert!(l.guess(&note, true).is_none());
        // One disagreement: right 5 times of 6, under the sure share, so it asks the model again.
        l.wrong(&g.led_by);
        assert!(l.guess(&note, false).is_none());
    }

    #[test]
    fn one_sure_word_is_not_enough_and_a_loop_or_an_overlapping_piece_teaches_nothing() {
        let mut data = json!({});
        let mut l = Learned { data: &mut data };
        for i in 0..MIN_SEEN {
            l.learn(
                &words(&format!("Travail du lundi {i}")),
                &["work".into()],
                Some("work"),
            );
        }
        // « travail » alone is seen everywhere: no guess from it.
        assert!(l.guess(&words("Un travail sur mon dos"), true).is_none());
        for i in 0..MIN_SEEN {
            l.learn(
                &words(&format!("Travail Zendure batterie {i}")),
                &["home/energie".into()],
                Some("home"),
            );
        }
        assert!(l.guess(&words("Zendure batterie défaut"), false).is_some());
        let looped = "Merci. ".repeat(40);
        assert!(is_loop(&looped));
        assert!(!is_loop(
            "Je pense que la batterie est vide parce que le soleil manque depuis trois jours."
        ));
        assert!(!worth_learning(&looped));
        assert!(!worth_learning(
            "Dictée du 2026-11-12 (partie 3/8) : une phrase assez normale."
        ));
        assert!(worth_learning(
            "Dictée du 2026-11-12 (partie 1/8) : une phrase assez normale."
        ));
        assert!(worth_learning("Une note sans découpage."));
    }
}
