//! Red flags: signs that are worth an urgent word with a professional, matched on folded text, clause by clause. Only a
//! real negation denies a sign (a negator right before it, one indefinite article allowed between; « plus (de) » after a
//! « ne » a few words before); doubt fires. A phrase that is harmless as a whole (`except`) silences a match only when it
//! covers it entirely. Rules that send to the 15 come before the 3114 rule, so the first hit gives the first number.

use serde::{Deserialize, Serialize};

use crate::model::Error;
use crate::text::words;

/// One question of the sheet that characterizes a red flag; data collection only, shown after the help screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub text: String,
}

/// One red-flag rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    /// Each pattern fires on its own.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Groups that fire together: every group must match somewhere in the text.
    #[serde(default)]
    pub together: Vec<Vec<String>>,
    /// Longer phrases that show a harmless sense (`*` is any one word).
    #[serde(default)]
    pub except: Vec<String>,
    pub message: String,
    /// The number the help screen puts first.
    pub first: String,
    #[serde(default)]
    pub characterization: Vec<Question>,
    pub sources: Vec<String>,
}

/// A public source a rule cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    #[serde(default)]
    pub title: String,
    pub level: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Deserialize)]
struct Raw {
    schema: String,
    version: u32,
    #[serde(default)]
    provenance: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    negators: Vec<String>,
    #[serde(default)]
    negation_articles: Vec<String>,
    #[serde(default)]
    negators_after_ne: Vec<String>,
    #[serde(default)]
    ne_words: Vec<String>,
    #[serde(default)]
    ne_window: usize,
    #[serde(default)]
    negators_after_ne_not_after: Vec<String>,
    #[serde(default)]
    clause_breaks: Vec<String>,
    #[serde(default)]
    characterization_intro: String,
    #[serde(default)]
    sources: Vec<Source>,
    rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
pub(crate) struct Compiled {
    patterns: Vec<Vec<String>>,
    together: Vec<Vec<Vec<String>>>,
    except: Vec<Vec<String>>,
}

#[derive(Debug)]
pub struct RedFlags {
    pub(crate) version: u32,
    pub(crate) license: String,
    pub(crate) provenance: String,
    pub(crate) intro: String,
    pub(crate) sources: Vec<Source>,
    pub(crate) rules: Vec<Rule>,
    compiled: Vec<Compiled>,
    negators: Vec<Vec<String>>,
    articles: Vec<String>,
    after_ne: Vec<Vec<String>>,
    ne: Vec<String>,
    ne_window: usize,
    not_after: Vec<String>,
    breaks: Vec<String>,
}

/// A rule that fired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RedFlagHit {
    pub id: String,
    pub message: String,
    /// The number to put first on the help screen.
    pub first: String,
}

fn wildcard(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, part) in p.split('*').enumerate() {
        if i > 0 {
            out.push("*".to_string());
        }
        out.extend(words(part));
    }
    out
}

fn split_clauses(text: &str, breaks: &[String]) -> Vec<String> {
    let mut clauses = Vec::new();
    let mut rest = text;
    loop {
        let cut = breaks
            .iter()
            .filter(|b| !b.is_empty())
            .filter_map(|b| rest.find(b.as_str()).map(|i| (i, b.len())))
            .min_by_key(|(i, _)| *i);
        match cut {
            Some((i, len)) => {
                clauses.push(rest[..i].to_string());
                rest = &rest[i + len..];
            }
            None => break,
        }
    }
    clauses.push(rest.to_string());
    clauses
}

impl RedFlags {
    pub(crate) fn from_json(json: &str) -> Result<RedFlags, Error> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| Error(format!("red_flags: {e}")))?;
        if raw.schema != "memcp.health.red_flags/1" {
            return Err(Error(format!("red_flags: schema {}", raw.schema)));
        }
        if raw.rules.is_empty() {
            return Err(Error("red_flags: no rules".into()));
        }
        let mut compiled = Vec::new();
        for r in &raw.rules {
            if r.id.is_empty() || raw.rules.iter().filter(|x| x.id == r.id).count() > 1 {
                return Err(Error(format!("rule {:?} is empty or listed twice", r.id)));
            }
            if r.patterns.is_empty() && r.together.is_empty() {
                return Err(Error(format!("rule {} has no pattern", r.id)));
            }
            if r.together.len() == 1 || r.together.iter().any(Vec::is_empty) {
                return Err(Error(format!(
                    "rule {} needs at least two non-empty groups in together",
                    r.id
                )));
            }
            if r.message.is_empty() || r.first.trim().is_empty() {
                return Err(Error(format!("rule {} lacks a message or a number", r.id)));
            }
            let all = r
                .patterns
                .iter()
                .chain(r.together.iter().flatten())
                .chain(&r.except);
            if all.into_iter().any(|p| words(p).is_empty()) {
                return Err(Error(format!("rule {} has an empty pattern", r.id)));
            }
            compiled.push(Compiled {
                patterns: r.patterns.iter().map(|p| words(p)).collect(),
                together: r
                    .together
                    .iter()
                    .map(|g| g.iter().map(|p| words(p)).collect())
                    .collect(),
                except: r.except.iter().map(|p| wildcard(p)).collect(),
            });
        }
        let phrases = |l: &[String]| -> Vec<Vec<String>> {
            l.iter()
                .map(|n| words(n))
                .filter(|n| !n.is_empty())
                .collect()
        };
        Ok(RedFlags {
            version: raw.version,
            license: raw.license,
            provenance: raw.provenance,
            intro: raw.characterization_intro,
            sources: raw.sources,
            rules: raw.rules,
            compiled,
            negators: phrases(&raw.negators),
            articles: raw
                .negation_articles
                .iter()
                .flat_map(|a| words(a))
                .collect(),
            after_ne: phrases(&raw.negators_after_ne),
            ne: raw.ne_words.iter().flat_map(|a| words(a)).collect(),
            ne_window: raw.ne_window,
            not_after: raw
                .negators_after_ne_not_after
                .iter()
                .flat_map(|a| words(a))
                .collect(),
            breaks: raw.clause_breaks.iter().map(|b| b.to_lowercase()).collect(),
        })
    }

    /// True when the words right before position `i` really deny the sign. // law: doubt fires
    fn denies(&self, toks: &[String], i: usize) -> bool {
        let before = &toks[..i];
        let mut ends = vec![before.len()];
        if before.last().is_some_and(|t| self.articles.contains(t)) {
            ends.push(before.len() - 1);
        }
        ends.into_iter().any(|end| {
            let b = &before[..end];
            self.negators.iter().any(|n| b.ends_with(n))
                || self
                    .after_ne
                    .iter()
                    .any(|n| b.ends_with(n) && self.follows_ne(b, end - n.len()))
        })
    }

    fn follows_ne(&self, toks: &[String], start: usize) -> bool {
        if start > 0 && self.not_after.contains(&toks[start - 1]) {
            return false;
        }
        let from = start.saturating_sub(self.ne_window + 1);
        toks[from..start].iter().any(|t| self.ne.contains(t))
    }

    fn fires_in(&self, toks: &[String], pat: &[String], except: &[Vec<String>]) -> bool {
        if pat.is_empty() || toks.len() < pat.len() {
            return false;
        }
        (0..=toks.len() - pat.len()).any(|i| {
            toks[i..i + pat.len()] == pat[..]
                && !self.denies(toks, i)
                && !excepted(toks, i, pat.len(), except)
        })
    }

    /// One hit per rule that fires, in referential order.
    pub fn check(&self, text: &str) -> Vec<RedFlagHit> {
        let clauses: Vec<Vec<String>> = split_clauses(&text.to_lowercase(), &self.breaks)
            .iter()
            .map(|c| words(c))
            .collect();
        let mut hits = Vec::new();
        for (rule, c) in self.rules.iter().zip(&self.compiled) {
            let found =
                |pat: &Vec<String>| clauses.iter().any(|t| self.fires_in(t, pat, &c.except));
            let alone = c.patterns.iter().any(found);
            let combined = !c.together.is_empty() && c.together.iter().all(|g| g.iter().any(found));
            if alone || combined {
                hits.push(RedFlagHit {
                    id: rule.id.clone(),
                    message: rule.message.clone(),
                    first: rule.first.clone(),
                });
            }
        }
        hits
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// The text shown above every characterization sheet.
    pub fn characterization_intro(&self) -> &str {
        &self.intro
    }
}

fn occurs_at(toks: &[String], pat: &[String], j: usize) -> bool {
    j + pat.len() <= toks.len() && pat.iter().zip(&toks[j..]).all(|(p, t)| p == "*" || p == t)
}

fn excepted(toks: &[String], i: usize, len: usize, except: &[Vec<String>]) -> bool {
    except.iter().any(|e| {
        e.len() > len && ((i + len).saturating_sub(e.len())..=i).any(|j| occurs_at(toks, e, j))
    })
}
