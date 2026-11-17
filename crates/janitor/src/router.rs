//! The deterministic router: which space a note belongs to, from the keywords and tags of each space (the `rules` note
//! of the space: `{"version":1,"tags":[…],"keywords":[…]}`). No model is asked for a note.
//!
//! - A **unique** keyword is one that belongs to a single space (a project, a colleague, a device): a note that holds
//!   it goes to that space at once. Unique keywords of two spaces in one note are an ambiguity: the note stays.
//! - An **ordinary** keyword is listed by several spaces. With no unique keyword in the note, each space scores one point
//!   per keyword or tag of its own the note holds; the best score wins, a tie leaves the note where it is.
//! - No match: the note stays where it is.
//! - **The names of the spaces are tags.** The name and the label of a space (and the name without the person's prefix)
//!   are implicit keywords of that space, matched in the text and in the tags of the note: `#finances` or the word
//!   « finances » is a unique keyword of the space `finances`. Case, accents and hyphens are folded.
//! - **« à ranger dans finances et santé »** (after `dans`, `in`, `ranger` or `file`) names two spaces or more: the note
//!   goes to each of them, instead of being an ambiguity.
//! - **A space with children is a folder.** When a space and one of its descendants both match, the most specific wins;
//!   the parent only gets a note no child matches. A keyword shared by a parent and its child belongs to the child.

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

/// A space as the layout shows it: its name, its label and its parent (when it is a sub-space).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: String,
    pub label: String,
    pub parent: Option<String>,
}

/// A letter without its accent (the ligatures stay as they are).
fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// The words of a text: lowercase runs of letters and digits, without accents.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase().chars().map(fold).collect())
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

/// A name a space answers to, as words: one to three words of three letters or more, no stop word.
fn name_phrase(raw: &str) -> Option<Vec<String>> {
    let words = tokens(raw);
    let fine = (1..=3).contains(&words.len())
        && words.iter().all(|w| {
            w.chars().count() >= 3
                && !w.chars().all(|c| c.is_ascii_digit())
                && !crate::learn::is_stop(w)
        });
    fine.then_some(words)
}

/// What each space answers to: its name, its name without the person's prefix (`sam-cars` → `cars`) and its label.
fn names(layout: &[Place]) -> BTreeMap<Vec<String>, BTreeSet<String>> {
    let mut out: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    for p in layout {
        let short = p.name.split_once('-').map(|(_, rest)| rest);
        for raw in [Some(p.name.as_str()), short, Some(p.label.as_str())]
            .into_iter()
            .flatten()
        {
            if let Some(words) = name_phrase(raw) {
                out.entry(words).or_default().insert(p.name.clone());
            }
        }
    }
    out
}

/// Whether `ancestor` is above `space` in the layout.
fn is_above(layout: &[Place], ancestor: &str, space: &str) -> bool {
    let mut cur = space;
    for _ in 0..16 {
        let Some(parent) = layout
            .iter()
            .find(|p| p.name == cur)
            .and_then(|p| p.parent.as_deref())
        else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        cur = parent;
    }
    false
}

/// Words that may sit between « dans » and the name of a space.
const FILLERS: &[&str] = &[
    "le", "la", "les", "the", "mon", "ma", "mes", "my", "coffre", "espace", "vault", "space",
];

/// The spaces the person names after `dans`, `in`, `ranger` or `file` (« à ranger dans finances et santé »): two or more,
/// in the order said; empty when the text names fewer.
fn named_spaces(words: &[String], names: &BTreeMap<Vec<String>, BTreeSet<String>>) -> Vec<String> {
    for (i, w) in words.iter().enumerate() {
        if !matches!(w.as_str(), "dans" | "in" | "ranger" | "file") {
            continue;
        }
        let mut found: Vec<String> = vec![];
        let mut j = i + 1;
        loop {
            while words.get(j).is_some_and(|x| FILLERS.contains(&x.as_str())) {
                j += 1;
            }
            let hit = (1..=3).rev().find_map(|n| {
                let owners = names.get(words.get(j..j + n)?)?;
                (owners.len() == 1).then(|| (n, owners.iter().next().cloned().unwrap_or_default()))
            });
            let Some((n, space)) = hit else { break };
            if !found.contains(&space) {
                found.push(space);
            }
            j += n;
            if words
                .get(j)
                .is_some_and(|x| matches!(x.as_str(), "et" | "and"))
            {
                j += 1;
            }
        }
        if found.len() >= 2 {
            return found;
        }
    }
    vec![]
}

/// Whether a tag of a note falls under a tag of the rules (the same path, or below it).
fn under(tag: &str, rule: &str) -> bool {
    tag == rule || tag.strip_prefix(rule).is_some_and(|r| r.starts_with('/'))
}

/// The spaces a note belongs to: one, or the several the person names (« à ranger dans finances et santé »); none when it
/// is unclear (no match, or two spaces equally good): it stays where it is. `layout` gives the labels and the parents of
/// the spaces (see the module's rules).
pub fn route(
    text: &str,
    tags: &[&str],
    rules: &BTreeMap<String, Rules>,
    layout: &[Place],
) -> Vec<String> {
    decide(text, tags, rules, layout).unwrap_or_default()
}

/// [`route`], saying why when nothing is chosen: `Err` holds the reason (`no keyword matched`, `two spaces tie: a, b`).
pub fn decide(
    text: &str,
    tags: &[&str],
    rules: &BTreeMap<String, Rules>,
    layout: &[Place],
) -> Result<Vec<String>, String> {
    let words = tokens(text);
    let known = names(layout);
    let named = named_spaces(&words, &known);
    if !named.is_empty() {
        return Ok(named);
    }
    // The tags of the note are read as words too (`#finances`), each one apart.
    let mut seen = words;
    for t in tags {
        seen.push(String::new());
        seen.extend(tokens(t));
    }
    let mut owners: BTreeMap<Vec<String>, BTreeSet<&str>> = BTreeMap::new();
    for (space, r) in rules {
        for k in &r.keywords {
            owners.entry(tokens(k)).or_default().insert(space.as_str());
        }
    }
    for (phrase, spaces) in &known {
        owners
            .entry(phrase.clone())
            .or_default()
            .extend(spaces.iter().map(String::as_str));
    }
    let mut unique: BTreeSet<&str> = BTreeSet::new();
    let mut score: BTreeMap<&str, usize> = BTreeMap::new();
    for (keyword, spaces) in &owners {
        if !holds(&seen, keyword) {
            continue;
        }
        // A keyword a parent and its child both list belongs to the child.
        let kept: Vec<&str> = spaces
            .iter()
            .copied()
            .filter(|s| !spaces.iter().any(|o| is_above(layout, s, o)))
            .collect();
        if let [only] = kept[..] {
            unique.insert(only);
        } else {
            for s in kept {
                *score.entry(s).or_default() += 1;
            }
        }
    }
    let mut by_tag: BTreeMap<&str, usize> = BTreeMap::new();
    for (space, r) in rules {
        let hits = r
            .tags
            .iter()
            .filter(|rt| tags.iter().any(|t| under(&t.to_lowercase(), rt)))
            .count();
        if hits > 0 {
            by_tag.insert(space.as_str(), hits);
        }
    }
    // A space with children is a folder: when one of its descendants matches, it is that one that counts.
    let hit: BTreeSet<&str> = unique
        .iter()
        .copied()
        .chain(score.keys().copied())
        .chain(by_tag.keys().copied())
        .collect();
    let folder = |s: &str| hit.iter().any(|h| is_above(layout, s, h));
    unique.retain(|s| !folder(s));
    score.retain(|s, _| !folder(s));
    match unique.len() {
        1 => return Ok(unique.iter().map(|s| (*s).to_string()).collect()),
        0 => {}
        _ => {
            let names: Vec<&str> = unique.iter().copied().collect();
            return Err(format!(
                "two spaces tie: keywords of {} are all in it",
                names.join(", ")
            ));
        }
    }
    for (space, hits) in by_tag {
        if !folder(space) {
            *score.entry(space).or_default() += hits;
        }
    }
    let Some(best) = score.values().copied().max() else {
        return Err("no keyword matched".into());
    };
    let leaders: Vec<&str> = score
        .iter()
        .filter(|(_, n)| **n == best)
        .map(|(s, _)| *s)
        .collect();
    match leaders[..] {
        [only] => Ok(vec![only.to_string()]),
        _ => Err(format!("two spaces tie: {}", leaders.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::collections::BTreeMap;

    use super::{Place, Rules, clean_keyword, route};

    fn one(text: &str, tags: &[&str], rules: &BTreeMap<String, Rules>) -> Option<String> {
        route(text, tags, rules, &[]).into_iter().next()
    }

    fn place(name: &str, label: &str, parent: Option<&str>) -> Place {
        Place {
            name: name.into(),
            label: label.into(),
            parent: parent.map(String::from),
        }
    }

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
            one("Meeting about Orion, and the budget", &[], &r).as_deref(),
            Some("work")
        );
        // Whole words only, any case; a phrase matches word for word.
        assert_eq!(one("The Orionids are visible", &[], &r), None);
        let phrases = rules(&[("home", &["heat pump"], &[])]);
        assert_eq!(
            one("The HEAT PUMP stopped", &[], &phrases).as_deref(),
            Some("home")
        );
        assert_eq!(one("heat or pump", &[], &phrases), None);
    }

    #[test]
    fn two_spaces_with_a_unique_keyword_each_are_an_ambiguity_left_alone() {
        let r = rules(&[("work", &["orion"], &[]), ("home", &["boiler"], &[])]);
        assert_eq!(one("orion and the boiler", &[], &r), None);
        assert_eq!(one("nothing known here", &[], &r), None);
    }

    #[test]
    fn ordinary_words_score_and_a_tie_leaves_the_note() {
        let r = rules(&[
            ("work", &["invoice", "budget", "planning"], &[]),
            ("home", &["invoice", "budget"], &["house"]),
        ]);
        // « planning » is work's own (unique): it wins at once.
        assert_eq!(
            one("planning of the budget", &[], &r).as_deref(),
            Some("work")
        );
        // Ordinary only: a tie, then a tag breaks it.
        assert_eq!(one("the invoice for the budget", &[], &r), None);
        assert_eq!(
            one("the invoice", &["house/repairs"], &r).as_deref(),
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

    #[test]
    fn the_name_and_the_label_of_a_space_are_its_keywords_accents_and_hyphens_folded() {
        let layout = [
            place("sam-finances", "Finances", None),
            place("sam-cars", "Voitures", None),
            place("sam-home-office", "Bureau", None),
        ];
        let none = BTreeMap::new();
        let go = |text: &str, tags: &[&str]| route(text, tags, &none, &layout);
        assert_eq!(go("Budget du mois", &["finances"]), ["sam-finances"]);
        assert_eq!(go("Les FINANCES du mois", &[]), ["sam-finances"]);
        assert_eq!(go("Pneus de la voiture, voitures", &[]), ["sam-cars"]);
        assert_eq!(go("Révision des Cars", &[]), ["sam-cars"]);
        assert_eq!(go("le bureau est rangé", &[]), ["sam-home-office"]);
        assert!(go("Rien ici", &[]).is_empty());
        // Two spaces named in one note, with no wording to file in both: an ambiguity.
        assert!(go("finances et voitures", &[]).is_empty());
    }

    #[test]
    fn a_phrase_naming_several_spaces_files_in_each() {
        let layout = [
            place("sam-finances", "Finances", None),
            place("sam-health", "Santé", None),
            place("sam-cars", "Cars", None),
        ];
        let none = BTreeMap::new();
        let go = |text: &str| route(text, &[], &none, &layout);
        assert_eq!(
            go("Ordonnance à ranger dans finances et santé"),
            ["sam-finances", "sam-health"]
        );
        assert_eq!(
            go("File in the health and finances, cars"),
            ["sam-health", "sam-finances", "sam-cars"]
        );
        // One name only: the ordinary match.
        assert_eq!(go("à ranger dans santé"), ["sam-health"]);
        // Without the wording, the two names are an ambiguity.
        assert!(go("santé et finances").is_empty());
    }

    #[test]
    fn the_most_specific_space_wins_and_a_folder_is_never_the_target() {
        let layout = [
            place("sam-finances", "Finances", None),
            place("sam-cars", "Cars", Some("sam-finances")),
            place("sam-leasing", "Leasing", Some("sam-cars")),
        ];
        let r = rules(&[
            ("sam-finances", &["budget", "facture"], &[]),
            ("sam-cars", &["facture", "pneus"], &[]),
        ]);
        let go = |text: &str, tags: &[&str]| route(text, tags, &r, &layout);
        // Parent and child keywords in one note: the child.
        assert_eq!(go("budget et pneus", &[]), ["sam-cars"]);
        // A keyword shared by the parent and its child is the child's.
        assert_eq!(go("la facture", &[]), ["sam-cars"]);
        // Deeper still.
        assert_eq!(go("budget leasing", &[]), ["sam-leasing"]);
        // No child matches: the parent.
        assert_eq!(go("le budget", &[]), ["sam-finances"]);
        // A tag of a child counts the same way.
        assert_eq!(go("le budget", &["cars"]), ["sam-cars"]);
    }
}
