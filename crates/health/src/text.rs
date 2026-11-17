//! Folding and tokens: the text as the detection reads it. Dates and clock times are tokens of their own, so they are
//! never taken for a value.

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Lowercase, without accents (`œ` and `æ` spelled out), the typographic apostrophe made plain.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().nfd() {
        match c {
            c if is_combining_mark(c) => {}
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            '\u{2019}' | '\u{2018}' | '`' => out.push('\''),
            c => out.push(c),
        }
    }
    out
}

/// The words of a text, folded: every run of letters and digits (the pattern side of a match).
pub fn words(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// A token of a folded text.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// Letters, then letters or digits (`spo2`).
    Word(String),
    /// A number: `72.5` (a comma is a decimal point), `12/8` (`pair` is the part after the slash).
    Num {
        text: String,
        pair: Option<String>,
        int: bool,
    },
    /// `12:30`, `12h30`; for the `h` form, the time as hours (`6h30` is `6.5`), which only a duration may read.
    Time(Option<f64>),
    /// `2026-11-17`, `17/11/2026`.
    Date,
    /// Any other character that is not a space; a line break is kept as `\n`.
    Sym(char),
}

pub fn tokenize(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = fold(text).chars().collect();
    let n = chars.len();
    let digit = |k: usize| k < n && chars[k].is_ascii_digit();
    let run = |mut k: usize| {
        while digit(k) {
            k += 1;
        }
        k
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            i = run(i);
            let int_part: String = chars[start..i].iter().collect();
            // 12:30, 12:30:15
            if i < n && chars[i] == ':' && digit(i + 1) {
                i = run(i + 1);
                if i < n && chars[i] == ':' && digit(i + 1) {
                    i = run(i + 1);
                }
                out.push(Tok::Time(None));
                continue;
            }
            // 12h30, 12h
            if i < n
                && chars[i] == 'h'
                && (digit(i + 1) || i + 1 >= n || !chars[i + 1].is_alphabetic())
            {
                let hours: f64 = int_part.parse().unwrap_or(f64::NAN);
                let m_start = i + 1;
                i = run(m_start);
                let minutes: f64 = if i > m_start {
                    chars[m_start..i]
                        .iter()
                        .collect::<String>()
                        .parse()
                        .unwrap_or(f64::NAN)
                } else {
                    0.0
                };
                let value = (hours.is_finite() && minutes.is_finite() && minutes < 60.0)
                    .then(|| hours + minutes / 60.0);
                out.push(Tok::Time(value));
                continue;
            }
            // 2026-11-17
            if i < n && chars[i] == '-' && digit(i + 1) {
                let j = run(i + 1);
                if j < n && chars[j] == '-' && digit(j + 1) {
                    i = run(j + 1);
                    out.push(Tok::Date);
                    continue;
                }
            }
            // 72,5 or 72.5
            if i + 1 < n && (chars[i] == '.' || chars[i] == ',') && digit(i + 1) {
                let j = run(i + 1);
                let frac: String = chars[i + 1..j].iter().collect();
                i = j;
                out.push(Tok::Num {
                    text: format!("{int_part}.{frac}"),
                    pair: None,
                    int: false,
                });
                continue;
            }
            // 12/8, and 17/11/2026 which is a date
            if i < n && chars[i] == '/' && digit(i + 1) {
                let j = run(i + 1);
                let second: String = chars[i + 1..j].iter().collect();
                if j < n && chars[j] == '/' && digit(j + 1) {
                    i = run(j + 1);
                    out.push(Tok::Date);
                    continue;
                }
                i = j;
                out.push(Tok::Num {
                    text: int_part,
                    pair: Some(second),
                    int: true,
                });
                continue;
            }
            out.push(Tok::Num {
                text: int_part,
                pair: None,
                int: true,
            });
            continue;
        }
        if c.is_alphabetic() {
            let s = i;
            while i < n && chars[i].is_alphanumeric() {
                i += 1;
            }
            out.push(Tok::Word(chars[s..i].iter().collect()));
            continue;
        }
        if c == '\n' || !c.is_whitespace() {
            out.push(Tok::Sym(c));
        }
        i += 1;
    }
    out
}

pub fn is_word(t: Option<&Tok>, w: &str) -> bool {
    matches!(t, Some(Tok::Word(x)) if x == w)
}

pub fn phrase_at(toks: &[Tok], i: usize, phrase: &[String]) -> bool {
    !phrase.is_empty()
        && phrase
            .iter()
            .enumerate()
            .all(|(k, w)| is_word(toks.get(i + k), w))
}
