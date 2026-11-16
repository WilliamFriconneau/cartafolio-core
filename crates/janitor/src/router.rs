//! The deterministic router: which space a note belongs to, from the keywords and tags of each space (the `rules` note
//! of the space: `{"version":1,"tags":[…],"keywords":[…]}`). No model is asked for a note.
//!
//! - A **unique** keyword is one that belongs to a single space (a project, a colleague, a device): a note that holds
//!   it goes to that space at once. Unique keywords of two spaces in one note are an ambiguity: the note stays.
//! - An **ordinary** keyword is listed by several spaces. With no unique keyword in the note, each space scores one point
//!   per keyword or tag of its own the note holds; the best score wins, a tie leaves the note where it is.
//! - No match: the note stays where it is.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

/// The most keywords, and the most tags, one space keeps (the same bound as the person's page).
pub const MAX_ITEMS: usize = 100;

/// The rules of one space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
}

impl Rules {
    /// Reads the text of a `rules` note; `None` when it is not a version 1 document.
    pub fn parse(text: &str) -> Option<Rules> {
        let doc: Value = serde_json::from_str(text.trim()).ok()?;
        if doc.get("version").and_then(Value::as_i64) != Some(1) {
            return None;
        }
        let list = |k: &str| -> Vec<String> {
            doc.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|s| s.trim().trim_start_matches('#').to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        };
        Some(Rules {
            tags: list("tags"),
            keywords: list("keywords"),
        })
    }

    /// The text of the note, in the format the person's page reads and writes.
    pub fn render(&self) -> String {
        json!({ "version": 1, "tags": self.tags, "keywords": self.keywords }).to_string()
    }

    /// These rules with `keywords` and `tags` added (sorted, each once, within the bound); `None` when nothing is new.
    pub fn with(&self, keywords: &[String], tags: &[String]) -> Option<Rules> {
        let grow = |old: &[String], new: &[String]| -> Vec<String> {
            let mut all: BTreeSet<String> = old.iter().cloned().collect();
            for n in new {
                if all.len() < MAX_ITEMS {
                    all.insert(n.clone());
                }
            }
            all.into_iter().collect()
        };
        let next = Rules {
            tags: grow(&self.tags, tags),
            keywords: grow(&self.keywords, keywords),
        };
        (next != *self).then_some(next)
    }
}

/// The words of a text: lowercase runs of letters and digits.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether the words of `phrase` come one after the other in `text`.
fn holds(text: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty() && text.windows(phrase.len()).any(|w| w == phrase)
}

/// A keyword as it may be written in a `rules` note: one to three words of three letters or more (not only digits),
/// 40 characters at most, never a word that says nothing of a subject. `None` otherwise.
pub fn clean_keyword(raw: &str) -> Option<String> {
    let words = tokens(raw.trim().trim_start_matches('#'));
    let fine = (1..=3).contains(&words.len())
        && words.iter().all(|w| {
            w.chars().count() >= 3
                && !w.chars().all(|c| c.is_ascii_digit())
                && !crate::learn::is_stop(w)
        });
    let keyword = words.join(" ");
    (fine && keyword.chars().count() <= 40).then_some(keyword)
}

/// Whether a tag of a note falls under a tag of the rules (the same path, or below it).
fn under(tag: &str, rule: &str) -> bool {
    tag == rule || tag.strip_prefix(rule).is_some_and(|r| r.starts_with('/'))
}

/// The space a note belongs to, or `None` when it is unclear (no match, or two spaces equally good): it stays where it is.
pub fn route(text: &str, tags: &[&str], rules: &BTreeMap<String, Rules>) -> Option<String> {
    let words = tokens(text);
    let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (space, r) in rules {
        for k in &r.keywords {
            owners.entry(k.as_str()).or_default().insert(space.as_str());
        }
    }
    let mut unique: BTreeSet<&str> = BTreeSet::new();
    let mut score: BTreeMap<&str, usize> = BTreeMap::new();
    for (keyword, spaces) in &owners {
        if !holds(&words, &tokens(keyword)) {
            continue;
        }
        if spaces.len() == 1 {
            unique.extend(spaces.iter().copied());
        } else {
            for s in spaces {
                *score.entry(s).or_default() += 1;
            }
        }
    }
    match unique.len() {
        1 => return unique.iter().next().map(|s| (*s).to_string()),
        0 => {}
        _ => return None,
    }
    for (space, r) in rules {
        let hits = r
            .tags
            .iter()
            .filter(|rt| tags.iter().any(|t| under(&t.to_lowercase(), rt)))
            .count();
        if hits > 0 {
            *score.entry(space.as_str()).or_default() += hits;
        }
    }
    let best = score.values().copied().max()?;
    let mut leaders = score.iter().filter(|(_, n)| **n == best);
    match (leaders.next(), leaders.next()) {
        (Some((space, _)), None) => Some((*space).to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::collections::BTreeMap;

    use super::{Rules, clean_keyword, route};

    fn rules(list: &[(&str, &[&str], &[&str])]) -> BTreeMap<String, Rules> {
        list.iter()
            .map(|(s, k, t)| {
                (
                    (*s).to_string(),
                    Rules {
                        keywords: k.iter().map(|x| (*x).to_string()).collect(),
                        tags: t.iter().map(|x| (*x).to_string()).collect(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_unique_keyword_routes_at_once() {
        let r = rules(&[
            ("work", &["orion", "budget"], &[]),
            ("home", &["boiler", "budget"], &[]),
        ]);
        assert_eq!(
            route("Meeting about Orion, and the budget", &[], &r).as_deref(),
            Some("work")
        );
        // Whole words only, any case; a phrase matches word for word.
        assert_eq!(route("The Orionids are visible", &[], &r), None);
        let phrases = rules(&[("home", &["heat pump"], &[])]);
        assert_eq!(
            route("The HEAT PUMP stopped", &[], &phrases).as_deref(),
            Some("home")
        );
        assert_eq!(route("heat or pump", &[], &phrases), None);
    }

    #[test]
    fn two_spaces_with_a_unique_keyword_each_are_an_ambiguity_left_alone() {
        let r = rules(&[("work", &["orion"], &[]), ("home", &["boiler"], &[])]);
        assert_eq!(route("orion and the boiler", &[], &r), None);
        assert_eq!(route("nothing known here", &[], &r), None);
    }

    #[test]
    fn ordinary_words_score_and_a_tie_leaves_the_note() {
        let r = rules(&[
            ("work", &["invoice", "budget", "planning"], &[]),
            ("home", &["invoice", "budget"], &["house"]),
        ]);
        // « planning » is work's own (unique): it wins at once.
        assert_eq!(
            route("planning of the budget", &[], &r).as_deref(),
            Some("work")
        );
        // Ordinary only: a tie, then a tag breaks it.
        assert_eq!(route("the invoice for the budget", &[], &r), None);
        assert_eq!(
            route("the invoice", &["house/repairs"], &r).as_deref(),
            Some("home")
        );
    }

    #[test]
    fn rules_round_trip_and_grow_only_with_something_new() {
        let r = Rules::parse(r#"{"version":1,"tags":["a/b"],"keywords":["Orion"]}"#).expect("v1");
        assert_eq!(r.keywords, ["orion"]);
        assert!(Rules::parse(r#"{"version":2}"#).is_none());
        assert!(r.with(&["orion".into()], &["a/b".into()]).is_none());
        let n = r.with(&["vega".into()], &[]).expect("new");
        assert_eq!(n.keywords, ["orion", "vega"]);
        assert_eq!(Rules::parse(&n.render()), Some(n));
        assert_eq!(clean_keyword(" Heat  Pump "), Some("heat pump".into()));
        assert_eq!(clean_keyword("a"), None);
        assert_eq!(clean_keyword("2026"), None);
        assert_eq!(clean_keyword("avec"), None);
    }
}
