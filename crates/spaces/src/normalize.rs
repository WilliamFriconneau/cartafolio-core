//! Lowercase, no accents, whole words: what the router compares.

fn fold(c: char, out: &mut String) {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => out.push('a'),
        'ç' => out.push('c'),
        'é' | 'è' | 'ê' | 'ë' => out.push('e'),
        'î' | 'ï' | 'í' | 'ì' => out.push('i'),
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => out.push('o'),
        'ù' | 'û' | 'ü' | 'ú' => out.push('u'),
        'ÿ' | 'ý' => out.push('y'),
        'ñ' => out.push('n'),
        'œ' => out.push_str("oe"),
        'æ' => out.push_str("ae"),
        c if c.is_alphanumeric() => out.push(c),
        _ => out.push(' '),
    }
}

/// The words of `text`, lowercased and without accents; every other sign separates.
pub fn normalize(text: &str) -> Vec<String> {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        fold(c, &mut out);
    }
    out.split_whitespace().map(str::to_string).collect()
}

/// True when the words of `phrase` appear one after the other in `words`.
pub(crate) fn contains(words: &[String], phrase: &str) -> bool {
    let wanted = normalize(phrase);
    !wanted.is_empty() && words.windows(wanted.len()).any(|w| w == wanted.as_slice())
}

/// The hashtags of `text` (`#santé` and `#sante` alike), lowercased, without accents.
pub fn hashtags(text: &str) -> Vec<String> {
    text.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | '.' | ';' | ':' | '!' | '?' | '(' | ')')
    })
    .filter_map(|t| t.strip_prefix('#'))
    .filter_map(|t| normalize(t).into_iter().next())
    .collect()
}

/// True when `word` is said in the note: a hashtag (`#maison`) only as a hashtag, never as the plain word.
pub(crate) fn said(words: &[String], tags: &[String], word: &str) -> bool {
    match word.strip_prefix('#') {
        Some(tag) => normalize(tag).first().is_some_and(|t| tags.contains(t)),
        None => contains(words, word),
    }
}
