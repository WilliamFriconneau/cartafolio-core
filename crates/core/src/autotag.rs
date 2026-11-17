//! Automatic tagging: what a node tags a note with when it is written, with no model.
//!
//! [`suggest`] is pure and deterministic. For one note of one space it gives, first, **the tag of the space** (its name),
//! then the tags proper to that space: the keywords and tags of its `rules` note found in the text, the terms of its
//! vocabularies found in the text and, for a health space, a small built-in set of French and English patterns:
//! **signs and symptoms** (`signe/douleur`, with the level as said: `level` = `2/10`) and **measures** (`mesure/tension`,
//! with the value as said: `tension` = `12/8`; blood pressure, weight, pulse, blood sugar, temperature, saturation).
//!
//! A *field* is a name and a value kept as said. It is stored as a tag, `field/<name>/<value>` (a `/` in the value is a
//! level of the path, a decimal point is `_`): `field/level/2/10`, `field/tension/12/8`, `field/poids/72_5`; a unit is the
//! field `<name>_unit` (`field/poids_unit/kg`). [`fields_of`] reads them back from a list of tags.
//!
//! The node applies the suggestion when a note is written (it only adds tags, never removes the caller's), and learns from
//! the corrections of the person (see [`learn`]). Nothing here reads the clock, the disk or a model.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::rules::Rules;

pub mod learn;
mod patterns;

pub use learn::{Hints, Store};

/// The most tags one suggestion adds besides the tag of the space.
pub const MAX_SUGGESTED: usize = 16;
/// The prefix of the tags that hold a field.
pub const FIELD_PREFIX: &str = "field/";

/// A space as the tagger needs to know it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceInfo {
    /// Its name (the file on disk, never changes). It is the tag of the space.
    pub name: String,
    /// What the person sees (the label); may be empty.
    pub label: String,
    /// The kind of vault (`health`, `home`…; `custom` or empty when unknown).
    pub kind: String,
}

impl SpaceInfo {
    /// Whether the built-in health patterns apply.
    pub fn is_health(&self) -> bool {
        let name = crate::text::fold(&self.name);
        let label = crate::text::fold(&self.label);
        self.kind == "health"
            || name == "health"
            || name.ends_with("-health")
            || name == "sante"
            || name.ends_with("-sante")
            || label == "health"
            || label == "sante"
    }

    /// The tag of the space: its name, as a tag.
    pub fn tag(&self) -> Option<String> {
        crate::facts::normalize_tag(&self.name).ok()
    }
}

/// What the tagger found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestion {
    /// The tags, the tag of the space first.
    pub tags: Vec<String>,
    /// The fields, `name → value as said`.
    pub fields: BTreeMap<String, String>,
}

impl Suggestion {
    /// The fields as tags (`field/level/2/10`).
    pub fn field_tags(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter_map(|(k, v)| field_tag(k, v))
            .collect()
    }

    /// Every tag the node writes: the tags, then the fields as tags, each once.
    pub fn all_tags(&self) -> Vec<String> {
        let mut out = self.tags.clone();
        for t in self.field_tags() {
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }

    /// The JSON form: `{"tags": [...], "fields": {...}, "field_tags": [...]}`.
    pub fn to_json(&self) -> Value {
        json!({ "tags": self.tags, "fields": self.fields, "field_tags": self.field_tags() })
    }

    /// What the person taught (see [`learn`]): the tags they added to notes like this one come in, the tags they
    /// removed from notes like this one go out. The tag of the space is never taken out.
    pub fn with_hints(mut self, hints: &Hints, space_tag: Option<&str>) -> Suggestion {
        self.tags
            .retain(|t| Some(t.as_str()) == space_tag || !hints.deny.contains(t));
        for t in &hints.add {
            if !self.tags.contains(t) && self.tags.len() < MAX_SUGGESTED + 1 {
                self.tags.push(t.clone());
            }
        }
        self
    }
}

/// A field as a tag: `field/<name>/<value>`; `None` when it cannot be one.
pub fn field_tag(name: &str, value: &str) -> Option<String> {
    let name: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let value: String = value
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            ',' | '.' => '_',
            c if c.is_whitespace() => '-',
            c => c,
        })
        .collect();
    if name.is_empty() || value.is_empty() {
        return None;
    }
    crate::facts::normalize_tag(&format!("{FIELD_PREFIX}{name}/{value}")).ok()
}

/// The fields held by a list of tags (`field/tension/12/8` gives `tension = 12/8`).
pub fn fields_of(tags: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for t in tags {
        let Some(rest) = t.strip_prefix(FIELD_PREFIX) else {
            continue;
        };
        let Some((name, value)) = rest.split_once('/') else {
            continue;
        };
        out.insert(name.to_string(), value.replace('_', "."));
    }
    out
}

/// Whether a tag is structural (the memory's own plumbing): it is never learned from and a write that carries one is left
/// as it is.
pub fn is_structural(tag: &str) -> bool {
    matches!(
        tag,
        "inbox" | "rules" | "index" | "guide" | "dashboard" | "janitor"
    ) || tag.starts_with("inbox/")
        || tag.starts_with("janitor/")
        || tag.starts_with("living/")
        || tag.starts_with("living-meta/")
        || tag.starts_with("sources/")
        || tag.starts_with("dossier/")
        || tag.starts_with("proposals/")
        || tag.starts_with("code/")
        || tag.starts_with(FIELD_PREFIX)
        || tag.starts_with('@')
}

/// Whether a tag on a write says the note is the memory's own plumbing (the rules of a space, an index, a living card,
/// the janitor's own notes): the node leaves such a write as it is.
pub fn is_structural_write(tag: &str) -> bool {
    matches!(tag, "rules" | "index" | "guide" | "dashboard")
        || tag.starts_with("janitor/")
        || tag.starts_with("living/")
        || tag.starts_with("living-meta/")
        || tag.starts_with("proposals/")
}

/// The person changed the tags of a note of `space` (`old` to `new`): the node learns it. What the janitor does is never
/// learned from (it is a program, not the person). Whether anything was learned.
pub fn learn_change(
    brain: &crate::Brain,
    agent: &str,
    space: &crate::Space,
    text: &str,
    old: &[String],
    new: &[String],
) -> bool {
    if agent.to_lowercase().contains("janitor") {
        return false;
    }
    let added: Vec<String> = new.iter().filter(|t| !old.contains(t)).cloned().collect();
    let removed: Vec<String> = old.iter().filter(|t| !new.contains(t)).cloned().collect();
    Store::open(brain.dir()).learn_tags(space.as_str(), text, &added, &removed)
}

/// The words of a text as the tagger compares them: lowercase, without accents, letters and digits.
pub(crate) fn words(text: &str) -> Vec<String> {
    crate::text::fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn phrase_words(raw: &str) -> Vec<String> {
    words(raw.trim().trim_start_matches('#'))
}

fn holds(text: &[String], phrase: &[String], loose: bool) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let same = |a: &String, b: &String| a == b || (loose && stem(a) == stem(b));
    text.windows(phrase.len())
        .any(|w| w.iter().zip(phrase).all(|(a, b)| same(a, b)))
}

/// A word without a plural mark (vocabularies only: « céphalées » finds « céphalée »).
fn stem(w: &str) -> &str {
    if w.chars().count() > 4 {
        w.strip_suffix('s')
            .or_else(|| w.strip_suffix('x'))
            .unwrap_or(w)
    } else {
        w
    }
}

fn slug(raw: &str) -> Option<String> {
    let w = phrase_words(raw);
    if w.is_empty() {
        return None;
    }
    crate::facts::normalize_tag(&w.join("-")).ok()
}

/// The deterministic tagger. `vocab` is the terms of the space's vocabularies found in the text (or all of them: only
/// those the text holds count). Pure.
pub fn suggest(space: &SpaceInfo, rules: &Rules, vocab: &[String], text: &str) -> Suggestion {
    let mut out = Suggestion::default();
    let space_tag = space.tag();
    if let Some(t) = &space_tag {
        out.tags.push(t.clone());
    }
    let room = MAX_SUGGESTED + usize::from(space_tag.is_some());
    let push = |tags: &mut Vec<String>, t: Option<String>| {
        if let Some(t) = t
            && !tags.contains(&t)
            && tags.len() < room
        {
            tags.push(t);
        }
    };
    let text_words = words(text);
    // The keywords and tags the space lists.
    for k in &rules.keywords {
        if holds(&text_words, &phrase_words(k), false) {
            push(&mut out.tags, slug(k));
        }
    }
    for t in &rules.tags {
        let last = t.rsplit('/').next().unwrap_or(t).replace(['-', '_'], " ");
        if holds(&text_words, &phrase_words(&last), false) {
            push(&mut out.tags, crate::facts::normalize_tag(t).ok());
        }
    }
    let health = space.is_health();
    // The terms of the vocabularies: signs and symptoms in a health space, plain words elsewhere.
    for term in vocab {
        if holds(&text_words, &phrase_words(term), true)
            && let Some(s) = slug(term)
        {
            let tag = if health { format!("signe/{s}") } else { s };
            push(&mut out.tags, crate::facts::normalize_tag(&tag).ok());
        }
    }
    if health {
        let found = patterns::health(text);
        for t in found.tags {
            push(&mut out.tags, Some(t));
        }
        out.fields = found.fields;
    }
    out
}

/// The candidate terms of a text for a vocabulary lookup: every run of one to three words, lowercase, as written and
/// without accents, each once. For the node, which asks its table for those that exist.
pub fn candidate_terms(text: &str) -> Vec<String> {
    let raw: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    let folded = words(text);
    let mut out = BTreeSet::new();
    for list in [&raw, &folded] {
        for n in 1..=3 {
            for w in list.windows(n) {
                let t = w.join(" ");
                if let Some(s) = t.strip_suffix('s') {
                    out.insert(s.to_string());
                }
                out.insert(t);
            }
        }
    }
    out.into_iter().take(1500).collect()
}

/// The suggestion for a text written in `space` for the person `user_id`, as the node gives it: the tagger with the rules
/// note, the vocabularies and the layout of the space, then what the person taught.
pub fn for_note(
    brain: &crate::Brain,
    user_id: &str,
    space: &crate::Space,
    conn: &rusqlite::Connection,
    text: &str,
) -> Suggestion {
    let label = brain
        .layout(user_id)
        .ok()
        .and_then(|l| l.into_iter().find(|p| p.space == *space))
        .map(|p| p.label)
        .unwrap_or_default();
    let info = SpaceInfo {
        name: space.as_str().to_string(),
        label,
        kind: brain.space_kind(space).unwrap_or_default(),
    };
    let (_, rules) = crate::rules::read(conn);
    let vocab = crate::vocabulary::matching(conn, &candidate_terms(text)).unwrap_or_default();
    let hints = Store::open(brain.dir()).hints(space.as_str(), text);
    let tag = info.tag();
    suggest(&info, &rules, &vocab, text).with_hints(&hints, tag.as_deref())
}

/// Adds to `tags` what the node finds in `text` for `space`: the tag of the space, then the tags proper to it, and the
/// fields as tags. Never removes a tag the caller gave. Nothing is added to Inbox, or to a write that carries a
/// plumbing tag (rules, index, a living card, the janitor's own notes). Returns the tags added.
pub fn add_to(
    brain: &crate::Brain,
    user_id: &str,
    space: &crate::Space,
    conn: &rusqlite::Connection,
    text: &str,
    tags: &mut Vec<String>,
) -> Vec<String> {
    if crate::triage::is_triage(brain, space) || tags.iter().any(|t| is_structural_write(t)) {
        return vec![];
    }
    let mut added = vec![];
    for t in for_note(brain, user_id, space, conn, text).all_tags() {
        if tags.len() >= crate::facts::MAX_TAGS {
            break;
        }
        if !tags.contains(&t) {
            tags.push(t.clone());
            added.push(t);
        }
    }
    added
}

#[cfg(test)]
mod tests;
