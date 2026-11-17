//! Text normalisation, digest, accent folding and stemming.
//! The same processing is used when indexing and when querying, so "énergie"
//! and "energie" give the same stem.
//!
//! The language is chosen once per process with [`set_language`] (from the
//! configuration); it defaults to English. Changing it later requires a reindex.

use std::sync::OnceLock;

use rust_stemmers::{Algorithm, Stemmer};
use sha2::{Digest, Sha256};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Languages the search understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// English stemming and stopwords (code `en`, the default).
    English,
    /// French stemming and stopwords (code `fr`).
    French,
}

impl Language {
    /// Reads a language code from the configuration: `"en"` or `"fr"`, `None` for anything else.
    pub fn parse(code: &str) -> Option<Language> {
        match code {
            "en" => Some(Language::English),
            "fr" => Some(Language::French),
            _ => None,
        }
    }

    /// The configuration code of the language (`"en"` or `"fr"`); inverse of [`Language::parse`].
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::French => "fr",
        }
    }

    fn algorithm(self) -> Algorithm {
        match self {
            Language::English => Algorithm::English,
            Language::French => Algorithm::French,
        }
    }

    fn stopwords(self) -> &'static [&'static str] {
        match self {
            Language::English => EN_STOPWORDS,
            Language::French => FR_STOPWORDS,
        }
    }
}

static LANGUAGE: OnceLock<Language> = OnceLock::new();

/// Sets the process language. Only the first call counts; returns `false` if a
/// different language was already set.
pub fn set_language(lang: Language) -> bool {
    *LANGUAGE.get_or_init(|| lang) == lang
}

/// The process language: the one given to [`set_language`], English if none was.
pub fn language() -> Language {
    *LANGUAGE.get_or_init(|| Language::English)
}

/// Common English stopwords (already folded).
const EN_STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "at", "by", "for", "with", "without",
    "is", "are", "was", "were", "be", "been", "it", "its", "this", "that", "these", "those", "i",
    "you", "he", "she", "we", "they", "me", "my", "your", "his", "her", "our", "their", "not",
    "no", "do", "does", "did", "as", "from", "so", "s", "t",
];

/// Common French stopwords (already folded: no accents).
const FR_STOPWORDS: &[&str] = &[
    "le", "la", "les", "l", "un", "une", "des", "du", "de", "d", "et", "ou", "a", "au", "aux",
    "en", "est", "sont", "ce", "ces", "cet", "cette", "il", "elle", "ils", "elles", "on", "je",
    "tu", "nous", "vous", "me", "te", "se", "mon", "ma", "mes", "ton", "ta", "tes", "son", "sa",
    "ses", "que", "qui", "quoi", "dans", "par", "pour", "sur", "avec", "sans", "ne", "pas", "y",
    "qu", "s", "n", "c", "j", "m", "t",
];

/// NFC, whitespace collapsed to single spaces, trimmed. This is the text as stored.
pub fn normalize(text: &str) -> String {
    let nfc: String = text.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Hex SHA-256 of the normalised text.
pub fn text_hash(text: &str) -> String {
    hex(&Sha256::digest(normalize(text).as_bytes()))
}

/// Lowercase hexadecimal text of `bytes` (two characters per byte).
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Lowercase, accents removed.
pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect()
}

/// Stems of a text in the process language.
pub fn stems(text: &str) -> Vec<String> {
    stems_in(language(), text)
}

/// Stems of a text: folded, split on non-alphanumerics, stopwords removed, stemmed.
/// Order and duplicates are kept (term frequency matters for BM25).
pub fn stems_in(lang: Language, text: &str) -> Vec<String> {
    let stemmer = Stemmer::create(lang.algorithm());
    let stop = lang.stopwords();
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !stop.contains(w))
        .map(|w| stemmer.stem(w).into_owned())
        .collect()
}

/// Jaccard index of two stem sets: 0 (nothing shared) to 1 (same words).
pub fn similarity(a: &[String], b: &[String]) -> f64 {
    use std::collections::BTreeSet;
    let a: BTreeSet<&String> = a.iter().collect();
    let b: BTreeSet<&String> = b.iter().collect();
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    inter / union
}

#[cfg(test)]
mod tests {
    use super::*;
    use Language::{English as En, French as Fr};

    #[test]
    fn normalises_whitespace_and_unicode() {
        assert_eq!(normalize("  a   b\n c "), "a b c");
        // decomposed and precomposed "é" are the same
        assert_eq!(normalize("e\u{301}"), normalize("\u{e9}"));
        assert_eq!(text_hash("Green  tea"), text_hash("Green tea"));
    }

    #[test]
    fn accents_and_case_are_folded() {
        assert_eq!(fold("Énergie"), "energie");
        assert_eq!(stems_in(Fr, "énergie"), stems_in(Fr, "ENERGIE"));
    }

    #[test]
    fn french_plurals_and_conjugations_meet() {
        assert_eq!(stems_in(Fr, "chevaux"), stems_in(Fr, "cheval"));
        assert_eq!(stems_in(Fr, "séances"), stems_in(Fr, "séance"));
        assert_eq!(stems_in(Fr, "travaillait"), stems_in(Fr, "travaille"));
    }

    #[test]
    fn english_plurals_and_tenses_meet() {
        assert_eq!(stems_in(En, "sessions"), stems_in(En, "session"));
        assert_eq!(stems_in(En, "running"), stems_in(En, "runs"));
    }

    #[test]
    fn stopwords_are_ignored() {
        assert_eq!(
            stems_in(Fr, "le chat et la souris"),
            stems_in(Fr, "chat souris")
        );
        assert_eq!(
            stems_in(En, "the cat and the mouse"),
            stems_in(En, "cat mouse")
        );
        assert!(stems_in(En, "the and of").is_empty());
    }

    #[test]
    fn similarity_index() {
        let a = stems_in(En, "Sam likes green tea");
        let b = stems_in(En, "Sam hates black coffee");
        let c = stems_in(En, "Sam likes green tea a lot");
        assert!(similarity(&a, &b) < 0.5);
        assert!(similarity(&a, &c) >= 0.5);
        assert_eq!(similarity(&a, &a), 1.0);
    }
}
