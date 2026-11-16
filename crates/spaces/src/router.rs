//! The keyword router, on the device: same note, same words, same result.

use crate::normalize::{contains, hashtags, normalize, said};
use crate::rules::RuleKind;
use crate::space::Tree;

/// Where a note goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routing {
    /// Decided on the device: these spaces (ids, in the tree's order).
    Spaces(Vec<String>),
    /// Only ordinary words of spaces that are not sensitive: suggested; the
    /// Intelligence or the person decides.
    Suggested(Vec<String>),
    /// Nothing found: the Intelligence, else the default space.
    Undecided,
    /// A distress note: it stays on the device, whatever its words, and the
    /// app shows the help reminder. // law: S1
    KeptLocal,
}

/// When a sensitive space is reached, only it and the spaces it may share
/// with keep the note; else `ids` as they are.
fn keep_sensitive(tree: &Tree, hit: &[String], ids: Vec<String>) -> Vec<String> {
    let sensitive: Vec<&str> = hit
        .iter()
        .filter(|id| tree.get(id).is_some_and(|n| n.sensitive))
        .map(String::as_str)
        .collect();
    if sensitive.is_empty() {
        return ids;
    }
    ids.into_iter()
        .filter(|id| {
            sensitive.contains(&id.as_str())
                || sensitive
                    .iter()
                    .any(|s| tree.get(s).is_some_and(|n| n.shares_with.contains(id)))
        })
        .collect()
}

/// `ids` and the ancestors that take their sub-spaces' notes, in the tree's order.
fn with_parents(tree: &Tree, ids: &[String]) -> Vec<String> {
    let mut all: Vec<String> = ids.to_vec();
    for id in ids {
        if let Some(n) = tree.get(id) {
            all.extend(
                tree.ancestors(&n.path)
                    .into_iter()
                    .filter(|p| p.includes_children)
                    .map(|p| p.id.clone()),
            );
        }
    }
    tree.spaces()
        .iter()
        .filter(|n| all.contains(&n.id))
        .map(|n| n.id.clone())
        .collect()
}

/// Routes `text`. `distress` comes from the caller's safety check, done
/// before the router and before any model.
///
/// 1. Any word of a sensitive space, unique or ordinary, decides: the note
///    goes there, and to the other spaces reached only when the sensitive
///    space may share with them.
/// 2. Else every space whose unique word is said gets the note.
/// 3. Else ordinary words only suggest.
pub fn route(tree: &Tree, text: &str, distress: bool) -> Routing {
    if distress {
        return Routing::KeptLocal;
    }
    let words = normalize(text);
    let tags = hashtags(text);
    let matching = |kind: RuleKind| -> Vec<String> {
        tree.spaces()
            .iter()
            .filter(|n| {
                tree.words(&n.id, kind)
                    .iter()
                    .any(|w| said(&words, &tags, w))
            })
            .map(|n| n.id.clone())
            .collect()
    };
    let unique = matching(RuleKind::UniqueWord);
    let ordinary = matching(RuleKind::OrdinaryWord);
    // Any word of a sensitive space, even an ordinary one, reaches it; then
    // the note goes only there and to the spaces it may share with (empty by
    // default: never health in a work folder unless the person said so).
    let sensitive: Vec<String> = unique
        .iter()
        .chain(&ordinary)
        .filter(|id| tree.get(id).is_some_and(|n| n.sensitive))
        .cloned()
        .collect();
    if !sensitive.is_empty() {
        let mut reached = sensitive.clone();
        reached.extend(unique.iter().filter(|id| !sensitive.contains(id)).cloned());
        let hit = reached.clone();
        return Routing::Spaces(keep_sensitive(tree, &hit, with_parents(tree, &reached)));
    }
    // Every space whose unique word is said gets the note, a space and its
    // sub-space alike.
    if !unique.is_empty() {
        return Routing::Spaces(with_parents(tree, &unique));
    }
    if ordinary.is_empty() {
        Routing::Undecided
    } else {
        Routing::Suggested(with_parents(tree, &ordinary))
    }
}

/// Among `spaces`, those reached only by a word that another of them has
/// too: shown as « added by a shared word ». Two spaces may share words on
/// purpose; it is never an error.
pub fn shared_word_spaces(tree: &Tree, text: &str, spaces: &[String]) -> Vec<String> {
    let words = normalize(text);
    let said = |id: &str| -> Vec<String> {
        tree.words(id, RuleKind::UniqueWord)
            .into_iter()
            .filter(|w| contains(&words, w))
            .map(|w| normalize(&w).join(" "))
            .collect()
    };
    spaces
        .iter()
        .filter(|id| {
            let mine = said(id);
            !mine.is_empty()
                && mine.iter().all(|w| {
                    spaces
                        .iter()
                        .any(|other| other != *id && said(other).contains(w))
                })
        })
        .cloned()
        .collect()
}

/// The other spaces that have this word too (« aussi dans Pro, Dev »).
pub fn word_also_in(tree: &Tree, space: &str, word: &str) -> Vec<String> {
    let wanted = normalize(word);
    tree.spaces()
        .iter()
        .filter(|s| s.id != space)
        .filter(|s| {
            [RuleKind::UniqueWord, RuleKind::OrdinaryWord]
                .iter()
                .any(|k| tree.words(&s.id, *k).iter().any(|w| normalize(w) == wanted))
        })
        .map(|s| s.id.clone())
        .collect()
}
