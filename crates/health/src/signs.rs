//! Signs and symptoms: the ones a text evokes by a label or an everyday keyword (whole words, not right after a negation),
//! and the level said next to them (`douleur 2/10`, `forte douleur`).

use std::cmp::Reverse;

use serde::Serialize;

use crate::model::{Lang, Manifestation, Pat, Referential};
use crate::text::{Tok, is_word, tokenize};

/// A sign found in a text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sign {
    pub id: String,
    pub label: String,
    /// The level as said: `2/10` (0 to 10, only on a named scale), or the word (`forte`, `mild`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The scale the sign is noted on; left out for a sign noted by its presence only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<String>,
    /// The name of the sign's tag in the tagger (`cephalee` for `headache`).
    #[serde(skip)]
    pub tag: String,
    /// The words that brought it up, folded (`mal au crane`); empty for a sign that comes with a more specific one.
    #[serde(skip)]
    pub words: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Found {
    pub m: usize,
    /// Token range of the first match.
    pub start: usize,
    pub end: usize,
    pub table: usize,
    pub words: String,
}

/// The last word of a pattern also matches its plural (`nausee`, `nausees`), when long enough not to be another word.
fn same_word(text: &str, pat: &str, last: bool) -> bool {
    text == pat
        || (last
            && pat.chars().count() >= 5
            && text.len() == pat.len() + 1
            && text.starts_with(pat)
            && (text.ends_with('s') || text.ends_with('x')))
}

impl Referential {
    pub(crate) fn negators_for(&self, lang: Lang) -> Vec<&Vec<String>> {
        lang.tables()
            .iter()
            .flat_map(|t| self.negators[*t].iter())
            .collect()
    }

    /// The signs of a tokenized text, one per sign, in order of first appearance.
    pub(crate) fn find(&self, toks: &[Tok], lang: Lang) -> Vec<Found> {
        let ws: Vec<(&str, usize)> = toks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match t {
                Tok::Word(w) => Some((w.as_str(), i)),
                _ => None,
            })
            .collect();
        let negators = self.negators_for(lang);
        // Every match as (start, end, sign) in words.
        let mut spans: Vec<(usize, usize, usize, usize)> = Vec::new();
        for (p, (w, _)) in ws.iter().enumerate() {
            let stripped = w
                .strip_suffix('s')
                .or_else(|| w.strip_suffix('x'))
                .filter(|s| s.chars().count() >= 5);
            for table in lang.tables() {
                let index = &self.index[*table];
                let lists = std::iter::once(index.get(*w)).chain(stripped.map(|s| index.get(s)));
                for pat in lists.flatten().flatten() {
                    let n = pat.words.len();
                    if p + n > ws.len() || !matches_at(&ws, p, pat) {
                        continue;
                    }
                    let before: Vec<&str> = ws[..p].iter().map(|(w, _)| *w).collect();
                    if negators.iter().any(|neg| ends_with(&before, neg)) {
                        continue;
                    }
                    spans.push((p, p + n, pat.m, pat.table));
                }
            }
        }
        // « mal au crâne » is a headache, not also « douleur » by a shorter word inside it: a match inside a longer one
        // of another sign is dropped.
        let kept: Vec<&(usize, usize, usize, usize)> = spans
            .iter()
            .filter(|(s, e, i, _)| {
                !spans
                    .iter()
                    .any(|(s2, e2, i2, _)| i2 != i && s2 <= s && e <= e2 && (e2 - s2) > (e - s))
            })
            .collect();
        // The first match of each sign, the longest at that place.
        let mut first: Vec<(usize, usize, usize, usize)> = Vec::new();
        for &&(start, end, m, table) in &kept {
            match first.iter_mut().find(|f| f.2 == m) {
                Some(f) if (start, Reverse(end)) < (f.0, Reverse(f.1)) => {
                    *f = (start, end, m, table)
                }
                Some(_) => {}
                None => first.push((start, end, m, table)),
            }
        }
        first.sort();
        first
            .into_iter()
            .map(|(s, e, m, table)| Found {
                m,
                start: ws[s].1,
                end: ws[e - 1].1 + 1,
                table,
                words: ws[s..e]
                    .iter()
                    .map(|(w, _)| *w)
                    .collect::<Vec<_>>()
                    .join(" "),
            })
            .collect()
    }

    /// The signs of a text with their levels; each sign once, a more general one (`also`) after the one that brings it.
    pub fn signs(&self, text: &str, lang: Lang) -> Vec<Sign> {
        self.signs_of(&tokenize(text), lang)
    }

    pub(crate) fn signs_of(&self, toks: &[Tok], lang: Lang) -> Vec<Sign> {
        let found = self.find(toks, lang);
        let levels = self.levels(toks, &found);
        let mut out: Vec<Sign> = Vec::new();
        for (f, level) in found.iter().zip(levels) {
            let m = &self.manifestations[f.m];
            // With every language at once, the label is in the language of the words that brought the sign up.
            let label_lang = if lang == Lang::Auto && f.table == 1 {
                Lang::En
            } else {
                lang
            };
            push_sign(&mut out, m, label_lang, level, &f.words);
            for a in &m.also {
                if let Some(g) = self.get(a) {
                    push_sign(&mut out, g, label_lang, None, "");
                }
            }
        }
        out
    }

    /// The ids of the signs a text evokes, each with the words that brought it up. A suggestion only: nothing is ticked by
    /// itself.
    pub fn suggest_with_words(&self, text: &str, lang: Lang) -> Vec<(String, String)> {
        self.signs(text, lang)
            .into_iter()
            .map(|s| (s.id, s.words))
            .collect()
    }

    /// The ids of the signs a text evokes.
    pub fn suggest(&self, text: &str, lang: Lang) -> Vec<String> {
        self.signs(text, lang).into_iter().map(|s| s.id).collect()
    }

    /// The level of each found sign.
    fn levels(&self, toks: &[Tok], found: &[Found]) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = vec![None; found.len()];
        let eligible = |f: &Found| {
            let s = self.manifestations[f.m].scale.as_str();
            s != "mood" && s != "presence"
        };
        // The numbers said on ten, clause by clause: a clause with as many signs as levels pairs them in order; a level
        // alone in its clause goes to the one sign of the clause before (`anxieux, 2 sur 10`); anything else is
        // ambiguous (`triste et anxieux à 4/10`: whose?) and is left out.
        let mut waiting: Option<usize> = None;
        let mut a = 0;
        while a <= toks.len() {
            let b = (a..toks.len())
                .find(|&k| matches!(toks[k], Tok::Sym('.' | ';' | '!' | '?' | '\n' | ',')))
                .unwrap_or(toks.len());
            let signs: Vec<usize> = (0..found.len())
                .filter(|&k| found[k].start >= a && found[k].start < b && eligible(&found[k]))
                .collect();
            let said = levels_on_ten(&toks[a..b]);
            match (signs.len(), said.len()) {
                (k, m) if k > 0 && k == m => {
                    for (s, (_, n)) in signs.iter().zip(&said) {
                        out[*s] = Some(format!("{n}/10"));
                    }
                    waiting = None;
                }
                (0, 1) => {
                    if let Some(w) = waiting.take() {
                        out[w] = Some(format!("{}/10", said[0].1));
                    }
                }
                (1, 0) => waiting = Some(signs[0]),
                (k, _) if k > 1 => {
                    // Several signs, fewer levels: a level that a later sign of the phrase closes belongs to the sign
                    // just before it (`douleur 4 sur 10 et fatigue`); one that ends the phrase could be any of them.
                    for (at, n) in &said {
                        let at = a + at;
                        let before = signs.iter().rfind(|s| found[**s].start < at);
                        let closed = signs.iter().any(|s| found[*s].start > at);
                        if let (Some(s), true) = (before, closed) {
                            out[*s].get_or_insert_with(|| format!("{n}/10"));
                        }
                    }
                    waiting = None;
                }
                _ => waiting = None,
            }
            a = b + 1;
        }
        // The word as said, for a sign with no number: `forte douleur`, `mild headache`.
        for (k, f) in found.iter().enumerate() {
            if out[k].is_none() {
                out[k] = self.quality(toks, f.start, f.end);
            }
        }
        out
    }

    fn quality(&self, toks: &[Tok], start: usize, end: usize) -> Option<String> {
        let is_quality = |t: Option<&Tok>| match t {
            Some(Tok::Word(w)) if self.level_words.contains(w) => Some(w.clone()),
            _ => None,
        };
        for k in end..end + 3 {
            if let Some(q) = is_quality(toks.get(k)) {
                return Some(q);
            }
            if matches!(
                toks.get(k),
                Some(Tok::Num { .. } | Tok::Date | Tok::Time(_))
            ) {
                break;
            }
        }
        if start > 0 {
            return is_quality(toks.get(start - 1));
        }
        None
    }

    pub fn get(&self, id: &str) -> Option<&Manifestation> {
        self.manifestations.iter().find(|m| m.id == id)
    }
}

fn push_sign(out: &mut Vec<Sign>, m: &Manifestation, lang: Lang, level: Option<String>, w: &str) {
    if out.iter().any(|s| s.id == m.id) {
        return;
    }
    out.push(Sign {
        id: m.id.clone(),
        label: m.label(lang).to_string(),
        level,
        scale: (m.scale != "presence").then(|| m.scale.clone()),
        tag: m.tag.clone(),
        words: w.to_string(),
    });
}

fn matches_at(ws: &[(&str, usize)], p: usize, pat: &Pat) -> bool {
    let n = pat.words.len();
    pat.words
        .iter()
        .enumerate()
        .all(|(k, w)| same_word(ws[p + k].0, w, k + 1 == n))
}

fn ends_with(before: &[&str], neg: &[String]) -> bool {
    !neg.is_empty()
        && before.len() >= neg.len()
        && before[before.len() - neg.len()..]
            .iter()
            .zip(neg)
            .all(|(a, b)| a == b)
}

/// The numbers given on ten in a run of tokens, with where they are: `2/10`, `2 sur 10`, `2 / 10`, `2 out of 10`; a number over ten is no
/// level.
fn levels_on_ten(toks: &[Tok]) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        let Tok::Num {
            text,
            pair,
            int: true,
        } = t
        else {
            continue;
        };
        let n: Option<u32> = match pair.as_deref() {
            Some("10") => text.parse().ok(),
            Some(_) => None,
            None => {
                let mut j = k + 1;
                if is_word(toks.get(j), "sur") || matches!(toks.get(j), Some(Tok::Sym('/'))) {
                    j += 1;
                } else if is_word(toks.get(j), "out") && is_word(toks.get(j + 1), "of") {
                    j += 2;
                } else {
                    j = 0;
                }
                (j > 0
                    && matches!(toks.get(j), Some(Tok::Num { text, pair: None, .. }) if text == "10"))
                .then(|| text.parse().ok())
                .flatten()
            }
        };
        if let Some(n) = n.filter(|n| *n <= 10) {
            out.push((k, n));
        }
    }
    out
}
