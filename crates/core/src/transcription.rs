//! Guided transcription: a short, ordered list of the words and phrases a speech model should expect, drawn from the
//! memory of the person, with no external model. A node gives it to Whisper as its initial prompt and an app gives it to
//! Apple's speech recogniser as `contextualStrings`.
//!
//! Four groups, in this order, each ordered by usefulness (recent and frequent first, specific before generic):
//!
//! - **names**: what recurs in the notes and looks like a name (a run of capitalised words said at least twice, never a
//!   capital that only starts a sentence, a day or a month) and the tags `personnes/…`, `lieux/…`, `produits/…`,
//!   `organisations/…` with their counts;
//! - **keywords**: the keywords of the `rules` note of each space, the most used first;
//! - **vocabulary**: the terms of the vocabularies, the most used first, then the shortest first;
//! - **health**: the signs and measures of the built-in health patterns (only for a health space the caller reads).
//!
//! Each space is worked out once and kept (see [`builds`]); the cache is rebuilt for a space when its fingerprint changed (a
//! counter of the changes made to its notes, rules and vocabularies) or after an hour (the recent notes weigh more than the
//! old ones). The answer for a caller is assembled from the spaces it reads: nothing of a space the caller does not read
//! reaches the list.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::autotag::{self, SpaceInfo};
use crate::brain::Brain;
use crate::error::Result;
use crate::space::Space;
use crate::util;

/// The words given when the caller does not say.
pub const DEFAULT_LIMIT: usize = 120;
/// The most words ever given.
pub const MAX_LIMIT: usize = 500;
/// The longest item, in characters; a longer one is left out.
pub const ITEM_MAX: usize = 40;
/// The most tokens of the `text` (a speech model reads about 224 of them).
pub const PROMPT_TOKENS: usize = 220;
/// The most recent notes read for each space.
const SCAN: usize = 4000;
/// A space's list is worked out again after this many seconds, whatever changed.
const REBUILD_AFTER: i64 = 3600;
/// The most items kept per group and per space.
const PER_SPACE: usize = 300;
/// The tags that name a person, a place, a product or an organisation (the part after the prefix is the name).
const NAME_TAGS: [&str; 14] = [
    "personnes",
    "personne",
    "lieux",
    "lieu",
    "produits",
    "produit",
    "organisations",
    "organisation",
    "organismes",
    "people",
    "person",
    "places",
    "products",
    "organizations",
];

/// One candidate, with how useful it is.
#[derive(Clone, Debug, PartialEq)]
struct Scored {
    word: String,
    score: f64,
    /// How many words it holds (a phrase is more specific than a word).
    parts: usize,
}

/// What one space gives, before the caller's list is assembled.
#[derive(Clone, Debug, Default)]
struct SpaceHints {
    names: Vec<Scored>,
    keywords: Vec<Scored>,
    vocabulary: Vec<Scored>,
    /// Whether the space is a health space: only then are the health terms given.
    health_space: bool,
    /// `(is a sign, slug, uses)` of the health patterns, for a health space.
    health: Vec<(bool, String, f64)>,
}

/// The four groups of an answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Groups {
    /// Names of people, places, products, organisations.
    pub names: Vec<String>,
    /// Keywords of the rules notes.
    pub keywords: Vec<String>,
    /// Terms of the vocabularies.
    pub vocabulary: Vec<String>,
    /// Signs and measures.
    pub health: Vec<String>,
}

/// The answer: the list, the text for an initial prompt, the groups it is made of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// Every word, the names first; at most the limit asked.
    pub words: Vec<String>,
    /// The words as a comma separated text of at most [`PROMPT_TOKENS`] tokens, for an initial prompt.
    pub text: String,
    /// The words by group.
    pub groups: Groups,
    /// When the oldest list the answer is made of was worked out (seconds since the epoch).
    pub built_at: i64,
    /// A number that changes when the memory the answer comes from changed.
    pub version: u64,
}

impl Hints {
    /// The JSON form: `{words, text, groups: {names, keywords, vocabulary, health}, built_at, version}`.
    pub fn to_json(&self) -> Value {
        json!({
            "words": self.words,
            "text": self.text,
            "groups": {
                "names": self.groups.names,
                "keywords": self.groups.keywords,
                "vocabulary": self.groups.vocabulary,
                "health": self.groups.health,
            },
            "built_at": util::iso(self.built_at),
            "version": self.version,
        })
    }
}

struct Entry {
    fingerprint: u64,
    built_at: i64,
    hints: Arc<SpaceHints>,
}

type Cache = Mutex<HashMap<(PathBuf, String), Entry>>;

static CACHE: LazyLock<Cache> = LazyLock::new(Default::default);
static BUILDS: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Default::default);

/// How many times a space's list was worked out for this memory since the process started (a counter for tests and for the
/// status of the node: a call that finds the cache fresh does not count).
pub fn builds(brain: &Brain) -> u64 {
    BUILDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(brain.dir())
        .copied()
        .unwrap_or(0)
}

/// The words to give a speech model for the spaces `spaces` (the ones the caller reads), at most `limit` of them (clamped
/// to 1..=[`MAX_LIMIT`]), the health terms in `lang` (`fr`, `en`; another or none: both).
pub fn hints(
    brain: &Brain,
    user_id: &str,
    spaces: &[Space],
    limit: usize,
    lang: Option<&str>,
) -> Result<Hints> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let lang = lang.and_then(|l| {
        let l = l
            .trim()
            .to_lowercase()
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_string();
        matches!(l.as_str(), "fr" | "en").then_some(l)
    });
    let layout = brain.layout(user_id).unwrap_or_default();
    let now = util::now();
    let mut parts: Vec<Arc<SpaceHints>> = Vec::new();
    let mut versions = Vec::new();
    let mut built_at = now;
    let mut sorted: Vec<&Space> = spaces.iter().collect();
    sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for space in sorted {
        let conn = brain.open_space(space)?;
        let info = SpaceInfo {
            name: space.as_str().to_string(),
            label: layout
                .iter()
                .find(|p| p.space == *space)
                .map(|p| p.label.clone())
                .unwrap_or_default(),
            kind: brain.space_kind(space).unwrap_or_default(),
        };
        let fingerprint = fingerprint(&conn, info.is_health())?;
        let key = (brain.dir().to_path_buf(), space.as_str().to_string());
        let cached = {
            let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            cache
                .get(&key)
                .filter(|e| e.fingerprint == fingerprint && now - e.built_at < REBUILD_AFTER)
                .map(|e| (e.built_at, e.hints.clone()))
        };
        let (at, one) = match cached {
            Some(c) => c,
            None => {
                let one = Arc::new(build(&conn, &info, now)?);
                *BUILDS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(brain.dir().to_path_buf())
                    .or_insert(0) += 1;
                CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    key,
                    Entry {
                        fingerprint,
                        built_at: now,
                        hints: one.clone(),
                    },
                );
                (now, one)
            }
        };
        built_at = built_at.min(at);
        versions.push((space.as_str().to_string(), fingerprint));
        parts.push(one);
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    versions.hash(&mut h);
    // A whole number a page can hold: 53 bits.
    let version = h.finish() & ((1 << 53) - 1);
    let groups = assemble(&parts, limit, lang.as_deref());
    let words: Vec<String> = [
        &groups.names,
        &groups.keywords,
        &groups.vocabulary,
        &groups.health,
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();
    Ok(Hints {
        text: prompt_text(&words),
        words,
        groups,
        built_at,
        version,
    })
}

/// A number that changes whenever the notes (any change to a note is an event of its history), the rules (a note) or the
/// vocabularies of the space changed: cheap, it reads four counters.
fn fingerprint(conn: &Connection, health: bool) -> Result<u64> {
    let row: (i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT (SELECT coalesce(max(id), 0) FROM fact_events),
                (SELECT coalesce(max(rowid), 0) FROM facts),
                (SELECT count(*) FROM vocabularies),
                (SELECT coalesce(sum(terms), 0) FROM vocabularies),
                (SELECT coalesce(max(imported_at), 0) FROM vocabularies)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (row, health).hash(&mut h);
    Ok(h.finish())
}

/// The weight of a note: the recent ones count more.
fn weight(now: i64, created_at: i64) -> f64 {
    let days = (now - created_at) / 86_400;
    match days {
        d if d <= 30 => 2.0,
        d if d <= 180 => 1.0,
        _ => 0.5,
    }
}

fn build(conn: &Connection, info: &SpaceInfo, now: i64) -> Result<SpaceHints> {
    // The notes of the person: not the plumbing of the memory (rules, index, living cards, the janitor's own notes) and not the
    // raw recordings.
    let mut st = conn.prepare(
        "SELECT f.text, f.created_at FROM facts f
         WHERE f.status = 'active' AND (f.\"type\" IS NULL OR f.\"type\" <> 'file')
           AND NOT EXISTS (SELECT 1 FROM fact_tags t WHERE t.fact_id = f.id AND (
                t.tag IN ('rules', 'index', 'guide', 'dashboard')
                OR t.tag LIKE 'janitor/%' OR t.tag LIKE 'living%' OR t.tag LIKE 'proposals/%'
                OR t.tag LIKE 'inbox/audio%'))
         ORDER BY f.created_at DESC LIMIT ?1",
    )?;
    let texts: Vec<(String, f64)> = st
        .query_map([SCAN as i64], |r| {
            Ok((r.get::<_, String>(0)?, weight(now, r.get(1)?)))
        })?
        .collect::<rusqlite::Result<_>>()?;

    // Names: what the text says, and the tags.
    let mut names = names_in(&texts);
    for (tag, count) in tag_counts(conn, &NAME_TAGS)? {
        let Some(word) = name_of_tag(&tag) else {
            continue;
        };
        let score = count as f64 * 2.0;
        let key = crate::text::fold(&word);
        match names.iter_mut().find(|n| crate::text::fold(&n.word) == key) {
            Some(n) => n.score += score,
            None => names.push(Scored {
                parts: word.split(' ').count(),
                word,
                score,
            }),
        }
    }
    order(&mut names, false);
    names.truncate(PER_SPACE);

    // Keywords and vocabulary: how often the notes say them.
    let (_, rules) = crate::rules::read(conn);
    let terms = crate::vocabulary::terms(conn, crate::vocabulary::MAX_TERMS)?;
    let key_of = |t: &str| autotag::words(t).join(" ");
    let mut counts: HashMap<String, f64> = HashMap::new();
    for t in rules.keywords.iter().chain(terms.iter()) {
        counts.insert(key_of(t), 0.0);
    }
    if !counts.is_empty() {
        for (text, w) in &texts {
            let words = autotag::words(text);
            for n in 1..=4 {
                for win in words.windows(n) {
                    if let Some(c) = counts.get_mut(&win.join(" ")) {
                        *c += w;
                    }
                }
            }
        }
    }
    let scored = |list: &[String]| -> Vec<Scored> {
        list.iter()
            .map(|t| Scored {
                word: t.clone(),
                score: counts.get(&key_of(t)).copied().unwrap_or(0.0),
                parts: t.split_whitespace().count(),
            })
            .collect()
    };
    let mut keywords = scored(&rules.keywords);
    order(&mut keywords, false);
    keywords.truncate(PER_SPACE);
    let terms: Vec<String> = terms.into_iter().filter(|t| usable(t)).collect();
    let mut vocabulary = scored(&terms);
    order(&mut vocabulary, true);
    vocabulary.truncate(PER_SPACE);

    let health = if info.is_health() {
        let mut tags = tag_counts(conn, &["signe", "mesure"])?;
        tags.sort();
        tags.into_iter()
            .filter_map(|(tag, n)| {
                let (kind, slug) = tag.split_once('/')?;
                Some((kind == "signe", slug.to_string(), n as f64))
            })
            .collect()
    } else {
        vec![]
    };
    Ok(SpaceHints {
        names,
        keywords,
        vocabulary,
        health_space: info.is_health(),
        health,
    })
}

/// The tags of the active notes that start with one of `prefixes` + `/`, each with the number of notes.
fn tag_counts(conn: &Connection, prefixes: &[&str]) -> Result<Vec<(String, i64)>> {
    let clause = prefixes
        .iter()
        .map(|p| format!("t.tag LIKE '{p}/%'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut st = conn.prepare(&format!(
        "SELECT t.tag, count(*) FROM fact_tags t JOIN facts f ON f.id = t.fact_id
         WHERE f.status = 'active' AND ({clause}) GROUP BY t.tag"
    ))?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `personnes/camille-roux` is « Camille Roux ».
fn name_of_tag(tag: &str) -> Option<String> {
    let last = tag.rsplit('/').next()?;
    let word = last
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    usable(&word).then_some(word)
}

/// Whether a word can be an item: 2 to [`ITEM_MAX`] characters, no comma or line break (the text is comma separated).
fn usable(w: &str) -> bool {
    let n = w.trim().chars().count();
    (2..=ITEM_MAX).contains(&n)
        && !w.contains([',', ';', '\n', '\r', '\t'])
        && w.chars().any(char::is_alphabetic)
}

/// Most useful first. Ties: the more specific first (more words, then longer) or, for a vocabulary, the shortest first.
fn order(list: &mut [Scored], shortest_first: bool) {
    list.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                if shortest_first {
                    a.word.chars().count().cmp(&b.word.chars().count())
                } else {
                    b.parts
                        .cmp(&a.parts)
                        .then_with(|| b.word.chars().count().cmp(&a.word.chars().count()))
                }
            })
            .then_with(|| a.word.cmp(&b.word))
    });
}

// ---- names ---------------------------------------------------------------------------------------------------------------

/// Words that are capitalised without being names: days, months (French and English, with their short forms), articles,
/// pronouns, greetings, the words a note starts with. Folded: lower case, no accents.
const NOT_NAMES: &[&str] = &[
    // days and months
    "lundi",
    "mardi",
    "mercredi",
    "jeudi",
    "vendredi",
    "samedi",
    "dimanche",
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "lun",
    "mar",
    "mer",
    "jeu",
    "ven",
    "sam",
    "dim",
    "janv",
    "fevr",
    "avr",
    "juil",
    "sept",
    "oct",
    "nov",
    "dec",
    "jan",
    "feb",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "mon",
    "tue",
    "wed",
    "thu",
    "fri",
    "sat",
    "sun",
    // titles
    "mme",
    "mlle",
    "mr",
    "dr",
    "pr",
    "madame",
    "monsieur",
    "docteur",
    "voir",
    // French function words and note openings
    "les",
    "des",
    "une",
    "elle",
    "nous",
    "vous",
    "ils",
    "elles",
    "ces",
    "cet",
    "cette",
    "mes",
    "tes",
    "ses",
    "notre",
    "nos",
    "votre",
    "vos",
    "leur",
    "leurs",
    "mais",
    "donc",
    "car",
    "que",
    "qui",
    "quoi",
    "dont",
    "quand",
    "comme",
    "pour",
    "par",
    "avec",
    "sans",
    "sur",
    "sous",
    "dans",
    "chez",
    "vers",
    "entre",
    "apres",
    "avant",
    "pendant",
    "depuis",
    "bonjour",
    "bonsoir",
    "salut",
    "merci",
    "oui",
    "non",
    "voici",
    "voila",
    "alors",
    "aussi",
    "puis",
    "ensuite",
    "enfin",
    "hier",
    "demain",
    "tout",
    "tous",
    "toute",
    "toutes",
    "rien",
    "cela",
    "ceci",
    "rendez",
    "rappel",
    "appeler",
    "penser",
    "faire",
    "prevoir",
    "demander",
    "noter",
    "verifier",
    "envoyer",
    "acheter",
    "mettre",
    "prendre",
    "aller",
    "venir",
    "ajouter",
    "note",
    "idee",
    "todo",
    "rdv",
    "tva",
    "ok",
    // English function words and note openings
    "the",
    "and",
    "but",
    "when",
    "what",
    "who",
    "how",
    "this",
    "that",
    "these",
    "those",
    "you",
    "she",
    "they",
    "my",
    "your",
    "his",
    "her",
    "our",
    "their",
    "yes",
    "hello",
    "thanks",
    "please",
    "today",
    "yesterday",
    "tomorrow",
    "then",
    "also",
    "just",
    "not",
    "there",
    "here",
    "with",
    "without",
    "for",
    "from",
    "into",
    "about",
    "after",
    "before",
    "during",
    "idea",
    "remember",
    "call",
    "buy",
];

fn not_a_name(folded: &str) -> bool {
    NOT_NAMES.contains(&folded)
}

/// One word of a text, with what stands before it.
struct Word<'a> {
    text: &'a str,
    /// The characters between the previous word and this one.
    before: &'a str,
}

/// The words of a text: letters, with a hyphen between two letters. An apostrophe ends a word (« l'Elysée » is « l » and
/// « Elysée »).
fn split_words(text: &str) -> Vec<Word<'_>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let (mut i, mut prev_end) = (0, 0);
    while i < chars.len() {
        if !chars[i].1.is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len()
            && (chars[i].1.is_alphabetic()
                || (chars[i].1 == '-'
                    && i > start
                    && chars.get(i + 1).is_some_and(|c| c.1.is_alphabetic())))
        {
            i += 1;
        }
        let (b, e) = (chars[start].0, chars.get(i).map_or(text.len(), |c| c.0));
        out.push(Word {
            text: &text[b..e],
            before: &text[prev_end..b],
        });
        prev_end = e;
    }
    out
}

/// Whether what stands before a word says it starts a sentence (or a line, or a list item).
fn starts_sentence(before: &str, first: bool) -> bool {
    first
        || before.chars().any(|c| {
            matches!(
                c,
                '.' | '!' | '?' | '…' | ':' | '\n' | '•' | '*' | '#' | '>' | '-' | '–' | '—'
            )
        })
}

/// A word written with a capital that is not only a capital letter of a short word.
fn capitalised(w: &str) -> bool {
    let n = w.chars().count();
    let Some(first) = w.chars().next() else {
        return false;
    };
    if !first.is_uppercase() || n < 3 {
        return false;
    }
    let has_lower = w.chars().any(char::is_lowercase);
    // An acronym (EDF): all capitals, up to six letters. Longer is shouting.
    has_lower || n <= 6
}

#[derive(Default)]
struct Seen {
    display: String,
    /// Said in the middle of a sentence (or as part of a phrase): a capital that means something.
    mid: u32,
    /// Said at the start of a sentence: a capital that may only be grammar.
    start: u32,
    score: f64,
    parts: usize,
}

/// The names that recur in `texts` (each with its weight), most useful first.
///
/// A run of up to four capitalised words (« Paul Martin ») is a candidate; so is a single capitalised word. A word that
/// only starts sentences is no name; nor is a day, a month, a title, an article or a word the texts also write in lower
/// case more often than they write it with a capital in the middle of a sentence. A candidate counts from its second
/// occurrence.
fn names_in(texts: &[(String, f64)]) -> Vec<Scored> {
    let mut seen: HashMap<String, Seen> = HashMap::new();
    // What the texts write in lower case: a word they also write that way is a word, not a name.
    let mut lower: HashMap<String, u32> = HashMap::new();
    let all: Vec<Vec<Word<'_>>> = texts.iter().map(|(t, _)| split_words(t)).collect();
    for words in &all {
        for w in words.iter().filter(|w| !capitalised(w.text)) {
            if w.text.chars().count() >= 3 {
                *lower.entry(crate::text::fold(w.text)).or_insert(0) += 1;
            }
        }
    }
    for (words, (_, weight)) in all.iter().zip(texts) {
        let mut i = 0;
        while i < words.len() {
            let w = &words[i];
            if !capitalised(w.text) {
                i += 1;
                continue;
            }
            // The run of capitalised words that follow, each after one space.
            let mut run = vec![i];
            while let Some(next) = words.get(run[run.len() - 1] + 1) {
                if next.before == " " && capitalised(next.text) {
                    run.push(run[run.len() - 1] + 1);
                } else {
                    break;
                }
            }
            i = run[run.len() - 1] + 1;
            let mut start = starts_sentence(w.before, run[0] == 0);
            // A verb or a noun that starts the sentence, then the name (« Appeler Marie Dupont »): the first word goes when the
            // texts also write it in lower case.
            if start && run.len() > 1 && lower.contains_key(&crate::text::fold(words[run[0]].text))
            {
                run.remove(0);
                start = false;
            }
            // Leave out the words that are not names at both ends of the run.
            while run
                .first()
                .is_some_and(|k| not_a_name(&crate::text::fold(words[*k].text)))
            {
                run.remove(0);
                start = false;
            }
            while run
                .last()
                .is_some_and(|k| not_a_name(&crate::text::fold(words[*k].text)))
            {
                run.pop();
            }
            if run.is_empty() || run.len() > 4 {
                continue;
            }
            let display = run
                .iter()
                .map(|k| words[*k].text)
                .collect::<Vec<_>>()
                .join(" ");
            let e = seen.entry(crate::text::fold(&display)).or_default();
            if e.display.is_empty() {
                e.display = display;
                e.parts = run.len();
            }
            if run.len() == 1 && start {
                e.start += 1;
            } else {
                e.mid += 1;
            }
            e.score += weight * if run.len() > 1 { 1.5 } else { 1.0 };
        }
    }
    let mut out: Vec<Scored> = seen
        .into_iter()
        .filter(|(key, e)| {
            let said_in_lower = lower.get(key).copied().unwrap_or(0);
            e.mid >= 1 && e.mid + e.start >= 2 && said_in_lower <= e.mid && usable(&e.display)
        })
        .map(|(_, e)| Scored {
            word: e.display,
            score: e.score,
            parts: e.parts,
        })
        .collect();
    order(&mut out, false);
    out.truncate(PER_SPACE);
    out
}

// ---- health ---------------------------------------------------------------------------------------------------------------

/// The labels of a sign or a measure in the language asked (both when none).
fn labels(slug: &str, lang: Option<&str>) -> Vec<String> {
    let Some((fr, en)) = autotag::health_labels(slug) else {
        return vec![slug.replace('-', " ")];
    };
    match lang {
        Some("fr") => vec![fr],
        Some("en") => vec![en],
        _ if fr == en => vec![fr],
        _ => vec![fr, en],
    }
}

// ---- the answer -----------------------------------------------------------------------------------------------------------

/// Adds the lists of the spaces: each space's scores are divided by its best, so that a large space does not bury a small
/// one, then summed for the words that several spaces have.
fn merge(lists: &[&Vec<Scored>], shortest_first: bool) -> Vec<Scored> {
    let mut all: HashMap<String, Scored> = HashMap::new();
    for list in lists {
        let best = list.iter().map(|s| s.score).fold(0.0, f64::max);
        for s in list.iter() {
            let share = if best > 0.0 { s.score / best } else { 0.0 };
            all.entry(crate::text::fold(&s.word))
                .and_modify(|e| e.score += share)
                .or_insert_with(|| Scored {
                    score: share,
                    ..s.clone()
                });
        }
    }
    let mut out: Vec<Scored> = all.into_values().collect();
    order(&mut out, shortest_first);
    out
}

/// The groups for `parts`, at most `limit` words in all: a share for each group (names 45 %, vocabulary 25 %, keywords and
/// health 15 % each); what a group does not use goes to the others, names first. A word is in the first group that has it.
fn assemble(parts: &[Arc<SpaceHints>], limit: usize, lang: Option<&str>) -> Groups {
    let names = merge(&parts.iter().map(|p| &p.names).collect::<Vec<_>>(), false);
    let keywords = merge(
        &parts.iter().map(|p| &p.keywords).collect::<Vec<_>>(),
        false,
    );
    let vocabulary = merge(
        &parts.iter().map(|p| &p.vocabulary).collect::<Vec<_>>(),
        true,
    );
    // Health: signs and measures by use, then in the order of the table.
    let table: Vec<&str> = {
        let (signs, measures) = autotag::health_slugs();
        signs.into_iter().chain(measures).collect()
    };
    let mut used: HashMap<(bool, String), f64> = HashMap::new();
    for (sign, slug, n) in parts.iter().flat_map(|p| p.health.iter()) {
        *used.entry((*sign, slug.clone())).or_insert(0.0) += n;
    }
    let any_health = parts.iter().any(|p| p.health_space);
    let health: Vec<String> = if any_health {
        let mut slugs: Vec<(usize, f64, &str)> = table
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let n = used
                    .iter()
                    .filter(|((_, slug), _)| slug == s)
                    .map(|(_, n)| *n)
                    .sum();
                (i, n, *s)
            })
            .collect();
        slugs.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        slugs
            .into_iter()
            .flat_map(|(_, _, s)| labels(s, lang))
            .collect()
    } else {
        vec![]
    };

    // The first group that has a word keeps it.
    let mut seen: HashSet<String> = HashSet::new();
    let mut clean = |items: Vec<String>| -> Vec<String> {
        items
            .into_iter()
            .filter(|w| usable(w) && seen.insert(crate::text::fold(w)))
            .collect()
    };
    let mut lists = [
        clean(names.into_iter().map(|s| s.word).collect()),
        clean(keywords.into_iter().map(|s| s.word).collect()),
        clean(vocabulary.into_iter().map(|s| s.word).collect()),
        clean(health),
    ];
    let share = [45, 15, 25, 15];
    let mut take: Vec<usize> = lists
        .iter()
        .zip(share)
        .map(|(l, p)| l.len().min(limit * p / 100))
        .collect();
    // What is left of the limit goes to the groups that have more, names first, then vocabulary, keywords, health.
    let mut left = limit.saturating_sub(take.iter().sum());
    for g in [0, 2, 1, 3] {
        let more = (lists[g].len() - take[g]).min(left);
        take[g] += more;
        left -= more;
    }
    for (l, n) in lists.iter_mut().zip(&take) {
        l.truncate(*n);
    }
    let [names, keywords, vocabulary, health] = lists;
    Groups {
        names,
        keywords,
        vocabulary,
        health,
    }
}

/// An estimate of the tokens of a word in a prompt: about one for three characters, and the separator.
fn tokens(w: &str) -> usize {
    w.chars().count().div_ceil(3) + 1
}

/// The first words that fit in [`PROMPT_TOKENS`], separated by commas.
fn prompt_text(words: &[String]) -> String {
    let mut used = 0;
    let mut out: Vec<&str> = Vec::new();
    for w in words {
        used += tokens(w);
        if used > PROMPT_TOKENS {
            break;
        }
        out.push(w);
    }
    out.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(texts: &[&str]) -> Vec<String> {
        let t: Vec<(String, f64)> = texts.iter().map(|t| (t.to_string(), 1.0)).collect();
        names_in(&t).into_iter().map(|s| s.word).collect()
    }

    #[test]
    fn a_name_is_said_twice_and_not_only_at_the_start_of_a_sentence() {
        let n = names(&[
            "Demain Paul viendra. Pendant ce temps Paul joue.",
            "Pendant la nuit il a plu. Pendant le jour aussi.",
            "Bonjour Marie. Marie a dit bonjour. Bonjour à tous.",
        ]);
        assert!(n.contains(&"Paul".to_string()), "{n:?}");
        assert!(!n.contains(&"Pendant".to_string()), "{n:?}");
        assert!(!n.contains(&"Demain".to_string()), "{n:?}");
        assert!(!n.contains(&"Bonjour".to_string()), "{n:?}");
        // Said once in the middle and once at the start: enough.
        assert!(n.contains(&"Marie".to_string()), "{n:?}");
    }

    #[test]
    fn days_months_and_common_words_in_capitals_are_not_names() {
        let n = names(&[
            "Rendez-vous Lundi prochain, puis Mardi, puis Janvier.",
            "On se voit Lundi. Mardi aussi, en Janvier ou en Mars. Monday and March.",
            "Le Pain est bon. Un bon pain. Du pain frais, du pain chaud. Le Pain encore.",
        ]);
        for w in [
            "Lundi", "Mardi", "Janvier", "Mars", "Monday", "March", "Pain",
        ] {
            assert!(!n.contains(&w.to_string()), "{w} in {n:?}");
        }
    }

    #[test]
    fn a_run_of_capitalised_words_is_one_name_and_a_title_is_left_out() {
        let n = names(&[
            "Voir Mme Dupont Martin demain.",
            "Hier Mme Dupont Martin a rappelé. Et Saint-Malo, Saint-Malo!",
        ]);
        assert!(n.contains(&"Dupont Martin".to_string()), "{n:?}");
        assert!(n.contains(&"Saint-Malo".to_string()), "{n:?}");
        assert!(!n.iter().any(|w| w.starts_with("Mme")), "{n:?}");
    }

    #[test]
    fn a_name_said_once_is_not_enough_and_a_heading_is_not_a_name() {
        let n = names(&[
            "On a vu Gustave hier.",
            "Réunion Equipe Projet Alpha Beta Gamma ici.",
            "Réunion Equipe Projet Alpha Beta Gamma là.",
        ]);
        assert!(n.is_empty(), "{n:?}");
    }

    #[test]
    fn every_sign_and_measure_of_the_referential_has_its_labels() {
        let (signs, measures) = autotag::health_slugs();
        for s in signs.into_iter().chain(measures) {
            assert!(autotag::health_labels(s).is_some(), "no label for {s}");
        }
    }

    #[test]
    fn the_prompt_is_cut_at_the_token_budget() {
        let words: Vec<String> = (0..200).map(|i| format!("mot-numero-{i}")).collect();
        let t = prompt_text(&words);
        assert!(t.split(", ").map(tokens).sum::<usize>() <= PROMPT_TOKENS);
        assert!(t.split(", ").count() < 200 && !t.is_empty());
        assert_eq!(prompt_text(&[]), "");
    }
}
