use crate::StoreError;
use std::collections::HashSet;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

pub fn normalize_tag(tag: &str) -> Result<String, StoreError> {
    let normalized: String = tag.chars().flat_map(char::to_lowercase).nfc().collect();
    let mut levels = normalized.split('/');
    if levels.clone().count() > 4
        || levels.any(|level| {
            !(1..=40).contains(&level.chars().count())
                || !level
                    .chars()
                    .all(|ch| ch.is_alphabetic() || ch.is_numeric() || matches!(ch, '-' | '_'))
        })
    {
        return Err(StoreError::Invalid(
            "Tags must have one to four slash-separated levels of 1 to 40 letters, digits, hyphens or underscores".into(),
        ));
    }
    Ok(normalized)
}

pub(crate) fn normalize_tags(tags: &[String]) -> Result<Vec<String>, StoreError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for tag in tags {
        let tag = normalize_tag(tag)?;
        if seen.insert(tag.clone()) {
            normalized.push(tag);
        }
    }
    Ok(normalized)
}

pub(crate) fn fold_tag(tag: &str) -> String {
    tag.nfd().filter(|&ch| !is_combining_mark(ch)).collect()
}
